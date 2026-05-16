use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_types::events::RuntimeEvent;
use tokio::sync::{broadcast, mpsc, Mutex, Notify};
use tonic::Status;
use uuid::Uuid;

use crate::grpc::proto::LifecycleEvent;

const BROADCAST_CAPACITY: usize = 512;
const STREAM_CAPACITY: usize = 64;
const IDLE_BROKER_TICKS: u32 = 12;

#[cfg(feature = "postgres")]
pub const EVENT_NOTIFY_CHANNEL: &str = "bpmn_lite_events";

pub struct EventFanout {
    engine: Arc<BpmnLiteEngine>,
    fallback_interval: Duration,
    brokers: Arc<Mutex<HashMap<Uuid, Arc<InstanceBroker>>>>,
}

impl EventFanout {
    pub fn new(engine: Arc<BpmnLiteEngine>, fallback_interval: Duration) -> Self {
        Self {
            engine,
            fallback_interval,
            brokers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn subscribe(
        &self,
        instance_id: Uuid,
    ) -> Result<mpsc::Receiver<Result<LifecycleEvent, Status>>, Status> {
        let broker = self.broker_for(instance_id).await;
        let mut rx = broker.subscribe();

        let replay = self
            .engine
            .read_events(instance_id, 0)
            .await
            .map_err(engine_err)?;

        let (tx, stream_rx) = mpsc::channel(STREAM_CAPACITY);
        let mut next_seq = 0;
        let mut terminal = false;

        for (seq, event) in replay {
            terminal |= is_terminal_event(&event);
            next_seq = seq.saturating_add(1);
            if tx
                .send(Ok(lifecycle_event(instance_id, seq, &event)))
                .await
                .is_err()
            {
                return Ok(stream_rx);
            }
        }

        if terminal {
            return Ok(stream_rx);
        }

        tokio::spawn(async move {
            while let Ok((seq, event)) = rx.recv().await {
                if seq < next_seq {
                    continue;
                }

                let terminal = is_terminal_event(&event);
                next_seq = seq.saturating_add(1);

                if tx
                    .send(Ok(lifecycle_event(instance_id, seq, &event)))
                    .await
                    .is_err()
                {
                    break;
                }

                if terminal {
                    break;
                }
            }
        });

        Ok(stream_rx)
    }

    pub async fn notify(&self, instance_id: Uuid) {
        let broker = {
            let guard = self.brokers.lock().await;
            guard.get(&instance_id).cloned()
        };
        if let Some(broker) = broker {
            broker.wake();
        }
    }

    async fn broker_for(&self, instance_id: Uuid) -> Arc<InstanceBroker> {
        let mut guard = self.brokers.lock().await;
        if let Some(broker) = guard.get(&instance_id) {
            if !broker.is_stopped() {
                return broker.clone();
            }
        }

        let broker = InstanceBroker::new(instance_id, self.engine.clone(), self.fallback_interval);
        broker.start();
        guard.insert(instance_id, broker.clone());
        self.spawn_cleanup(instance_id, broker.clone());
        broker
    }

    fn spawn_cleanup(&self, instance_id: Uuid, broker: Arc<InstanceBroker>) {
        let brokers = self.brokers.clone();
        let interval = self.fallback_interval;
        tokio::spawn(async move {
            loop {
                if broker.is_stopped() {
                    let mut guard = brokers.lock().await;
                    if guard
                        .get(&instance_id)
                        .is_some_and(|current| Arc::ptr_eq(current, &broker))
                    {
                        guard.remove(&instance_id);
                    }
                    break;
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    #[cfg(feature = "postgres")]
    pub async fn start_postgres_listener(
        self: &Arc<Self>,
        database_url: String,
    ) -> anyhow::Result<()> {
        let mut listener = connect_postgres_listener(&database_url).await?;

        let fanout = self.clone();
        tokio::spawn(async move {
            loop {
                match listener.recv().await {
                    Ok(notification) => match Uuid::parse_str(notification.payload()) {
                        Ok(instance_id) => fanout.notify(instance_id).await,
                        Err(error) => tracing::warn!(
                            payload = notification.payload(),
                            %error,
                            "ignored invalid BPMN-Lite event notification"
                        ),
                    },
                    Err(error) => {
                        tracing::error!(%error, "Postgres LISTEN bpmn_lite_events failed");
                        listener = loop {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            match connect_postgres_listener(&database_url).await {
                                Ok(listener) => break listener,
                                Err(error) => tracing::error!(
                                    %error,
                                    "Postgres LISTEN bpmn_lite_events reconnect failed"
                                ),
                            }
                        };
                    }
                }
            }
        });

        Ok(())
    }
}

#[cfg(feature = "postgres")]
async fn connect_postgres_listener(
    database_url: &str,
) -> anyhow::Result<sqlx::postgres::PgListener> {
    let mut listener = sqlx::postgres::PgListener::connect(database_url).await?;
    listener.listen(EVENT_NOTIFY_CHANNEL).await?;
    Ok(listener)
}

struct InstanceBroker {
    instance_id: Uuid,
    engine: Arc<BpmnLiteEngine>,
    fallback_interval: Duration,
    notify: Notify,
    stopped: AtomicBool,
    tx: broadcast::Sender<(u64, RuntimeEvent)>,
}

impl InstanceBroker {
    fn new(
        instance_id: Uuid,
        engine: Arc<BpmnLiteEngine>,
        fallback_interval: Duration,
    ) -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            instance_id,
            engine,
            fallback_interval,
            notify: Notify::new(),
            stopped: AtomicBool::new(false),
            tx,
        })
    }

    fn start(self: &Arc<Self>) {
        let broker = self.clone();
        tokio::spawn(async move {
            broker.run().await;
        });
    }

    fn subscribe(&self) -> broadcast::Receiver<(u64, RuntimeEvent)> {
        self.tx.subscribe()
    }

    fn wake(&self) {
        self.notify.notify_waiters();
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    async fn run(self: Arc<Self>) {
        let mut next_seq = 0;
        let mut idle_ticks = 0;

        loop {
            let events = match self.engine.read_events(self.instance_id, next_seq).await {
                Ok(events) => events,
                Err(error) => {
                    tracing::warn!(
                        instance_id = %self.instance_id,
                        error = %error,
                        "event fanout broker read failed"
                    );
                    Vec::new()
                }
            };

            let mut delivered = false;
            let mut terminal = false;
            for (seq, event) in events {
                next_seq = seq.saturating_add(1);
                terminal |= is_terminal_event(&event);
                delivered = true;
                let _ = self.tx.send((seq, event));
            }

            if terminal {
                self.stopped.store(true, Ordering::Relaxed);
                break;
            }

            if delivered || self.tx.receiver_count() > 0 {
                idle_ticks = 0;
            } else {
                idle_ticks += 1;
                if idle_ticks >= IDLE_BROKER_TICKS {
                    self.stopped.store(true, Ordering::Relaxed);
                    break;
                }
            }

            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(self.fallback_interval) => {}
            }
        }
    }
}

fn lifecycle_event(instance_id: Uuid, seq: u64, event: &RuntimeEvent) -> LifecycleEvent {
    LifecycleEvent {
        sequence: seq,
        event_type: event_type(event),
        process_instance_id: instance_id.to_string(),
        payload_json: serde_json::to_string(event).unwrap_or_default(),
    }
}

fn event_type(event: &RuntimeEvent) -> String {
    let event_type = format!("{:?}", event);
    event_type
        .split_once('{')
        .or_else(|| event_type.split_once(' '))
        .map(|(name, _)| name.trim().to_string())
        .unwrap_or(event_type)
}

fn is_terminal_event(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::Completed { .. }
            | RuntimeEvent::Cancelled { .. }
            | RuntimeEvent::Terminated { .. }
            | RuntimeEvent::IncidentCreated { .. }
    )
}

fn engine_err(error: anyhow::Error) -> Status {
    Status::internal(format!("{:#}", error))
}
