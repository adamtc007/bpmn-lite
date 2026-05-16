use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::sleep;
use tonic::transport::Channel;

use bpmn_lite_server::grpc::proto::bpmn_lite_client::BpmnLiteClient;
use bpmn_lite_server::grpc::proto::{
    ActivateJobsRequest, CancelRequest, CompileRequest, CompleteJobRequest, FailJobRequest,
    HealthRequest, InspectRequest, JobActivationMsg, MetricsRequest, ProtoValue, SignalRequest,
    StartRequest, SubscribeRequest,
};

#[tokio::main]
async fn main() -> Result<()> {
    run_from_args(std::env::args().skip(1)).await
}

const SINGLE_TASK_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="single_task" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="do_work" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

const TWO_TASK_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="two_task" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task_a" name="step_one" />
    <bpmn:serviceTask id="task_b" name="step_two" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task_a" />
    <bpmn:sequenceFlow id="f2" sourceRef="task_a" targetRef="task_b" />
    <bpmn:sequenceFlow id="f3" sourceRef="task_b" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

const APPROVAL_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="approval_gate" isExecutable="true">
    <bpmn:startEvent id="start1" />
    <bpmn:serviceTask id="review_task" name="review_case" />
    <bpmn:exclusiveGateway id="gw1" name="Decision" />
    <bpmn:serviceTask id="task_a" name="approve_case" />
    <bpmn:serviceTask id="task_b" name="reject_case" />
    <bpmn:endEvent id="end1" />
    <bpmn:sequenceFlow id="f1" sourceRef="start1" targetRef="review_task" />
    <bpmn:sequenceFlow id="f2" sourceRef="review_task" targetRef="gw1" />
    <bpmn:sequenceFlow id="f3" sourceRef="gw1" targetRef="task_a">
      <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f4" sourceRef="gw1" targetRef="task_b" />
    <bpmn:sequenceFlow id="f5" sourceRef="task_a" targetRef="end1" />
    <bpmn:sequenceFlow id="f6" sourceRef="task_b" targetRef="end1" />
  </bpmn:process>
</bpmn:definitions>"#;

#[derive(Clone, Copy)]
struct WorkflowFixture {
    key: &'static str,
    bpmn_xml: &'static str,
    task_types: &'static [&'static str],
    signal_messages: &'static [&'static str],
}

const FIXTURES: &[WorkflowFixture] = &[
    WorkflowFixture {
        key: "single_task",
        bpmn_xml: SINGLE_TASK_BPMN,
        task_types: &["do_work"],
        signal_messages: &[],
    },
    WorkflowFixture {
        key: "two_task",
        bpmn_xml: TWO_TASK_BPMN,
        task_types: &["step_one", "step_two"],
        signal_messages: &[],
    },
    WorkflowFixture {
        key: "approval_gate",
        bpmn_xml: APPROVAL_BPMN,
        task_types: &["review_case", "approve_case", "reject_case"],
        signal_messages: &[],
    },
];

#[derive(Clone)]
struct CompiledFixture {
    fixture: WorkflowFixture,
    bytecode_version: Vec<u8>,
}

#[derive(Clone)]
struct InstanceTracker {
    process_instance_id: String,
    workflow_key: String,
    signal_messages: Vec<String>,
    terminal_state: Option<String>,
}

#[derive(Clone)]
struct HarnessConfig {
    server_url: String,
    subscription_server_url: Option<String>,
    instances: usize,
    workers: usize,
    max_jobs_per_poll: i32,
    signal_interval_ms: u64,
    inspect_concurrency: usize,
    timeout_secs: u64,
    profile: String,
    fail_every: usize,
    ghost_signal_every: usize,
    cancel_after_fail: bool,
    seed: u64,
    subscriptions_per_instance: usize,
}

impl HarnessConfig {
    fn smoke(server_url: String) -> Self {
        Self {
            server_url,
            subscription_server_url: None,
            instances: 16,
            workers: 4,
            max_jobs_per_poll: 8,
            signal_interval_ms: 75,
            inspect_concurrency: 8,
            timeout_secs: 30,
            profile: "smoke".to_string(),
            fail_every: 0,
            ghost_signal_every: 5,
            cancel_after_fail: true,
            seed: 7,
            subscriptions_per_instance: 0,
        }
    }

    fn stress(server_url: String) -> Self {
        Self {
            server_url,
            subscription_server_url: None,
            instances: 250,
            workers: 16,
            max_jobs_per_poll: 32,
            signal_interval_ms: 25,
            inspect_concurrency: 32,
            timeout_secs: 120,
            profile: "stress".to_string(),
            fail_every: 13,
            ghost_signal_every: 7,
            cancel_after_fail: true,
            seed: 97,
            subscriptions_per_instance: 0,
        }
    }

    fn poison(server_url: String) -> Self {
        Self {
            server_url,
            subscription_server_url: None,
            instances: 50,
            workers: 8,
            max_jobs_per_poll: 16,
            signal_interval_ms: 50,
            inspect_concurrency: 16,
            timeout_secs: 90,
            profile: "poison".to_string(),
            fail_every: 1,
            ghost_signal_every: 5,
            cancel_after_fail: false,
            seed: 211,
            subscriptions_per_instance: 0,
        }
    }

    fn subscription(server_url: String) -> Self {
        Self {
            server_url,
            subscription_server_url: None,
            instances: 24,
            workers: 6,
            max_jobs_per_poll: 12,
            signal_interval_ms: 50,
            inspect_concurrency: 12,
            timeout_secs: 60,
            profile: "subscription".to_string(),
            fail_every: 0,
            ghost_signal_every: 5,
            cancel_after_fail: true,
            seed: 313,
            subscriptions_per_instance: 3,
        }
    }
}

#[derive(Default)]
struct Metrics {
    instances_started: AtomicUsize,
    job_activations: AtomicUsize,
    job_completions: AtomicUsize,
    job_failures: AtomicUsize,
    signals_sent: AtomicUsize,
    inspections: AtomicUsize,
    subscription_events: AtomicUsize,
}

#[derive(Default)]
struct Summary {
    terminal_counts: HashMap<String, usize>,
    workflow_counts: HashMap<String, usize>,
}

#[derive(Default)]
struct Sequence {
    state: AtomicU64,
}

impl Sequence {
    fn with_seed(seed: u64) -> Self {
        Self {
            state: AtomicU64::new(seed),
        }
    }

    fn next(&self) -> u64 {
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            let next = current
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            match self
                .state
                .compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(observed) => current = observed,
            }
        }
    }
}

#[derive(Clone)]
struct WorkerLoopConfig {
    worker_idx: usize,
    server_url: String,
    task_types: Vec<String>,
    metrics: Arc<Metrics>,
    shutdown: Arc<AtomicUsize>,
    sequence: Arc<Sequence>,
    fail_every: usize,
    cancel_after_fail: bool,
    max_jobs: i32,
}

async fn run_from_args<I>(args: I) -> Result<()>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let config = parse_args(&args)?;
    run_harness(config).await
}

fn parse_args(args: &[String]) -> Result<HarnessConfig> {
    let mut profile = "stress".to_string();
    let mut server_url = "http://127.0.0.1:50051".to_string();
    let mut subscription_server_url = None;
    let mut instances = None;
    let mut workers = None;
    let mut max_jobs_per_poll = None;
    let mut signal_interval_ms = None;
    let mut inspect_concurrency = None;
    let mut timeout_secs = None;
    let mut fail_every = None;
    let mut ghost_signal_every = None;
    let mut cancel_after_fail = None;
    let mut seed = None;
    let mut subscriptions_per_instance = None;

    let mut i = 0usize;
    while i < args.len() {
        let key = &args[i];
        let value = args.get(i + 1);
        match key.as_str() {
            "--profile" => {
                profile = value
                    .ok_or_else(|| anyhow!("--profile requires a value"))?
                    .clone();
                i += 2;
            }
            "--server-url" => {
                server_url = value
                    .ok_or_else(|| anyhow!("--server-url requires a value"))?
                    .clone();
                i += 2;
            }
            "--subscription-server-url" => {
                subscription_server_url = Some(
                    value
                        .ok_or_else(|| anyhow!("--subscription-server-url requires a value"))?
                        .clone(),
                );
                i += 2;
            }
            "--instances" => {
                instances = Some(parse_usize_flag("--instances", value)?);
                i += 2;
            }
            "--workers" => {
                workers = Some(parse_usize_flag("--workers", value)?);
                i += 2;
            }
            "--max-jobs" => {
                max_jobs_per_poll = Some(parse_i32_flag("--max-jobs", value)?);
                i += 2;
            }
            "--signal-interval-ms" => {
                signal_interval_ms = Some(parse_u64_flag("--signal-interval-ms", value)?);
                i += 2;
            }
            "--inspect-concurrency" => {
                inspect_concurrency = Some(parse_usize_flag("--inspect-concurrency", value)?);
                i += 2;
            }
            "--timeout-secs" => {
                timeout_secs = Some(parse_u64_flag("--timeout-secs", value)?);
                i += 2;
            }
            "--fail-every" => {
                fail_every = Some(parse_usize_flag("--fail-every", value)?);
                i += 2;
            }
            "--ghost-signal-every" => {
                ghost_signal_every = Some(parse_usize_flag("--ghost-signal-every", value)?);
                i += 2;
            }
            "--cancel-after-fail" => {
                cancel_after_fail = Some(true);
                i += 1;
            }
            "--no-cancel-after-fail" => {
                cancel_after_fail = Some(false);
                i += 1;
            }
            "--seed" => {
                seed = Some(parse_u64_flag("--seed", value)?);
                i += 2;
            }
            "--subscriptions-per-instance" => {
                subscriptions_per_instance =
                    Some(parse_usize_flag("--subscriptions-per-instance", value)?);
                i += 2;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => bail!("Unknown argument: {}", unknown),
        }
    }

    let mut config = match profile.as_str() {
        "smoke" => HarnessConfig::smoke(server_url),
        "stress" => HarnessConfig::stress(server_url),
        "poison" | "poison-jobs" => HarnessConfig::poison(server_url),
        "subscription" | "subscription-fanout" => HarnessConfig::subscription(server_url),
        other => bail!(
            "Unknown profile '{}'. Use smoke, stress, poison, or subscription.",
            other
        ),
    };

    if let Some(value) = instances {
        config.instances = value;
    }
    config.subscription_server_url = subscription_server_url;
    if let Some(value) = workers {
        config.workers = value;
    }
    if let Some(value) = max_jobs_per_poll {
        config.max_jobs_per_poll = value;
    }
    if let Some(value) = signal_interval_ms {
        config.signal_interval_ms = value;
    }
    if let Some(value) = inspect_concurrency {
        config.inspect_concurrency = value;
    }
    if let Some(value) = timeout_secs {
        config.timeout_secs = value;
    }
    if let Some(value) = fail_every {
        config.fail_every = value;
    }
    if let Some(value) = ghost_signal_every {
        config.ghost_signal_every = value.max(1);
    }
    if let Some(value) = cancel_after_fail {
        config.cancel_after_fail = value;
    }
    if let Some(value) = seed {
        config.seed = value;
    }
    if let Some(value) = subscriptions_per_instance {
        config.subscriptions_per_instance = value;
    }

    Ok(config)
}

fn parse_usize_flag(name: &str, value: Option<&String>) -> Result<usize> {
    value
        .ok_or_else(|| anyhow!("{} requires a value", name))?
        .parse::<usize>()
        .with_context(|| format!("{} must be a positive integer", name))
}

fn parse_i32_flag(name: &str, value: Option<&String>) -> Result<i32> {
    value
        .ok_or_else(|| anyhow!("{} requires a value", name))?
        .parse::<i32>()
        .with_context(|| format!("{} must be an integer", name))
}

fn parse_u64_flag(name: &str, value: Option<&String>) -> Result<u64> {
    value
        .ok_or_else(|| anyhow!("{} requires a value", name))?
        .parse::<u64>()
        .with_context(|| format!("{} must be an integer", name))
}

fn print_help() {
    eprintln!(
        "Usage: cargo run -p bpmn-lite-server --bin load_harness -- \
         [--profile smoke|stress] [--server-url URL] [--subscription-server-url URL] \
         [--instances N] [--workers N] \
         [--max-jobs N] [--signal-interval-ms N] [--inspect-concurrency N] \
         [--timeout-secs N] [--fail-every N] [--ghost-signal-every N] \
         [--cancel-after-fail|--no-cancel-after-fail] [--seed N] \
         [--subscriptions-per-instance N]"
    );
}

fn fixture_by_key(key: &str) -> Option<&'static WorkflowFixture> {
    FIXTURES.iter().find(|fixture| fixture.key == key)
}

async fn run_harness(config: HarnessConfig) -> Result<()> {
    let started_at = Instant::now();
    println!(
        "Running BPMN-Lite harness profile={} server={} instances={} workers={}",
        config.profile, config.server_url, config.instances, config.workers
    );

    assert_service_ready(&config.server_url).await?;
    let compiled = compile_fixtures(&config.server_url).await?;
    let task_types = compiled
        .iter()
        .flat_map(|fixture| fixture.fixture.task_types.iter().copied())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let instances = Arc::new(Mutex::new(Vec::<InstanceTracker>::with_capacity(
        config.instances,
    )));
    let metrics = Arc::new(Metrics::default());
    let sequence = Arc::new(Sequence::with_seed(config.seed));

    start_instances(
        &config.server_url,
        &compiled,
        &instances,
        &metrics,
        &sequence,
        config.instances,
    )
    .await?;

    let subscription_handles = spawn_subscription_watchers(&config, &instances, &metrics).await;

    let shutdown = Arc::new(AtomicUsize::new(0));
    let mut worker_handles = Vec::with_capacity(config.workers);
    for worker_idx in 0..config.workers {
        let url = config.server_url.clone();
        let task_types = task_types.clone();
        let metrics = Arc::clone(&metrics);
        let shutdown = Arc::clone(&shutdown);
        let sequence = Arc::clone(&sequence);
        let fail_every = config.fail_every;
        let cancel_after_fail = config.cancel_after_fail;
        let max_jobs = config.max_jobs_per_poll;
        worker_handles.push(tokio::spawn(async move {
            worker_loop(WorkerLoopConfig {
                worker_idx,
                server_url: url,
                task_types,
                metrics,
                shutdown,
                sequence,
                fail_every,
                cancel_after_fail,
                max_jobs,
            })
            .await
        }));
    }

    let signal_handle = {
        let url = config.server_url.clone();
        let instances = Arc::clone(&instances);
        let metrics = Arc::clone(&metrics);
        let shutdown = Arc::clone(&shutdown);
        let sequence = Arc::clone(&sequence);
        tokio::spawn(async move {
            signal_loop(
                url,
                instances,
                metrics,
                shutdown,
                sequence,
                config.signal_interval_ms,
                config.ghost_signal_every,
            )
            .await
        })
    };

    let summary =
        monitor_until_terminal(&config, &instances, &metrics, &shutdown, started_at).await?;

    shutdown.store(1, Ordering::Relaxed);

    for handle in worker_handles {
        handle.await??;
    }
    signal_handle.await??;
    wait_for_subscription_watchers(subscription_handles).await?;

    emit_summary(&config, &metrics, &summary, started_at.elapsed());
    assert_service_metrics(&config.server_url).await?;
    Ok(())
}

async fn spawn_subscription_watchers(
    config: &HarnessConfig,
    instances: &Arc<Mutex<Vec<InstanceTracker>>>,
    metrics: &Arc<Metrics>,
) -> Vec<tokio::task::JoinHandle<Result<usize>>> {
    if config.subscriptions_per_instance == 0 {
        return Vec::new();
    }

    let snapshot = instances.lock().await.clone();
    let mut handles = Vec::with_capacity(snapshot.len() * config.subscriptions_per_instance);
    for tracker in snapshot {
        for subscriber_idx in 0..config.subscriptions_per_instance {
            let server_url = config
                .subscription_server_url
                .clone()
                .unwrap_or_else(|| config.server_url.clone());
            let process_instance_id = tracker.process_instance_id.clone();
            let metrics = Arc::clone(metrics);
            handles.push(tokio::spawn(async move {
                subscription_watcher(server_url, process_instance_id, subscriber_idx, metrics).await
            }));
        }
    }
    handles
}

async fn subscription_watcher(
    server_url: String,
    process_instance_id: String,
    subscriber_idx: usize,
    metrics: Arc<Metrics>,
) -> Result<usize> {
    let mut client = connect_client(&server_url).await?;
    let mut stream = client
        .subscribe_events(SubscribeRequest {
            process_instance_id: process_instance_id.clone(),
            tenant_id: String::new(),
        })
        .await
        .with_context(|| {
            format!(
                "SubscribeEvents failed for {} subscriber {}",
                process_instance_id, subscriber_idx
            )
        })?
        .into_inner();

    let mut count = 0usize;
    while let Some(event) = stream.message().await? {
        if event.process_instance_id != process_instance_id {
            bail!(
                "subscriber {} received event for wrong instance: expected {}, got {}",
                subscriber_idx,
                process_instance_id,
                event.process_instance_id
            );
        }
        count += 1;
        metrics.subscription_events.fetch_add(1, Ordering::Relaxed);
    }

    if count == 0 {
        bail!(
            "subscriber {} received no events for {}",
            subscriber_idx,
            process_instance_id
        );
    }
    Ok(count)
}

async fn wait_for_subscription_watchers(
    handles: Vec<tokio::task::JoinHandle<Result<usize>>>,
) -> Result<()> {
    if handles.is_empty() {
        return Ok(());
    }

    let mut total_events = 0usize;
    for handle in handles {
        let count = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .context("timed out waiting for subscription watcher to finish")???;
        total_events += count;
    }
    if total_events == 0 {
        bail!("subscription watchers received no lifecycle events");
    }
    Ok(())
}

async fn assert_service_ready(server_url: &str) -> Result<()> {
    let mut client = connect_client(server_url).await?;
    let health = client.health(HealthRequest {}).await?.into_inner();
    if !health.ready {
        bail!("BPMN-Lite service is not ready: {}", health.status);
    }
    Ok(())
}

async fn assert_service_metrics(server_url: &str) -> Result<()> {
    let mut client = connect_client(server_url).await?;
    let metrics = client.metrics(MetricsRequest {}).await?.into_inner();
    if metrics.requests_total == 0 {
        bail!("BPMN-Lite metrics did not record requests");
    }
    Ok(())
}

async fn compile_fixtures(server_url: &str) -> Result<Vec<CompiledFixture>> {
    let mut client = connect_client(server_url).await?;
    let mut compiled = Vec::with_capacity(FIXTURES.len());
    for fixture in FIXTURES {
        let response = client
            .compile(CompileRequest {
                bpmn_xml: fixture.bpmn_xml.to_string(),
                validate_only: false,
            })
            .await
            .with_context(|| format!("Compile failed for fixture '{}'", fixture.key))?
            .into_inner();
        compiled.push(CompiledFixture {
            fixture: *fixture,
            bytecode_version: response.bytecode_version,
        });
    }
    Ok(compiled)
}

async fn start_instances(
    server_url: &str,
    compiled: &[CompiledFixture],
    instances: &Arc<Mutex<Vec<InstanceTracker>>>,
    metrics: &Arc<Metrics>,
    sequence: &Arc<Sequence>,
    count: usize,
) -> Result<()> {
    let mut client = connect_client(server_url).await?;
    for idx in 0..count {
        let fixture = &compiled[idx % compiled.len()];
        let case_number = sequence.next();
        let payload = json!({
            "workflow_key": fixture.fixture.key,
            "case_id": format!("case-{}", case_number),
            "approved": case_number.is_multiple_of(2),
            "batch_index": idx,
        })
        .to_string();
        let hash = bpmn_lite_vm::compute_hash(&payload);
        let response = client
            .start_process(StartRequest {
                process_key: fixture.fixture.key.to_string(),
                bytecode_version: fixture.bytecode_version.clone(),
                domain_payload: payload,
                domain_payload_hash: hash.to_vec(),
                session_stack_json: String::new(),
                orch_flags: Default::default(),
                correlation_id: format!("{}-{}", fixture.fixture.key, idx),
                entry_id: uuid::Uuid::nil().to_string(),
                runbook_id: uuid::Uuid::nil().to_string(),
                tenant_id: String::new(),
            })
            .await
            .with_context(|| format!("StartProcess failed for fixture '{}'", fixture.fixture.key))?
            .into_inner();

        metrics.instances_started.fetch_add(1, Ordering::Relaxed);
        instances.lock().await.push(InstanceTracker {
            process_instance_id: response.process_instance_id,
            workflow_key: fixture.fixture.key.to_string(),
            signal_messages: fixture
                .fixture
                .signal_messages
                .iter()
                .map(|message| (*message).to_string())
                .collect(),
            terminal_state: None,
        });
    }
    Ok(())
}

async fn worker_loop(config: WorkerLoopConfig) -> Result<()> {
    let WorkerLoopConfig {
        worker_idx,
        server_url,
        task_types,
        metrics,
        shutdown,
        sequence,
        fail_every,
        cancel_after_fail,
        max_jobs,
    } = config;
    let mut client = connect_client(&server_url).await?;
    loop {
        if shutdown.load(Ordering::Relaxed) != 0 {
            break;
        }

        let mut stream = client
            .activate_jobs(ActivateJobsRequest {
                task_types: task_types.clone(),
                max_jobs,
                timeout_ms: 100,
                worker_id: format!("worker-{}", worker_idx),
                tenant_id: String::new(),
            })
            .await?
            .into_inner();

        let mut saw_job = false;
        while let Some(job) = stream.message().await? {
            saw_job = true;
            metrics.job_activations.fetch_add(1, Ordering::Relaxed);
            process_job(
                &mut client,
                job,
                &metrics,
                &sequence,
                fail_every,
                cancel_after_fail,
            )
            .await?;
        }

        if !saw_job {
            sleep(Duration::from_millis(20)).await;
        }
    }
    Ok(())
}

async fn process_job(
    client: &mut BpmnLiteClient<Channel>,
    job: JobActivationMsg,
    metrics: &Arc<Metrics>,
    sequence: &Arc<Sequence>,
    fail_every: usize,
    cancel_after_fail: bool,
) -> Result<()> {
    let turn = sequence.next() as usize;
    if fail_every > 0 && turn.is_multiple_of(fail_every) {
        let process_instance_id = job.process_instance_id.clone();
        client
            .fail_job(FailJobRequest {
                job_key: job.job_key,
                error_class: "TRANSIENT".to_string(),
                message: "synthetic load-harness transient failure".to_string(),
                retry_hint_ms: 250,
                worker_id: job.worker_id,
                claim_token: job.claim_token,
                tenant_id: job.tenant_id.clone(),
            })
            .await?;
        if cancel_after_fail {
            client
                .cancel(CancelRequest {
                    process_instance_id,
                    reason: "synthetic load-harness cancellation after fail_job".to_string(),
                    tenant_id: String::new(),
                })
                .await?;
        }
        metrics.job_failures.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }

    let payload_json = next_payload_for_job(&job, turn)?;
    let orch_flags = next_orch_flags(&job, turn);
    client
        .complete_job(CompleteJobRequest {
            job_key: job.job_key,
            domain_payload: payload_json,
            domain_payload_hash: job.domain_payload_hash,
            orch_flags,
            worker_id: job.worker_id,
            claim_token: job.claim_token,
            tenant_id: job.tenant_id,
        })
        .await?;
    metrics.job_completions.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn next_payload_for_job(job: &JobActivationMsg, turn: usize) -> Result<String> {
    let mut payload = serde_json::from_str::<serde_json::Value>(&job.domain_payload)
        .with_context(|| format!("Job payload is not valid JSON for {}", job.job_key))?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| anyhow!("Job payload must be a JSON object for {}", job.job_key))?;
    object.insert(
        "last_task_type".to_string(),
        serde_json::Value::String(job.task_type.clone()),
    );
    object.insert(
        "worker_turn".to_string(),
        serde_json::Value::from(turn as u64),
    );
    object.insert(
        "approved".to_string(),
        serde_json::Value::Bool((turn & 1) == 0),
    );
    serde_json::to_string(&payload).context("Failed to serialize completion payload")
}

fn next_orch_flags(job: &JobActivationMsg, turn: usize) -> HashMap<String, ProtoValue> {
    let mut flags = HashMap::new();
    if job.task_type == "review_case" {
        flags.insert(
            "approved".to_string(),
            ProtoValue {
                kind: Some(bpmn_lite_server::grpc::proto::proto_value::Kind::BoolValue(
                    (turn & 1) == 0,
                )),
            },
        );
    }
    flags
}

async fn signal_loop(
    server_url: String,
    instances: Arc<Mutex<Vec<InstanceTracker>>>,
    metrics: Arc<Metrics>,
    shutdown: Arc<AtomicUsize>,
    sequence: Arc<Sequence>,
    interval_ms: u64,
    ghost_signal_every: usize,
) -> Result<()> {
    let mut client = connect_client(&server_url).await?;
    loop {
        if shutdown.load(Ordering::Relaxed) != 0 {
            break;
        }

        let snapshot = instances.lock().await.clone();
        let active = snapshot
            .iter()
            .filter(|instance| {
                instance.terminal_state.is_none() && !instance.signal_messages.is_empty()
            })
            .collect::<Vec<_>>();

        if active.is_empty() {
            sleep(Duration::from_millis(interval_ms)).await;
            continue;
        }

        for target in active {
            let turn = sequence.next() as usize;
            let signal_name = if ghost_signal_every > 0 && turn.is_multiple_of(ghost_signal_every) {
                "ghost_signal".to_string()
            } else {
                let fixture = fixture_by_key(&target.workflow_key)
                    .ok_or_else(|| anyhow!("Unknown workflow fixture '{}'", target.workflow_key))?;
                fixture
                    .signal_messages
                    .get(turn % fixture.signal_messages.len())
                    .copied()
                    .unwrap_or("resume_signal")
                    .to_string()
            };
            let payload = format!(
                r#"{{"signal_turn":{},"signal_name":"{}"}}"#,
                turn, signal_name
            );

            client
                .signal(SignalRequest {
                    process_instance_id: target.process_instance_id.clone(),
                    message_name: signal_name.clone(),
                    correlation_key: None,
                    payload: payload.into_bytes(),
                    msg_id: format!("{}-{}", signal_name, turn),
                    tenant_id: String::new(),
                })
                .await?;

            metrics.signals_sent.fetch_add(1, Ordering::Relaxed);
        }

        sleep(Duration::from_millis(interval_ms)).await;
    }
    Ok(())
}

async fn monitor_until_terminal(
    config: &HarnessConfig,
    instances: &Arc<Mutex<Vec<InstanceTracker>>>,
    metrics: &Arc<Metrics>,
    shutdown: &Arc<AtomicUsize>,
    started_at: Instant,
) -> Result<Summary> {
    let semaphore = Arc::new(Semaphore::new(config.inspect_concurrency.max(1)));
    let deadline = started_at + Duration::from_secs(config.timeout_secs);

    loop {
        if Instant::now() > deadline {
            let summary = build_summary(instances).await;
            let active = instances
                .lock()
                .await
                .iter()
                .filter(|instance| instance.terminal_state.is_none())
                .map(|instance| {
                    format!("{}:{}", instance.workflow_key, instance.process_instance_id)
                })
                .take(8)
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "Harness timed out after {}s; terminal_counts={:?}; workflow_counts={:?}; sample_active=[{}]",
                config.timeout_secs,
                summary.terminal_counts,
                summary.workflow_counts,
                active
            );
        }

        let snapshot = instances.lock().await.clone();
        let mut handles = Vec::new();
        for (idx, tracker) in snapshot.iter().enumerate() {
            if tracker.terminal_state.is_some() {
                continue;
            }
            let permit = semaphore.clone().acquire_owned().await?;
            let server_url = config.server_url.clone();
            let process_instance_id = tracker.process_instance_id.clone();
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let mut client = connect_client(&server_url).await?;
                let inspect = client
                    .inspect(InspectRequest {
                        process_instance_id,
                        tenant_id: String::new(),
                    })
                    .await?
                    .into_inner();
                Ok::<(usize, String), anyhow::Error>((idx, inspect.state))
            }));
        }

        if handles.is_empty() {
            shutdown.store(1, Ordering::Relaxed);
            let summary = build_summary(instances).await;
            return Ok(summary);
        }

        let mut updates = Vec::new();
        for handle in handles {
            let (idx, state) = handle.await??;
            metrics.inspections.fetch_add(1, Ordering::Relaxed);
            updates.push((idx, state));
        }

        let mut guard = instances.lock().await;
        for (idx, state) in updates {
            if state != "RUNNING" {
                if let Some(item) = guard.get_mut(idx) {
                    item.terminal_state = Some(state);
                }
            }
        }

        drop(guard);
        sleep(Duration::from_millis(50)).await;
    }
}

async fn build_summary(instances: &Arc<Mutex<Vec<InstanceTracker>>>) -> Summary {
    let guard = instances.lock().await;
    let mut summary = Summary::default();
    for instance in guard.iter() {
        let state = instance
            .terminal_state
            .clone()
            .unwrap_or_else(|| "RUNNING".to_string());
        *summary.terminal_counts.entry(state).or_insert(0) += 1;
        *summary
            .workflow_counts
            .entry(instance.workflow_key.clone())
            .or_insert(0) += 1;
    }
    summary
}

fn emit_summary(
    config: &HarnessConfig,
    metrics: &Arc<Metrics>,
    summary: &Summary,
    elapsed: Duration,
) {
    let jobs_total = metrics.job_activations.load(Ordering::Relaxed);
    let started = metrics.instances_started.load(Ordering::Relaxed);
    let completions = metrics.job_completions.load(Ordering::Relaxed);
    let failures = metrics.job_failures.load(Ordering::Relaxed);
    let signals = metrics.signals_sent.load(Ordering::Relaxed);
    let inspections = metrics.inspections.load(Ordering::Relaxed);
    let subscription_events = metrics.subscription_events.load(Ordering::Relaxed);
    let throughput = if elapsed.as_secs_f64() > 0.0 {
        jobs_total as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    let mut terminal = String::new();
    for (state, count) in &summary.terminal_counts {
        let _ = write!(&mut terminal, "{}={}, ", state, count);
    }
    if terminal.ends_with(", ") {
        terminal.truncate(terminal.len() - 2);
    }

    println!(
        "{}",
        json!({
            "profile": config.profile,
            "server_url": config.server_url,
            "instances_started": started,
            "job_activations": jobs_total,
            "job_completions": completions,
            "job_failures": failures,
            "signals_sent": signals,
            "inspections": inspections,
            "subscription_events": subscription_events,
            "elapsed_ms": elapsed.as_millis(),
            "jobs_per_second": throughput,
            "terminal_counts": summary.terminal_counts,
            "workflow_counts": summary.workflow_counts,
            "terminal_summary": terminal,
        })
    );
}

async fn connect_client(server_url: &str) -> Result<BpmnLiteClient<Channel>> {
    BpmnLiteClient::connect(server_url.to_string())
        .await
        .with_context(|| format!("Failed to connect to BPMN-Lite server at {}", server_url))
}
