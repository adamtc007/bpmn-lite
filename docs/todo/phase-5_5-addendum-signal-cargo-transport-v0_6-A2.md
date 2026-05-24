# Phase 5.5 v0.6 Addendum: Signal / Cargo / Transport Separation

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-ADDENDUM-002 |
| Version | v0.6-A2 |
| Status | ADDENDUM to v0.6 plan; SUPPLANTS v0.6-A1 (LISTEN/NOTIFY) |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Applies to | Phase 5.5 plan v0.6 — affects §4 non-goals, §8.5 outbox sender flow, T2A.2 storage crate, T2B.1 bus client crate, T2B.2 bus server crate |
| Instruction to Sonnet | A2 supplants A1. Disregard A1. Read this addendum alongside v0.6 §8 and the relevant T2 tranches. v0.6 stands as plan of record; this refines the signal mechanism. |

---

## 0. The architectural cut

Three concerns, conflated in most distributed systems, properly separated here:

| Layer | Concern | Mechanism | Failure semantics |
|---|---|---|---|
| **Cargo** | What durably needs to be sent or remembered | Postgres tables (outbox, inbox, pending_invocation) | Crash-safe via WAL; transactional with business state |
| **Signal (in-process)** | "Cargo just appeared, look at the DB" | `tokio::sync::Notify` | Lossy by design; safety-net timer covers |
| **Transport (cross-process)** | Carries the cargo across the network | TCP/IP via gRPC (tonic) | TCP assured delivery; gRPC HTTP/2 framing; retry via outbox on connection-level failure |

Each layer uses the minimum mechanism that achieves its property. No middleware. No "configurable strategy." No framework on top of framework. The architecture is hardware-near.

### TCP/IP is the foundation

This design leverages TCP/IP assured delivery as the substrate. TCP guarantees:

- In-order byte delivery
- Acknowledgement-based reliable transport
- Congestion control
- Connection state across packet loss and retransmission

These are decades-hardened properties built into every Linux kernel. The right architectural posture is to *use them*, not to reimplement them in application code.

gRPC adds:
- HTTP/2 framing (multiplexing, header compression)
- Protobuf serialisation (typed, compact)
- Standard error codes
- Per-call deadlines

That's it. We do not need a message broker on top of TCP because TCP already provides reliable in-order delivery. The outbox layer above gRPC handles the case where the connection itself fails (peer down, network partition) — not the case where individual packets are lost (TCP handles that).

### Where the failure points actually are

| Failure point | Mitigated by | Notes |
|---|---|---|
| Individual packet loss | TCP retransmission | Invisible to application |
| Connection-level failure (peer down, partition) | Outbox + retry with backoff | Persists across reconnect |
| Receiver crash mid-execution | Phase 5 engine recovery + inbox idempotency | Verb re-runs idempotently or errors |
| Caller crash before commit | Nothing committed; no-op recovery | Pure failure |
| Caller crash after commit, before signal | Fallback timer catches | 30s worst case |
| In-process signal lost (channel full, sender stuck) | Fallback timer | 30s worst case |
| Postgres unavailable | Caller and receiver both fail-fast | Same as any DB-backed app |

This is **a small list**. Each item maps to a specific, named mechanism. There are no diffuse "framework might do something unexpected" failure modes because there is no framework above the named primitives.

---

## 1. Mechanism details

### Outbound signal: `tokio::sync::Notify`

The outbox writer (BPMN executor) and the outbox sender (gRPC client) are in the same process per domain. They communicate via an in-memory atomic flag with a wake-list — `tokio::sync::Notify`.

Writer side:
```rust
async fn submit_to_outbox(
    pool: &PgPool,
    entry: NewOutboxEntry,
    notifier: &Arc<Notify>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    insert_outbox(&mut tx, &entry).await?;
    tx.commit().await?;
    
    notifier.notify_one();  // wake sender if waiting; coalesces if not
    Ok(())
}
```

Sender side:
```rust
async fn outbox_sender_loop(
    pool: PgPool,
    notifier: Arc<Notify>,
    grpc_clients: BusGrpcClients,
) -> Result<()> {
    let mut fallback = tokio::time::interval(Duration::from_secs(30));
    
    // Drain on startup — outbox may have entries from before this task started
    drain_outbox(&pool, &grpc_clients).await?;
    
    loop {
        tokio::select! {
            _ = notifier.notified() => {
                drain_outbox(&pool, &grpc_clients).await?;
            }
            _ = fallback.tick() => {
                drain_outbox(&pool, &grpc_clients).await?;
            }
        }
    }
}

async fn drain_outbox(pool: &PgPool, clients: &BusGrpcClients) -> Result<()> {
    loop {
        let entries = sqlx::query_as!(
            OutboxEntry,
            "SELECT * FROM outbox
             WHERE status = 'pending' AND next_attempt_at <= now()
             ORDER BY created_at
             FOR UPDATE SKIP LOCKED
             LIMIT 10"
        ).fetch_all(pool).await?;
        
        if entries.is_empty() {
            return Ok(());
        }
        
        for entry in entries {
            send_one_entry(pool, clients, entry).await?;
        }
        // Loop: more may have arrived while we were sending
    }
}
```

**Why `Notify` specifically:**
- Coalescing semantics: many `notify_one()` calls between waits don't queue; one wake-up serves all of them
- No allocation on the hot path
- Lock-free wake-up in the common case (only takes a lock if there's a waiter to remove from the list)
- Implementation is ~200 lines of Rust in the tokio source; auditable, well-understood

**Why the fallback timer:**
- The signal can be missed (writer crashes between commit and notify; channel state race in extreme cases)
- 30 seconds is operationally invisible (foundational services won't notice; observability dashboards show steady-state)
- It bounds worst-case latency at 30s instead of "never" — a genuine availability guarantee
- It is *not* a polling interval; it's a safety net at a different time scale than the signal

### Inbound: TCP via gRPC; no separate signal

The inbound side does not need a signal mechanism. The TCP packet *is* the signal. The kernel's epoll/io_uring layer wakes the listening process; tonic decodes the HTTP/2 frame and protobuf payload; the handler runs.

```rust
// In bpmn-lite's gRPC server (ResultService for receiving results from peers)
async fn deliver_result(
    &self,
    request: Request<InvocationResult>,
) -> Result<Response<ResultAck>, Status> {
    let result = request.into_inner();
    
    // Idempotency check
    let already = self.inbox.lookup(&result.idempotency_key).await?;
    if already.is_some() {
        return Ok(Response::new(ResultAck {
            status: ReceiptStatus::DuplicateIgnored as i32,
            detail: String::new(),
        }));
    }
    
    // Atomic: record inbox + clear pending + advance process
    let mut tx = self.pool.begin().await?;
    self.inbox.record(&mut tx, &result).await?;
    let pending = self.pending.complete(&mut tx, &result.execution_id).await?;
    tx.commit().await?;
    
    if let Some(p) = pending {
        self.executor.advance(p.process_instance_id, result.outcome).await?;
    }
    
    Ok(Response::new(ResultAck {
        status: ReceiptStatus::Received as i32,
        detail: String::new(),
    }))
}
```

**The signal is the syscall return.** epoll_wait returns; tokio's reactor wakes the task; tonic dispatches to the handler; the handler runs. There is no additional signal layer because there is no additional process boundary to cross.

This is the natural extension of "use the OS for what it's good at." The OS already provides event-driven notification for network sockets. We use it.

---

## 2. Implementation specifics

### Crate ownership

- `dsl-bus-storage` — owns outbox/inbox/pending schemas and CRUD. **No notification concerns.** Pure cargo operations.
- `dsl-bus-client` — owns the outbox sender task and the `OutboxNotifier`. Writer crates depend on `dsl-bus-client` to get a notifier handle to pair with their outbox writes.
- `dsl-bus-server` — owns the inbound gRPC dispatch. Receives `InvocationRequest` and `InvocationResult`; dispatches to consumer-provided callbacks. No signalling layer of its own.

### OutboxNotifier API

In `dsl-bus-client`:

```rust
pub struct OutboxNotifier {
    inner: Arc<Notify>,
}

impl OutboxNotifier {
    pub(crate) fn new() -> (Self, Arc<Notify>) {
        let inner = Arc::new(Notify::new());
        (Self { inner: inner.clone() }, inner)
    }
    
    pub fn notify(&self) {
        self.inner.notify_one();
    }
}
```

The writer (in `bpmn-lite-runtime` or `ob-poc-bus-handler` etc.) calls `notifier.notify()` after `tx.commit()`. The sender task (owned by `dsl-bus-client`) holds the shared `Arc<Notify>` and waits on it.

### What NOT to do

The discipline points that protect this architecture from creeping into a Java-style mess:

1. **Don't wrap `tokio::sync::Notify` in a trait.** It is the abstraction. Wrapping it so it's "swappable" is the Java mistake — you don't need to swap it because there's nothing to swap it for. If signalling ever needs to cross process boundaries, that's a *different mechanism* (the outbound bus call itself), not a parameterised version of this one.

2. **Don't add middleware/interceptors/hooks to the signal path.** The path is `notify() → notified() → drain`. Three steps. No logging hooks, no metrics interceptors, no "extension points." Metrics live in `drain_outbox` (count entries drained, timing of sends); they do not live in the signal layer.

3. **Don't add a "notification queue" inside the sender.** Notifications are stateless wake-up signals; multiple notify_one() calls coalesce into a single wake-up; the database table is the queue. There is no second queue in memory.

4. **Don't make the 30-second fallback configurable.** It's a safety net at a fixed time scale. Operators do not need to tune it. Tuning it casually invites the wrong mental model ("polling is fast" — no, polling is the fallback). Keep it as a `const FALLBACK_TIMER_SECS: u64 = 30;` named clearly for what it is.

5. **Don't add a generic "event bus" abstraction.** This architecture has exactly three communication primitives — in-process Notify, durable Postgres tables, and gRPC over TCP. Each is named, each is used directly. A generic "event bus" library on top adds layers without earning them.

6. **Don't add LISTEN/NOTIFY "for symmetry" or "as a future option."** It's not in the architecture. Adding it later for a real reason is fine; adding it now because "what if someone wants it" is YAGNI applied wrong.

---

## 3. v0.6 plan changes

### §4 non-goals

**Remove:** "LISTEN/NOTIFY for outbox wake-up (polling sender for demo; LISTEN/NOTIFY deferred)"

**Add:** "Postgres LISTEN/NOTIFY for outbox wake-up — in-process `tokio::sync::Notify` covers all cases needed; LISTEN/NOTIFY would be additional infrastructure with no clear earned purpose."

**Add:** "Configurable signal mechanism — `tokio::sync::Notify` is the chosen primitive; no swappable abstraction over it."

### §8.5 outbox sender flow

Replace the polling pseudocode (and the v0.6-A1 LISTEN/NOTIFY pseudocode) with the §1 pseudocode above.

### T2A.2 (dsl-bus-storage)

**No NOTIFY in INSERT statements.** The crate owns cargo only. Writers signal via `OutboxNotifier` after the commit completes.

CRUD API unchanged from v0.6 §8.5. Specifically:

- `insert_outbox(tx, entry)` — INSERT only; no NOTIFY
- `select_pending_outbox(limit)` — unchanged
- `mark_outbox_submitted(id, execution_id)` — unchanged
- `mark_outbox_retry(id, backoff_secs, error)` — unchanged

Tests: standard CRUD tests; no notification-related tests in this crate.

### T2B.1 (dsl-bus-client)

**Add:** `OutboxNotifier` API per §2 above.

**Sender task:** uses `tokio::select!` on `notifier.notified()` and the 30-second fallback timer.

**Tests:**
- `notification_drives_drain` — writer commits + calls notify; sender wakes within ~1ms (allow 10ms in test); drains the new entry
- `fallback_timer_drains_missed_signal` — insert outbox row WITHOUT calling notify; sender drains within 30s + tolerance
- `burst_coalescing` — 20 INSERT+notify in rapid succession; sender drains all 20 (single wake-up serves multiple notifications via Notify's coalescing)
- `sender_isolation_from_writer_failure` — writer task panics after commit but before notify; fallback timer covers; sender drains anyway

### T2B.2 (dsl-bus-server)

**No signalling layer.** Inbound gRPC handlers dispatch directly. Tests verify handler invocation on incoming gRPC calls (standard tonic testing patterns).

### §12 Risk register

**Update R5:**

R5 now reads:
> "Signal/cargo separation risk: in-process notification could be missed (writer crash between commit and notify; channel race under load).
> 
> **Primary mitigation:** at-least-once delivery via persistent outbox + idempotent inbox. Cargo is durable; signal loss is recoverable.
> 
> **Signal-specific mitigation:** 30-second fallback timer drains the outbox unconditionally regardless of signal arrival. Worst-case latency for a missed signal is bounded.
> 
> **TCP/IP reliance:** application-level retry covers connection-level failure (peer down, partition); TCP itself handles packet loss invisibly. Failure modes are enumerable."

---

## 4. Performance characteristics

Latency budget per cross-domain callout:

| Step | Time |
|---|---|
| Caller submit (transaction commit) | ~3ms |
| Notify → sender wake-up | <50μs (tokio Notify, microsecond-scale) |
| Drain query (SELECT) | ~0.5ms |
| gRPC send (TCP local) | ~0.5ms |
| Receiver gRPC dispatch | ~50μs (epoll wake + tonic decode) |
| Receiver inbox + verb execution | ~8ms (verb-dependent) |
| Receiver outbox commit (result) | ~3ms |
| Receiver notify → sender wake-up | <50μs |
| gRPC return (TCP local) | ~0.5ms |
| Caller inbox + advance | ~3ms |

**Total per callout: ~19ms.** Verb execution dominates; framework/signalling/transport overhead is <2ms total.

Demo workflow (4 callouts): **~76ms end-to-end.**

Idle cost:
- Sender task: parked on Notify; zero CPU, zero queries
- Receiver: parked on epoll; zero CPU, zero queries
- Postgres: serving real work only; no notification overhead

This is what "performance is a first-class non-functional deliverable" looks like in practice. Every microsecond of overhead is accounted for. The dominant cost is the work itself, not the framework.

---

## 5. Why this is the architecture

The system uses:

1. **TCP/IP** — for assured byte delivery between hosts. Decades-hardened; the foundation of all reliable inter-system communication. Used directly via standard sockets.

2. **HTTP/2 + gRPC** — for typed remote procedure call semantics over TCP. Multiplexed connection; protobuf framing; standard error codes. Used as the wire format.

3. **`tokio::sync::Notify`** — for in-process wake-up between the writer task and the sender task. Atomic flag with wake-list. Used as the signal primitive.

4. **Postgres + WAL** — for durable cargo storage with transactional integrity. The outbox table records what to send; the inbox table records what was received; the pending table records what's in flight. Used as the durability substrate.

5. **`tokio::time::interval`** — for the 30-second fallback timer. Trivial; covers the rare case where the signal is lost.

That's the whole list. Five primitives, each tied directly to a hardware or OS-level mechanism, each performing exactly one function. No framework, no middleware, no "convenient abstraction layer."

Performance follows from the architecture. The system is fast because nothing in the path is doing work that isn't earning its place.

End of addendum v0.6-A2.
