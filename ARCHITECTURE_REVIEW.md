# `bpmn-lite` Architecture Review

## Section 1: Executive Summary & System Overview

### Assessment

`bpmn-lite` is not production-ready as a durable workflow orchestrator. The implementation contains useful compiler, VM, persistence, and tenancy building blocks, but its core durability contract is not closed: several externally visible transitions span multiple database commits; ordinary timers do not resume; leases are not fenced; the DSL bus acknowledgement path is incomplete; and the event stream cannot reconstruct all state after a crash. These are correctness defects, not optimization opportunities.

The repository currently contains two workflow runtimes behind one engine facade:

1. BPMN XML is parsed into an IR graph, verified, lowered to `CompiledProgram`, and executed by a bytecode VM using persisted fibers.
2. The BPMN-like DSL is compiled to `WorkflowExecutionPlan` and executed by `PlanWalker`, using a current-node cursor and placeholder map instead of VM fibers.

`Engine::tick_instance_inner` selects the runtime from `ProcessInstance.plan_hash`. The two paths do not share transition semantics, start transactions, wait handling, event coverage, recovery checks, or concurrency behavior. This is the dominant architectural defect. A deterministic engine cannot have two independently evolving definitions of process execution and still claim one recovery model.

### Structural strengths that are worth retaining

- The XML compiler has recognizable parse, graph, verification, and lowering stages rather than interpreting XML directly.
- VM state is explicit: program counter, stack, registers, wait state, and process payload can be persisted rather than depending on a Rust call stack.
- `commit_tick`, `atomic_start`, deduplication records, scheduler leases, tenant-scoped transactions, RLS migrations, event sequencing, pending invocations, and an outbox show awareness of durable-execution requirements.
- The codebase contains tests for joins, races, boundary timers, messages, integrity, RLS, and transaction rollback. These are useful assets once they are turned into mandatory production gates.
- The runtime contains no discovered `unsafe` blocks. The problem is semantic durability, not Rust memory unsafety.

These strengths do not compensate for missing end-to-end invariants. Most existing atomic operations cover only a subset of the transition they are named after.

### Primary production risks

| Risk | Consequence | Production severity |
|---|---|---|
| Ordinary timer waits are never promoted to runnable | Workflows using `WaitFor` or `WaitUntil` remain parked forever | Critical |
| VM and `PlanWalker` implement different semantics | Recovery, audit, and behavioral equivalence depend on which frontend created the instance | Critical |
| FFI and job completion cross transaction boundaries | A crash can duplicate an external side effect or persist a resumed fiber without committing its result | Critical |
| Bus submission acknowledgements do not update pending invocation state | Plan instances remain in `WaitingOnSubmission`; results cannot find the NULL execution ID | Critical |
| Optimistic conflict and lock timeout are treated as successful results | The workflow advances and pending work is deleted even though the operation did not commit | Critical |
| Leases have owner strings but no fencing generation | An expired owner can still reach unfenced mutation methods after a replacement owner starts | Critical |
| Artifact identity excludes executable metadata and verification is shallow | Invalid or semantically different programs can share an artifact key and enter the VM | Critical |
| Snapshots and journal records have no explicit schema/ABI envelope | Rolling upgrades can make persisted state unreadable or change replay meaning | Critical |
| Network dispatch happens while holding outbox row locks and a DB transaction | Slow peers consume pool capacity, hold locks, and serialize throughput | Major |
| Tenant is omitted from primary instance lookup | Many hot reads scan tenants and open a transaction per tenant | Major |
| Demo REST API starts unauthenticated on all interfaces | A production process exposes mutable demonstration endpoints unless explicitly overridden | Major |
| Most PostgreSQL/bus tests are ignored and CI only checks layering | Critical recovery and transport behavior is not continuously verified | Major |

### Readiness verdict

The engine is suitable for a prototype or controlled single-process experiment. It should not be used for money movement, regulated workflows, long-running processes, or operations whose side effects cannot be safely repeated. Adding replicas or increasing scheduler concurrency would amplify current correctness failures rather than merely expose performance limits.

The correct target is one pure, deterministic transition kernel, one versioned executable format, and one transactionally fenced commit protocol. XML and DSL should remain authoring frontends; neither should own a separate runtime.

## Section 2: As-Is Implementation Architecture

### Component architecture

#### Types and executable model

`bpmn-lite-types` defines bytecode instructions, values, process state, fibers, wait states, events, jobs, incidents, compiled artifacts, FFI declarations, and integrity helpers. `CompiledProgram` is a public struct whose fields are all public. That lets any caller construct or mutate an artifact without going through the compiler verifier. Process state combines durable engine state with DSL-plan fields such as `plan_hash`, `current_node_id`, and placeholders, which is evidence that two execution models were merged at the storage type rather than unified in the execution semantics.

#### XML compiler path

`bpmn-lite-compiler` parses BPMN XML, constructs a Petgraph-based IR, runs graph-level checks, and lowers to VM bytecode and side tables. Lowering produces the instruction vector plus debug, join, wait, message, race, boundary, write-set, symbol, data-object, error-route, and FFI metadata.

The final bytecode verifier in `bpmn-lite-compiler/src/verifier.rs::verify_bytecode` checks control-flow targets and rejects most backward branches. It does not perform abstract interpretation of stack height or register use, prove termination bounds, validate that every side-table address and identifier agrees with an instruction, or calculate a safe maximum frame size.

#### DSL compiler and plan runtime

The DSL path produces `WorkflowExecutionPlan`. `bpmn-lite-engine/src/plan_walker.rs` walks nodes by identifier, mutates placeholders, and dispatches callouts through the bus. It is not a second lowering path into VM bytecode. Joins are effectively cursor transitions rather than token barriers, split selection is order-dependent, and part of routing is coupled to a domain string (`cbu_type_routing`). Plan transitions also do not emit the complete runtime journal needed for replay.

#### VM

`bpmn-lite-vm` interprets `Instr` values. A `Fiber` contains a bytecode program counter, operand stack, fixed register set, and `WaitState`. `run_fiber` executes up to a step limit and returns when it parks, ends, terminates, or requests FFI. VM instructions append `TickOperation` values to a `TransactionContext` for some paths, but other VM methods directly call store methods. This mixed mutation model is the source of several torn transitions.

The token model is therefore a set of persisted fibers:

- `Running` fibers are eligible to execute.
- `Job`, `Message`, `Join`, `Race`, `Timer`, and `Incident` fibers are parked.
- forks create multiple fibers;
- joins use separately persisted join-barrier counts;
- race and boundary-timer logic is partly in the engine after the main VM commit.

There is no single persisted `next_due_at` index for parked fibers. The scheduler claims instances in `Running` process state and the engine ignores every fiber whose wait state is not `Running`. It later performs a special pass for `Race`, but no equivalent pass exists for an ordinary `Timer`.

#### Engine and scheduler

`bpmn-lite-engine` is both application service and transaction coordinator. It starts instances, claims transitions, loads programs and fibers, invokes the VM, dispatches FFI, completes/fails jobs, delivers messages, handles cancellation/incidents, and runs post-tick boundary/race passes. The main module is consequently large and has multiple paths that mutate the same state with different atomicity guarantees.

The server scheduler enumerates tenants, claims a bounded batch, and ticks claimed instances sequentially per tenant. A slow in-process or remote FFI call occurs under the transition lease and blocks the rest of that batch. The engine repeatedly reloads the immutable program and reloads the full fiber collection for the main pass, boundary promotion, and race evaluation.

#### Persistence

`bpmn-lite-store::ProcessStore` is a large asynchronous trait spanning snapshots, fibers, joins, jobs, deduplication, messages, events, artifacts, templates, incidents, leases, tenants, integrity, plans, pending invocations, and outbox mutations. The hot runtime uses `Arc<dyn ProcessStore>`, `async_trait`, and `anyhow::Result`. Database latency dwarfs the virtual-call cost today, but the abstraction prevents a synchronous/WASM kernel and erases errors that the scheduler must classify precisely.

`bpmn-lite-store-postgres` implements the trait in a monolithic store with 46 migrations. Canonical VM state is stored in `process_instances` and `fibers`, while DSL bus support also defines `bpmn_process_instance` and a separate `BpmnProcessInstanceStore`. The duplicate model is not a read projection; code writes and reads it as another source of process state.

Many methods accept only `instance_id`. PostgreSQL then calls `resolve_tenant_id`, enumerates tenants, and opens tenant-scoped transactions until it finds the row. This turns a primary-key lookup into O(number of tenants) database work and makes tenant identity an inferred property despite the engine already having it.

#### Transport and FFI

Native jobs are persisted in a queue and activated by workers. The DSL bus uses a pending-invocation row and outbox. The outbox sender begins a transaction, locks entries, connects to a peer, sends gRPC, and updates the row before committing. A successful submission updates only the outbox row; it does not call `PendingInvocationStore::record_ack` or move the owning process from `WaitingOnSubmission` to `WaitingOnInvocation`.

FFI is executed directly from `Engine::handle_ffi_dispatch`. A pending event is appended, the dispatcher is invoked, a completion event is appended, and the fiber/instance/incident is saved using separate calls. The `ffi_invocation_record` table exists, but the runtime dispatch path does not persist its state machine.

### Workflow lifecycle diagram

```mermaid
sequenceDiagram
    autonumber
    actor Client
    participant Compiler
    participant ArtifactStore
    participant Engine
    participant Scheduler
    participant Store
    participant VM
    participant PlanWalker
    participant Outbox
    participant Worker

    alt BPMN XML path
        Client->>Compiler: BPMN XML
        Compiler->>Compiler: Parse -> IRGraph -> verify -> lower
        Compiler->>ArtifactStore: store CompiledProgram by instruction hash
        Client->>Engine: start(bytecode_version, tenant, payload)
        Engine->>Store: atomic_start(instance + root fiber + event)
    else DSL plan path
        Client->>Compiler: DSL
        Compiler->>ArtifactStore: store WorkflowExecutionPlan
        Client->>PlanWalker: start(plan_hash, variables)
        PlanWalker->>Store: save_instance only
        Note over PlanWalker,Store: No root fiber and different start/event contract
    end

    loop scheduler poll
        Scheduler->>Store: list tenants / claim Running instances
        Store-->>Scheduler: instance IDs + owner lease
        Scheduler->>Engine: tick one instance
        Engine->>Store: load instance
        alt plan_hash is absent
            Engine->>Store: load CompiledProgram + fibers
            Engine->>VM: run Running fibers
            VM-->>Engine: operations / park / end / FFI
            Engine->>Store: commit_tick(snapshot, fibers, events, jobs)
            opt FFI instruction
                Engine->>Worker: direct dispatch after Pending event
                Worker-->>Engine: result
                Engine->>Store: separate event/fiber/instance writes
            end
            opt ordinary Timer wait
                Note over Engine,Store: Fiber remains Timer; no wake-up path exists
            end
        else plan_hash is present
            Engine->>PlanWalker: advance current node
            PlanWalker->>Store: atomic pending + outbox + WaitingOnSubmission
            Outbox->>Worker: submit while DB transaction remains open
            Worker-->>Outbox: SubmissionAck(execution_id)
            Outbox->>Store: mark outbox submitted only
            Note over Outbox,Store: Pending execution_id and process state are not advanced
        end
    end

    opt process crash / restart
        Scheduler->>Store: scan Running instances
        Store-->>Scheduler: VM and plan instances
        Scheduler->>Store: inspect fibers/program/events
        Note over Scheduler,Store: Scanner assumes VM shape and only scans default tenant for interrupted FFI
    end
```

### State management model

| State | In-memory form | Persisted form | Recovery limitation |
|---|---|---|---|
| Executable | `CompiledProgram` or `WorkflowExecutionPlan` | JSONB/text keyed by a hash | Hash does not cover all `CompiledProgram` metadata; two executable models |
| Process variables | domain JSON string, orchestration flags, plan placeholders | process-instance columns and JSONB | Full values cloned and rewritten; no explicit envelope version |
| Control position | fiber `pc` for VM; `current_node_id` for plan | `fibers` JSONB fields or instance plan columns | Recovery logic assumes VM fibers and program |
| Operand state | `Vec<Value>` stack and registers per fiber | serialized JSONB | No verified maximum or compact binary encoding |
| Parallel tokens | multiple fibers plus join barriers | `fibers` and join tables | Some race/join updates occur outside the main tick transaction |
| Waits | `WaitState` enum | fiber JSONB plus jobs/messages/pending rows | ordinary timers have no due-work promotion; plan waits use another protocol |
| External effects | job activation, direct FFI call, DSL outbox | queue, events, pending/outbox tables | Direct FFI is not transactionally coupled; bus ack state machine is incomplete |
| Audit | `RuntimeEvent` | append-only event log with sequence table | Not every state mutation has an event; not sufficient for deterministic replay |
| Concurrency | transition owner and expiry | lease columns | No monotonic fencing token or snapshot revision CAS |

Persistence is snapshot-oriented, not event-sourced. The event log is useful audit data but cannot reproduce plan transitions, all fiber mutations, generated IDs, wall-clock decisions, or external-effect state. Recovery therefore depends on every snapshot and auxiliary table having committed consistently. That condition is not currently met.

## Section 3: Architectural Audit & Opportunities for Improvement

### Critical findings

#### C1. Two runtime semantics invalidate deterministic execution

**Evidence:** `bpmn-lite-engine/src/engine.rs::tick_instance_inner` checks `plan_hash` and returns from the `PlanWalker` branch before loading VM fibers. Plan start saves a process instance without the atomic root-fiber/event contract used by bytecode start. Recovery checks missing program/fibers/start events as VM corruption even though those are normal for plan instances.

This is not a clean strategy abstraction. It is two state machines sharing a table and facade. BPMN parallelism, joins, split semantics, loops, retries, event coverage, and restart behavior depend on which compiler frontend was used.

**Current Pattern**

```rust
if let Some(inst) = self.store.load_instance(instance_id).await? {
    if inst.plan_hash.is_some() {
        PlanWalker::new(...).advance(instance_id, owner).await?;
        return Ok(());
    }
}
// Separate VM lifecycle begins here.
```

**Proposed Hardened Pattern**

```rust
pub trait WorkflowFrontend {
    fn lower(&self, source: &[u8]) -> Result<VerifiedWorkflow, CompileError>;
}

pub struct VerifiedWorkflow(ExecutableWorkflow); // private constructor

pub fn transition(
    workflow: &VerifiedWorkflow,
    snapshot: &Snapshot,
    command: Command,
    context: DeterministicContext,
) -> Result<Transition, TransitionError> {
    kernel::apply(workflow, snapshot, command, context)
}
```

XML and DSL must lower into the same artifact and enter the same transition function. Remove `plan_hash` as a runtime discriminator after migration.

#### C2. Ordinary timer waits never resume

**Evidence:** `Instr::WaitFor` and `Instr::WaitUntil` set `WaitState::Timer` and advance the PC. `Engine::tick_instance_inner` skips every fiber whose wait is not `Running`. The later timer scan handles only `WaitState::Race`. There is no code that changes an ordinary timer fiber back to `Running`.

**Current Pattern**

```rust
fiber.wait = WaitState::Timer { deadline_ms: deadline };
fiber.pc += 1;

// Later, every tick:
if fiber.wait != WaitState::Running {
    continue;
}
```

**Proposed Hardened Pattern**

```rust
// Persisted as part of the same transition as parking the fiber.
effects.push(DurableEffect::ScheduleTimer {
    timer_id: EffectId::for_instruction(instance, fiber, pc),
    instance_id: instance.id,
    fiber_id: fiber.id,
    due_at: context.logical_time + duration,
});

// Due timer consumption and fiber promotion are one fenced transaction.
Command::TimerFired { timer_id, fired_at } => {
    snapshot.consume_timer(timer_id)?;
    snapshot.resume_fiber(timer.fiber_id, fired_at)?;
    events.push(Event::TimerFired { timer_id, fired_at });
}
```

The database needs a `(tenant_id, due_at)` index and unique `timer_id`; polling all running instances is not an acceptable timer wheel.

#### C3. Bus submission and result state machine is incomplete

**Evidence:** `PlanWalker` persists the pending invocation, outbox record, and `WaitingOnSubmission`. `dsl-bus-client/src/sender.rs::dispatch_invocation` handles a successful `SubmissionAck` by calling only `mark_outbox_submitted`. It does not persist the execution ID into the pending row or transition the process to `WaitingOnInvocation`. Result advancement searches pending work by execution ID, so a NULL execution ID produces a successful no-op and the process remains stuck.

**Current Pattern**

```rust
match from_proto_opt(&ack.execution_id) {
    Ok(Some(exec_id)) => {
        mark_outbox_submitted(&mut **tx, entry.id, exec_id).await?;
    }
    // pending invocation and process state are untouched
}
```

**Proposed Hardened Pattern**

```sql
BEGIN;
UPDATE workflow_outbox
   SET state = 'submitted', execution_id = $execution_id
 WHERE tenant_id = $tenant AND effect_id = $effect_id
   AND state = 'dispatching';

UPDATE pending_effects
   SET state = 'accepted', execution_id = $execution_id
 WHERE tenant_id = $tenant AND effect_id = $effect_id
   AND state = 'awaiting_submission';

UPDATE workflow_instances
   SET state = 'waiting_on_effect', revision = revision + 1
 WHERE tenant_id = $tenant AND instance_id = $instance
   AND revision = $expected_revision AND fence = $fence;
COMMIT;
```

All three row changes must succeed or none may commit. Duplicate acknowledgements must return the stored execution ID and leave the revision unchanged.

#### C4. Conflict and lock-timeout results are treated as success

**Evidence:** `bpmn-lite-server/src/bus_runtime.rs` sets `is_transient` for `OptimisticConflict` and `LockTimeout`, then enters `if is_success || is_transient`, binds the response, advances the plan node, sets the instance to `Running`, and removes the pending invocation.

This converts “the remote operation did not commit” into “workflow step completed.” It is direct data loss.

**Current Pattern**

```rust
let is_transient = matches!(outcome,
    OptimisticConflict | LockTimeout);

if is_success || is_transient {
    advance_node_and_bind(&mut instance, node, &input);
    instance.state = ProcessState::Running;
}
```

**Proposed Hardened Pattern**

```rust
match outcome {
    Committed | IdempotentReplayReturned => {
        state.apply_outputs(validated_outputs)?;
        state.complete_effect(effect_id)?;
    }
    OptimisticConflict | LockTimeout => {
        state.schedule_retry(effect_id, retry_policy.next(attempt)?)?;
        events.push(Event::EffectRetryScheduled { effect_id, attempt });
    }
    Rejected(reason) => state.fail_effect(effect_id, reason)?,
}
```

Transient results retain the pending effect, increment a durable attempt counter, and never bind outputs.

#### C5. Direct FFI has no crash-safe effect protocol

**Evidence:** `Engine::handle_ffi_dispatch` independently appends a pending event, performs the external/in-process dispatch, appends a completed event, changes the fiber, saves incidents, and saves the instance. A crash after the side effect but before completion persistence causes re-execution. The runtime does not use `ffi_invocation_record`, although the schema and event comments imply it should.

No local transaction can make an arbitrary HTTP/gRPC/in-process side effect exactly once. The engine must make the request durable before dispatch and require an idempotency key at the effect boundary.

**Current Pattern**

```rust
store.append_event(FfiInvocationPending { invocation_id, ... }).await?;
let result = dispatcher.dispatch(call).await; // external side effect
store.append_event(FfiInvocationCompleted { invocation_id, ... }).await?;
fiber.pc += 1;
store.save_fiber(instance_id, fiber).await?;
```

**Proposed Hardened Pattern**

```rust
// Kernel: no I/O.
transition.effects.push(DurableEffect::Invoke {
    effect_id: EffectId::derive(instance_id, revision, ordinal),
    operation,
    input,
    idempotency_key,
});

// Store adapter: snapshot + event + outbox in one commit.
store.commit_transition(claim, transition).await?;

// Dispatcher: outside the transition transaction.
let response = owner.invoke(effect.clone()).await;
store.record_inbox(effect.effect_id, response).await?; // unique effect_id
```

Only pure, explicitly side-effect-free functions may run synchronously inside the kernel.

#### C6. Job completion persists the resumed fiber before atomic completion

**Evidence:** `Vm::complete_job` changes the fiber to `Running` and immediately calls `save_fiber`. Only after it returns does `Engine::complete_job_inner` apply the payload and call `atomic_complete` to write dedupe, snapshot, payload history, events, and job acknowledgement. Race resolution similarly emits events and saves its fiber before `atomic_complete`.

A crash or failed `atomic_complete` leaves a running fiber with the old process payload and an unacknowledged job. A redelivery may then see no parked fiber and be classified as a ghost signal.

**Current Pattern**

```rust
// VM
fiber.pc += 1;
fiber.wait = WaitState::Running;
store.save_fiber(instance_id, &fiber).await?;

// Engine, later
apply_completion(&mut instance, &completion);
store.atomic_complete(tenant, owner, &instance, &completion, &events).await?;
```

**Proposed Hardened Pattern**

```rust
let transition = kernel::apply(
    &artifact,
    &snapshot,
    Command::EffectCompleted { effect_id, output },
    context,
)?;

store.commit_transition(
    Claim { tenant, instance_id, expected_revision, fence },
    transition, // includes fiber, payload, inbox dedupe, events, and job ack
).await?;
```

The VM must return mutations; it must not write storage from instruction helpers.

#### C7. Cancellation, failure, races, and child creation contain torn transitions

**Evidence:** cancellation appends wait-cancel events, cancels jobs, updates state, deletes fibers, and appends the terminal event through separate store calls. Incident creation saves the incident, appends an event, saves the fiber, and later saves the process. Boundary-race promotion and firing save fibers and append events independently. Child process creation, parent correlation, and parent wait state are not one atomic unit.

The invariant “snapshot state and journal describe the same completed transition” can be violated at every boundary.

**Current Pattern**

```rust
store.save_incident(&incident).await?;
store.append_event(IncidentCreated { ... }).await?;
store.save_fiber(instance_id, &fiber).await?;
store.save_instance(owner, &instance).await?;
```

**Proposed Hardened Pattern**

```rust
pub struct Transition {
    pub next_snapshot: Snapshot,
    pub events: Vec<EventEnvelope>,
    pub effects: Vec<DurableEffect>,
    pub child_starts: Vec<ChildStart>,
    pub terminal_cleanup: CleanupSet,
}

// A single SQL transaction validates revision/fence and applies every field.
store.commit_transition(claim, transition).await?;
```

Child start requires one transaction covering the child snapshot/start event, parent wait, correlation record, and idempotency key. Cross-database child creation would require a saga/outbox rather than pretending to be atomic.

#### C8. Owner leases are not fencing tokens

**Evidence:** transition claims use an owner string and expiry. Several direct methods—including fiber saves and event appends—do not take the lease owner. A slow worker can pass its lease expiry, another worker can acquire the instance, and the stale worker can still reach an unfenced method. `commit_tick` performs an owner check, but not every mutation goes through `commit_tick`.

**Current Pattern**

```rust
claim_instance_for_transition(tenant, id, owner, lease_ms).await?;
// Slow dispatch; lease may expire.
store.save_fiber(id, &fiber).await?; // no owner or generation
```

**Proposed Hardened Pattern**

```rust
pub struct Claim {
    tenant_id: TenantId,
    instance_id: InstanceId,
    expected_revision: u64,
    fence: u64,
}

UPDATE workflow_instances
   SET snapshot = $snapshot,
       revision = revision + 1
 WHERE tenant_id = $tenant
   AND instance_id = $instance
   AND revision = $expected_revision
   AND fence = $fence;
```

Every write must be reachable only through a fenced transition commit. Lease acquisition increments `fence`; renewal never changes it.

#### C9. Artifact identity and verification do not protect the VM

**Evidence:** the compiler version is described as the BLAKE3 hash of the serialized instruction program, while `CompiledProgram` contains executable side tables beyond the instruction vector. PostgreSQL inserts artifacts with `ON CONFLICT (bytecode_version) DO NOTHING`. A metadata-different artifact can collide at the application identity level and silently retain the first row. All artifact fields are public, and `verify_bytecode` checks targets/backward jumps but not stack/register or metadata invariants.

**Current Pattern**

```rust
pub struct CompiledProgram {
    pub bytecode_version: [u8; 32],
    pub program: Vec<Instr>,
    pub race_plan: BTreeMap<RaceId, RacePlanEntry>,
    pub ffi_task_decls: BTreeMap<Addr, FfiTaskDecl>,
    // ...all mutable by callers
}
```

**Proposed Hardened Pattern**

```rust
#[derive(Serialize)]
struct ArtifactEnvelope {
    abi_version: ArtifactAbi,
    compiler_version: CompilerVersion,
    instructions: Vec<Instr>,
    metadata: ExecutableMetadata,
    limits: VerifiedLimits,
}

pub struct ExecutableWorkflow {
    envelope: ArtifactEnvelope,
    hash: ArtifactHash,
}

impl ExecutableWorkflow {
    pub fn verify(bytes: &[u8]) -> Result<Self, VerificationError>;
}
```

Hash canonical bytes of the complete envelope. The verifier must perform CFG dataflow for stack height/types, register bounds, termination/loop bounds, side-table referential integrity, valid entry/end states, and declared resource maxima. On artifact-key conflict, compare bytes and fail on mismatch.

#### C10. Recovery is snapshot repair, not deterministic replay

**Evidence:** the runtime reads wall clock and creates UUIDv7 values during execution. Plan transitions are not fully journaled. Multiple paths mutate fibers or state without a corresponding complete event. Serialized enums and structs lack explicit schema envelopes. Startup FFI interruption detection scans event history only for the default tenant and recovery errors are logged without preventing readiness.

The current event log cannot reproduce a snapshot or prove that replay reaches the same state.

**Current Pattern**

```rust
let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
let incident_id = Uuid::now_v7();
store.save_fiber(instance_id, &fiber).await?;
```

**Proposed Hardened Pattern**

```rust
pub struct DeterministicContext {
    pub logical_time: Timestamp,
    pub command_id: CommandId,
    pub next_revision: u64,
}

impl DeterministicContext {
    pub fn derived_id(&self, ordinal: u32) -> DurableId {
        DurableId::derive(self.command_id, self.next_revision, ordinal)
    }
}

pub struct SnapshotEnvelope {
    pub schema_version: SnapshotSchema,
    pub artifact_abi: ArtifactAbi,
    pub revision: u64,
    pub state: Snapshot,
}
```

Commands supply logical time and external results. The committed journal records the command identity, prior/new revision, generated effect IDs, and result events. Recovery loads a versioned snapshot and replays the tail; an offline verifier replays from genesis or a checkpoint and compares hashes.

### Major findings

#### M1. Lineage and spawn idempotency are not committed with canonical state

**Evidence:** the bus spawn handler validates `entry_id` and `runbook_id`, but `spawn_process_with_idempotency` receives them as `_entry_id` and `_runbook_id`; `PlanWalker::start_process` persists nil lineage. Canonical instance creation can commit before the separate mirror/idempotency transaction. Plan start also stores a zero-filled payload hash for `{}` rather than its BLAKE3 value.

**Current Pattern**

```rust
async fn spawn_process_with_idempotency(
    _entry_id: Uuid,
    _runbook_id: Uuid,
    ...
) { /* canonical start occurs separately */ }
```

**Proposed Hardened Pattern**

```rust
pub struct StartCommand {
    tenant_id: TenantId,
    instance_id: InstanceId,
    artifact_hash: ArtifactHash,
    entry_id: EntryId,
    runbook_id: RunbookId,
    correlation_id: CorrelationId,
    idempotency_key: IdempotencyKey,
    initial_payload: CanonicalJson,
}

store.start_instance(command, verified_snapshot, started_event).await?;
// One transaction; unique (tenant_id, idempotency_key).
```

#### M2. Duplicate process-instance models create split authority

**Evidence:** `process_instances`/`ProcessInstance` drive the VM and plan-discriminated engine, while migration 034 adds `bpmn_process_instance` with `BpmnProcessInstanceStore`. Later migrations and bus code continue to use both. The second table is neither explicitly a projection nor transactionally derived.

**Current Pattern**

```text
process_instances       <- Engine / ProcessStore
bpmn_process_instance   <- bus handlers / BpmnProcessInstanceStore
```

**Proposed Hardened Pattern**

```text
workflow_instances      <- sole writable aggregate snapshot
workflow_effects        <- outbox/inbox effect state
workflow_journal        <- immutable transition records
instance_read_model     <- optional rebuildable projection, never authoritative
```

Backfill and validate the canonical table, switch reads, stop dual writes, then remove the obsolete table and trait in a later compatibility release.

#### M3. Job retry semantics can loop or ignore policy

**Evidence:** transient job failure schedules approximately immediate retry, ignores the provided retry hint, reports an incorrect remaining count, and the retry update uses saturation without a terminal guard. Explicit failure and stale-claim reclamation apply different exhaustion rules. Dedupe pruning after 24 hours also weakens protection against very late deliveries.

**Current Pattern**

```rust
if transient {
    retry_claimed_job(job_key, now + 1).await?;
    // retry_hint_ms and durable attempt policy are not authoritative
}
```

**Proposed Hardened Pattern**

```rust
match RetryPolicy::decision(attempt, retry_hint, error_class) {
    RetryDecision::At { attempt, due_at } => {
        effect.schedule_attempt(attempt, due_at)?;
    }
    RetryDecision::Exhausted => effect.dead_letter(error)?,
    RetryDecision::Terminal => effect.fail(error)?,
}
```

Attempt count, policy version, next due time, and terminal state must update atomically. Inbox dedupe records must live at least as long as the workflow/effect retention contract.

#### M4. Outbox dispatch holds database locks across network I/O

**Evidence:** the sender begins a SQL transaction, selects/locks an entry, resolves and connects to a peer, performs gRPC submission, and only then updates/commits. Tenants and entries are processed serially; a new channel is built per entry. Rejections without execution IDs are recorded as retries indefinitely even when the receiver says they are non-retryable.

**Current Pattern**

```rust
let mut tx = pool.begin().await?;
let entries = claim_outbox(&mut tx).await?;
for entry in entries {
    let channel = endpoint.connect().await?;
    client.submit(request).await?; // transaction and row lock remain open
}
tx.commit().await?;
```

**Proposed Hardened Pattern**

```rust
let batch = store.claim_effects(owner, lease, limit).await?; // short transaction

stream::iter(batch)
    .map(|effect| dispatch_with_pooled_channel(effect))
    .buffer_unordered(MAX_IN_FLIGHT)
    .for_each(|result| store.record_dispatch_result(result))
    .await;
```

Claims use a lease/fence, connections are pooled per peer, authority/version/malformed rejection is terminal, and only transport/lock failures follow bounded backoff.

#### M5. Hot persistence paths scale with tenant count and serialize full state

**Evidence:** `load_instance`, `save_fiber`, `load_fibers`, event methods, and other calls resolve tenant from instance ID by enumerating tenants. A normal tick loads the artifact once and fibers up to three times. Fibers serialize complete stack/register/wait JSON; jobs copy process payload, flags, and session stack.

**Current Pattern**

```rust
async fn load_instance(&self, id: Uuid) -> Result<Option<ProcessInstance>> {
    let tenant = self.resolve_tenant_id(id).await?; // loops tenants
    self.load_in_tenant(&tenant, id).await
}
```

**Proposed Hardened Pattern**

```rust
async fn load_snapshot(
    &self,
    tenant: &TenantId,
    instance: InstanceId,
) -> Result<SnapshotEnvelope, StoreError>;

// Scheduler claim returns the already-loaded revision/fence and due command.
struct ClaimedWork {
    claim: Claim,
    snapshot: SnapshotEnvelope,
    command: Command,
}
```

Use tenant-qualified composite keys, an immutable `Arc<ExecutableWorkflow>` cache keyed by full artifact hash, one snapshot load per transition, compact binary frame encoding, bounded frame sizes, and payload references in queues.

#### M6. Scheduler throughput is sequential and unfair under slow work

**Evidence:** claimed IDs are ticked sequentially per tenant. Default polling and batch limits cap scheduling cadence, while direct FFI keeps the tick occupied under a lease. There is no explicit global/per-tenant in-flight budget, due-work priority, or backpressure tied to the DB pool.

**Current Pattern**

```rust
for instance_id in claimed_ids {
    engine.tick_instance_as_owner(instance_id, owner).await?;
}
```

**Proposed Hardened Pattern**

```rust
let permits = Arc::new(Semaphore::new(config.max_in_flight));
fair_tenant_stream(claimed_work)
    .map(|work| run_one_transition(work, permits.clone()))
    .buffer_unordered(config.max_in_flight)
    .collect::<Vec<_>>()
    .await;
```

The scheduler should claim only locally executable transitions. Remote work is an outbox concern. Concurrency must be bounded by pool capacity, tenant quotas, and measured CPU cost.

#### M7. Event subscription loses continuity on lag

**Evidence:** subscription replays persisted events and then reads a broadcast channel, but `while let Ok(...)` terminates silently on lag. There is no cursor-based backfill after `Lagged`, and an incident event is treated as terminal despite incidents being resolvable.

**Current Pattern**

```rust
while let Ok(event) = receiver.recv().await {
    yield event;
}
```

**Proposed Hardened Pattern**

```rust
loop {
    match receiver.recv().await {
        Ok(event) if event.seq == next_seq => deliver(event),
        Ok(event) if event.seq > next_seq => backfill(next_seq..event.seq).await?,
        Err(RecvError::Lagged(_)) => backfill_from(next_seq).await?,
        Err(RecvError::Closed) => reconnect_from(next_seq).await?,
        _ => {}
    }
}
```

Subscriber cursors must be sequence-based. Only immutable process terminal states close a process stream.

#### M8. Public API and error boundaries do not encode invariants

**Evidence:** compiler modules described as internal are public; `CompiledProgram`, `TransactionContext`, and its operation vector expose writable internals; store implementations expose low-level tenant transaction details; production modules export demo/plan-walker facilities. `anyhow` and silent fallback values blur corruption, invalid input, conflict, transient failure, and business rejection.

**Current Pattern**

```rust
pub struct TransactionContext {
    pub instance_id: Uuid,
    pub tenant_id: String,
    pub ops: Vec<TickOperation>,
}

async fn load_instance(...) -> anyhow::Result<_>;
```

**Proposed Hardened Pattern**

```rust
pub(crate) struct TransitionBuilder { /* invariant-preserving fields */ }

#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("revision conflict")]
    Conflict,
    #[error("stale fencing token")]
    StaleFence,
    #[error("snapshot integrity failure: {0}")]
    Integrity(IntegrityError),
    #[error("storage unavailable: {0}")]
    Unavailable(StorageCause),
}
```

Use `pub(crate)` by default, narrow facade re-exports, sealed verified types, typed domain errors, and `#![forbid(unsafe_code)]` across the core/compiler/runtime crates.

#### M9. Integrity checking is misleading and incomplete

**Evidence:** `verify_instance_integrity` exists but normal runtime load does not call it. PostgreSQL immutability triggers protect selected columns, while engine code handles `IntegrityViolation` as though each pickup verifies a snapshot. The hash does not cover every auxiliary row required to execute a transition.

**Current Pattern**

```rust
let instance = store.load_instance(id).await?;
// No mandatory verification of snapshot + fibers + pending effects.
```

**Proposed Hardened Pattern**

```rust
let envelope = store.load_snapshot(tenant, id).await?;
envelope.verify_hash()?;
envelope.verify_artifact_binding(&artifact)?;
envelope.verify_revision_chain(last_journal_record)?;
```

Integrity must cover the canonical aggregate or be described honestly as column immutability. On mismatch, quarantine atomically and make readiness/alerts reflect the failure.

#### M10. Production packaging and CI defaults are unsafe

**Evidence:** `bpmn-lite-server` has no default PostgreSQL feature while the binary defaults `BPMN_LITE_STORE` to `postgres`. The same binary starts a memory-backed REST demo on `0.0.0.0:8080` without authentication. The checked-in GitHub workflow runs the layering script but does not enforce build, test, clippy, formatting, PostgreSQL migrations, bus integration, crash recovery, or WASM compatibility. Numerous PostgreSQL and bus tests are ignored.

**Current Pattern**

```rust
let rest_bind = env::var("BPMN_LITE_REST_BIND")
    .unwrap_or_else(|_| "0.0.0.0:8080".to_string());
tokio::spawn(axum::serve(listener, demo_router(...)));
```

**Proposed Hardened Pattern**

```rust
if config.demo.enabled {
    ensure!(config.environment != Environment::Production,
            "demo endpoints are forbidden in production");
    serve_demo(config.demo.bind.unwrap_or(localhost_ephemeral())).await?;
}
```

Build separate production and demo binaries. Production startup must validate persistence features, migrations, authentication, tenancy, recovery scan completion, and required dispatcher capabilities before reporting ready.

### Minor findings

- Core modules are several thousand lines long, which makes transactional ownership difficult to audit and encourages direct store calls.
- Some comments are stale, including the stated `ProcessStore` method count and claims that FFI events match a table the runtime does not populate.
- Duplicate comments and unused imports are present in hot runtime files.
- Several parsing and binding paths silently substitute `false`, `0`, `Null`, empty output, or strings when a contract is invalid. These should produce typed compile-time or transition errors.
- The workspace has inconsistent lint policy and unused patch warnings. These are not runtime failures, but they reduce signal in production CI.
- `async_trait` and dynamic dispatch are not the current performance bottleneck; database round trips and serialization are. Replacing all trait objects before correcting transaction granularity would be optimization theater.

## Section 4: Production Hardening & Implementation Plan

### Target architecture

Both XML and DSL frontends must compile to a single, immutable `ExecutableWorkflow`. A synchronous `bpmn-lite-kernel` should own BPMN semantics and have no database, Tokio, network, wall-clock, or random-number dependency. Its only operation is a deterministic transition:

```rust
pub fn apply(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError>;
```

The native engine becomes an adapter around claim/load/apply/commit. PostgreSQL owns concurrency and durable effect records. Dispatchers consume committed effects outside instance transactions. Results return as deduplicated commands. WASM embeds the same kernel and supplies storage, clock, and transport through host commands rather than linking Tokio/sqlx into the module.

### Phase 1: Immediate Stabilization & Resiliency

- [ ] **P0: Stop new plan-runtime starts.** Keep DSL parsing/validation, but lower valid DSL plans into the canonical executable. Feature-gate `PlanWalker` for migration inspection only.
- [ ] **P0: Fix ordinary timers.** Add durable timer rows with unique deterministic IDs, `(tenant_id, due_at)` claiming, atomic consumption, and fenced fiber resumption. Cover `WaitFor`, `WaitUntil`, race timers, boundary timers, and repeating timers with the same mechanism.
- [ ] **P0: Introduce `revision` and monotonic `fence`.** Increment the fence on every new claim. Require tenant, expected revision, and fence on the only writable transition API.
- [ ] **P0: Make VM execution pure with respect to persistence.** Remove direct store calls from `complete_job`, race resolution, joins, incidents, messages, and instruction helpers. Return a complete `Transition` mutation set.
- [ ] **P0: Replace direct FFI with durable effects.** Atomically persist snapshot/journal/outbox, dispatch outside the transaction, and consume results through a unique inbox effect ID. Require idempotency keys from external owners.
- [ ] **P0: Repair bus acknowledgement semantics.** Atomically update outbox, pending effect, execution ID, process wait state, and revision. Treat conflict/timeout as retryable without advancing or binding output.
- [ ] **P0: Unify job completion.** Commit resumed control state, payload, flags, inbox dedupe, job acknowledgement, payload history, and events in one fenced transaction.
- [ ] **P0: Unify terminal transitions.** Cancellation, termination, incidents, failure routing, race winners, job cleanup, and child starts must each be one transition commit.
- [ ] **P0: Version persistent data.** Add snapshot, artifact ABI, journal event, effect protocol, and command schema versions with explicit upgrade functions.
- [ ] **P0: Harden artifacts.** Hash canonical bytes of the complete executable envelope; fail if an existing hash maps to different bytes. Make fields private and admit only verifier-produced artifacts.
- [ ] **P0: Extend verification.** Prove stack height/type at each CFG edge, register bounds, resource maxima, instruction/side-table consistency, entry/end reachability, and bounded backward flow.
- [ ] **P0: Consolidate persistence authority.** Backfill `workflow_instances` from the canonical process row, validate lineage and hashes, stop dual writes, and turn any required UI view into a rebuildable projection. Quarantine plan instances that cannot be translated with demonstrated semantic equivalence.
- [ ] **P0: Make recovery a readiness gate.** Scan every tenant, claim work with fences, reconcile pending effects/timers/jobs, quarantine corrupt snapshots, and refuse readiness if required migrations or recovery invariants fail.
- [ ] **P1: Correct retry contracts.** Persist policy version, attempt, due time, last error, and terminal state. Apply bounded exponential backoff with jitter derived from the command ID; honor a bounded retry hint.
- [ ] **P1: Preserve lineage.** Make tenant, artifact, entry, runbook, correlation, parent, and idempotency identities required typed start-command fields committed with the initial snapshot and event.

**Phase 1 exit criteria**

- Every durable state mutation is executed through one revision/fence-checked commit API.
- Fault injection at every pre/post-commit boundary produces neither a lost transition nor more than one committed effect identity.
- Timer, job, message, FFI, bus, cancellation, incident, race, and child-start scenarios recover after forced process termination.
- Existing active VM snapshots migrate or remain readable through an explicit version adapter. Untranslatable plan snapshots are reported and quarantined, never guessed.

### Phase 2: Performance & Scalability

- [ ] Require tenant-qualified keys on every store method and remove tenant enumeration from instance reads.
- [ ] Return the snapshot, revision, fence, and due command from scheduler claim to avoid reloading the aggregate.
- [ ] Cache immutable verified artifacts in bounded `Arc` caches keyed by full artifact hash; invalidate only through versioned deployment metadata.
- [ ] Replace JSONB fiber stacks/registers with a versioned compact encoding after benchmarking postcard/rkyv-style formats against upgrade requirements. Keep a deterministic canonical format for hashing.
- [ ] Store large domain payloads once by content/version reference; do not copy full payload, flags, and session stack into each job or effect row.
- [ ] Enforce compiler-derived and runtime-checked limits for stack depth, register count, fibers, joins, pending effects, payload size, event size, and steps per transition.
- [ ] Split `ProcessStore` into a small transactional runtime store, immutable artifact repository, journal reader, and administration/projection interfaces. Use typed errors throughout.
- [ ] Implement bounded concurrent scheduling with round-robin tenant fairness, per-tenant quotas, due-time ordering, and a global limit tied to CPU and database-pool capacity.
- [ ] Claim outbox work in a short transaction, release locks, dispatch concurrently through pooled peer channels, and commit each result independently.
- [ ] Remove remote dispatch from transition leases. A transition should normally hold its database transaction for milliseconds, not the duration of a network request.
- [ ] Make broker subscriptions cursor-based and backfill from PostgreSQL after lag or reconnect.
- [ ] Profile before adding lock-free structures. The kernel should already be lock-free through exclusive snapshot ownership; scheduler queues should be sharded only where contention measurements justify it.

**Phase 2 exit criteria**

- Instance lookup and transition work are O(1) in tenant count.
- A normal transition performs one aggregate load and one commit, excluding artifact cache misses.
- Load tests report throughput, p50/p95/p99 transition latency, DB round trips, lock wait, scheduler lag, outbox age, allocations, and resident bytes per active/parked instance.
- CI rejects more than a 10% regression from an approved benchmark baseline unless the change includes an explicit performance waiver.

### Phase 3: Production Readiness

- [ ] Add a transition journal containing command ID/type, logical time, prior/new revision, artifact hash, state hash, event envelopes, and durable effect IDs.
- [ ] Implement replay from genesis and checkpoint-tail replay; compare the reconstructed snapshot hash with the stored snapshot and emit a hard divergence incident.
- [ ] Add deterministic clock/ID test adapters and prohibit direct wall-clock/random access in the kernel through dependency and lint boundaries.
- [ ] Add OpenTelemetry traces spanning claim, transition, commit, effect dispatch, remote execution, inbox result, and resume using stable instance/command/effect correlation.
- [ ] Export metrics for due-work lag, transition latency, conflict rate, stale fences, lease expiry, retries, dead letters, timer lateness, outbox age, recovery duration, quarantines, and replay divergence.
- [ ] Add structured operational endpoints for readiness, liveness, migration level, recovery status, scheduler ownership, queue depth, and artifact availability.
- [ ] Split the demo server from the production binary. Require authentication/authorization, TLS configuration, request limits, tenant identity, and non-wildcard bind acknowledgement in production.
- [ ] Make `fmt --check`, clippy with warnings denied, workspace tests, PostgreSQL migrations, RLS tests, bus/FFI integration, crash-recovery tests, compiler property tests, and WASM kernel builds mandatory.
- [ ] Run restart/kill fault injection at every transition cut point, database failover tests, duplicate/reordered delivery tests, rolling-version compatibility tests, and multi-replica lease races in CI or nightly gates.
- [ ] Add sustained soak tests with high parked-instance counts, timer storms, hot tenants, slow/dead peers, large payloads, event-consumer lag, and database connection pressure.
- [ ] Define retention and archival rules for snapshots, journal, payload versions, effects, dedupe/inbox records, incidents, and dead letters. Dedupe retention must cover the maximum possible late-delivery period.
- [ ] Document the delivery guarantee accurately as at-least-once transport with effectively-once committed workflow effects when receivers honor the idempotency key.

**Phase 3 exit criteria**

- The deterministic kernel builds for `wasm32-wasip2` without Tokio, sqlx, sockets, filesystem access, wall clock, or nondeterministic randomness.
- Replay produces the same state hash across native and WASM implementations for the same artifact, snapshot, commands, and logical time.
- Multi-replica chaos tests demonstrate that expired owners cannot commit, duplicate results do not reapply outputs, and no accepted effect disappears.
- All formerly ignored production-path PostgreSQL and bus tests run in CI against ephemeral infrastructure.
- Production readiness remains false until migrations, recovery scan, artifact verification, required dispatchers, and tenant isolation checks succeed.
