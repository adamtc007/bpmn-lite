# T3 Implementation Prompt — BPMN Executor Plan Walker

## Context

You are working in the `bpmn-lite/` workspace (its own git repo). Read `CLAUDE.md` at the workspace root first — it describes all 18 crates, the A-phase/B-phase history, and the existing engine architecture.

T2 (the federated DSL bus infrastructure) is complete and committed. The bus crates (`dsl-bus-protocol`, `dsl-bus-storage`, `dsl-bus-client`, `dsl-bus-server`), handler crates (`bpmn-lite-bus-handler`, `ob-poc-bus-handler`, `dmn-lite-bus-handler`), and app wiring (`bpmn-lite-server/src/bus_runtime.rs`) are all working.

T3 builds the **plan walker** — the piece that advances a `WorkflowExecutionPlan` through its nodes, dispatching cross-domain verb invocations over the bus and resuming when results arrive.

**The existing `BpmnLiteEngine` in `bpmn-lite-engine/src/engine.rs` (2,435 lines) walks BPMN XML bytecode via the fiber VM. T3 does NOT modify that path.** T3 adds a parallel walker that operates on the bpmn-dsl compiler's `WorkflowExecutionPlan` type. Both share the same store layer.

---

## Pre-locked decisions (non-negotiable)

1. **New module, not new crate.** The plan walker lives in `bpmn-lite-engine/src/plan_walker.rs`. It shares the store via the same `Arc<dyn ProcessStore>` the bytecode engine uses. Do NOT create a `bpmn-lite-runtime` crate.

2. **Plan stored separately by hash.** Same pattern as `CompiledProgram` + `bytecode_version`. Serialize the `WorkflowExecutionPlan` (serde JSON or bincode), BLAKE3 hash it, store via new `store_plan(hash, &plan)` / `load_plan(hash)` methods on `ProcessStore`. Add `plan_hash: Option<[u8; 32]>` to `ProcessInstance`. The plan is immutable; the instance is mutable. They stay in separate rows.

3. **Tick-driven advancement.** When a bus result arrives, `StoreBackedAdvancer` (in `bus_runtime.rs`) sets the process instance to `Running` and returns. The scheduler's `tick_all` / `tick_instance` picks it up on the next cycle and calls the plan walker. Do NOT call the walker inline from the advancer.

4. **Rip orphans first.** 8 dead files in `bpmn-lite-engine/src/` are not referenced from `lib.rs`. Delete them in the first commit before writing any new code.

---

## Implementation order

### T3.0 — Rip orphan files

`git rm` these 8 files from `bpmn-lite-engine/src/`:
- `bpmn_executor.rs` (791 lines — old v0.3 executor)
- `demo.rs` (261 lines)
- `demo_invoker.rs` (67 lines)
- `event_bus.rs` (131 lines)
- `lifecycle.rs` (57 lines)
- `sage_observer.rs` (525 lines)
- `subscriber.rs` (321 lines)
- `verb_catalogue.rs` (185 lines)

None are referenced from `lib.rs`. Verify with `cargo build --workspace` after deletion.

Commit: `T3.0: rip 8 orphan engine files (2,338 LOC) — dead since T2 rip-and-replace`

**STOP. Verify the workspace compiles and all tests pass before proceeding.**

---

### T3.1 — Plan storage on ProcessStore

**Files to modify:**
- `bpmn-lite-store/src/store.rs` — add `store_plan` / `load_plan` to `ProcessStore` trait
- `bpmn-lite-store/src/memory.rs` — implement for `MemoryStore`
- `bpmn-lite-store-postgres/src/lib.rs` (or appropriate file) — implement for `PostgresProcessStore`
- `bpmn-lite-types/src/lib.rs` (or `process_instance.rs`) — add `plan_hash: Option<[u8; 32]>` to `ProcessInstance`

**Pattern to follow:** Look at how `store_program` / `load_program` work for `CompiledProgram` + `bytecode_version`. Mirror that exactly:

```rust
// On ProcessStore trait:
async fn store_plan(&self, plan_hash: [u8; 32], plan: &WorkflowExecutionPlan) -> Result<()>;
async fn load_plan(&self, plan_hash: [u8; 32]) -> Result<Option<WorkflowExecutionPlan>>;
```

Serialization: use `serde_json::to_vec` for the plan body. Derive `Serialize, Deserialize` on `WorkflowExecutionPlan` and all its constituent types in `bpmn-lite-compiler/src/dsl/plan.rs` (they currently only derive `Debug, Clone`).

Hash: `blake3::hash(&serialized_bytes)` — the workspace already uses BLAKE3 for `bytecode_version` and FFI template IDs.

**Postgres migration:** New migration (next sequence number after the latest in `bpmn-lite-store-postgres/migrations/`). Table `workflow_plans` with columns `plan_hash BYTEA PRIMARY KEY, plan_body JSONB NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now()`. Add `plan_hash BYTEA` column to `process_instances` table (nullable — bytecode-path instances won't have it).

**Add to ProcessInstance:** `plan_hash: Option<[u8; 32]>` and `current_node_id: Option<String>` and `placeholder_values: Option<serde_json::Value>` (JSON object, `HashMap<String, Value>` when deserialized).

Commit: `T3.1: plan storage — WorkflowExecutionPlan stored by BLAKE3 hash, ProcessInstance extended with plan_hash/current_node_id/placeholder_values`

**STOP. Verify compile + tests.**

---

### T3.2 — The plan walker core

**New file:** `bpmn-lite-engine/src/plan_walker.rs`

**Register in `lib.rs`:**
```rust
pub mod plan_walker;
```

**Structure:**

```rust
use bpmn_lite_compiler::dsl::plan::*;
use bpmn_lite_store::store::ProcessStore;
// ... other imports

pub struct PlanWalker {
    store: Arc<dyn ProcessStore>,
    bus_client: Arc<dsl_bus_client::BusClient>,
    tenant_id: String,
}

/// Result of one advance cycle.
pub enum AdvanceOutcome {
    /// Reached a callout node — submitted to bus, now WaitingOnSubmission.
    Submitted { callout_id: Uuid, node_id: String, verb_fqn: String },
    /// Reached an end event — process completed.
    Completed { node_id: String, status: String },
    /// Process is not in a walkable state (already waiting, failed, etc).
    NotRunnable,
}
```

**Core method — `advance`:**

```rust
impl PlanWalker {
    /// Advance a plan-based process instance until the next callout or end event.
    /// Call this from the tick loop when instance.status == Running && instance.plan_hash.is_some().
    pub async fn advance(&self, instance_id: Uuid) -> Result<AdvanceOutcome> {
        // 1. Load instance. If status != Running, return NotRunnable.
        // 2. Load plan via instance.plan_hash.
        // 3. Read current_node_id from instance.
        // 4. Enter walk loop:
        //    - Look up node in plan.nodes[current_node_id]
        //    - Match on ExecutionNode variant:
        //
        //      StartEvent → set current_node_id = node.next, continue loop
        //
        //      ExclusiveGateway → evaluate placeholder_values against each
        //        GatewayExecFlow.placeholder / expected_value. Take first match.
        //        Set current_node_id = matched_flow.next, continue loop.
        //        If no match → Failed (no gateway path matched).
        //
        //      ServiceTask → dispatch over bus:
        //        a. Generate callout_id (UUIDv7) + idempotency_key (UUIDv7)
        //        b. Build InvocationRequest from verb_fqn + static_args + 
        //           resolved placeholders from instance.placeholder_values
        //        c. Insert pending invocation (callout_id, process_instance_id,
        //           node_id, idempotency_key, execution_id=None)
        //        d. Insert outbox entry (with callout_id)
        //        e. Set instance.status = WaitingOnSubmission,
        //           instance.waiting_on_callout_id = Some(callout_id),
        //           instance.current_node_id = Some(node.id) [stays on this node until result]
        //        f. Save instance. Return Submitted.
        //
        //      BusinessRuleTask → same dispatch pattern as ServiceTask,
        //        using decision_id instead of verb_fqn
        //
        //      EndEvent → set instance.status = Completed,
        //        instance.completed_at = Some(now),
        //        instance.end_status = Some(node.status).
        //        Save instance. Return Completed.
        //
        // 5. Save instance after each node transition (current_node_id + placeholder_values).
    }
}
```

**Gateway evaluation** is deliberately simple for v0.6:
```rust
fn evaluate_gateway(
    gateway: &GatewayExecNode,
    placeholder_values: &HashMap<String, serde_json::Value>,
) -> Result<&str> {
    for flow in &gateway.flows {
        if let Some(val) = placeholder_values.get(&flow.placeholder) {
            if val.as_str() == Some(&flow.expected_value) {
                return Ok(&flow.next);
            }
        }
    }
    Err(anyhow!("no gateway flow matched for gateway {}", gateway.id))
}
```

**Domain-prefix splitting on verb_fqn (CRITICAL):**

`ServiceTaskExecNode.verb_fqn` is namespaced: `"ob-poc:cbu.create"`, `"dmn-lite:cbu_type_routing"`. The plan walker MUST split on `:` to extract the target domain for bus routing and the bare verb_id for the `InvocationRequest`:

```rust
fn split_verb_fqn(verb_fqn: &str) -> Result<(&str, &str)> {
    verb_fqn.split_once(':')
        .ok_or_else(|| anyhow!(
            "verb_fqn missing domain prefix (expected 'domain:verb.id'): {}",
            verb_fqn
        ))
}

// In the ServiceTask dispatch path:
let (target_domain, verb_id) = split_verb_fqn(&node.verb_fqn)?;
client.submit_invocation(target_domain, InvocationRequest {
    verb_id: verb_id.to_owned(),
    // ... rest of request with static_args + resolved placeholders
}).await?;
```

Same applies to `BusinessRuleExecNode.decision_id` — it's namespaced too (`"dmn-lite:cbu_type_routing"`).

Do NOT send the full namespaced FQN as `verb_id` on the wire — the receiver doesn't understand the prefix.

**Session stack propagation is OUT OF T3 SCOPE.**

The existing bytecode path carries `SessionStackState` on `ProcessInstance` and `JobActivation`. The new bus path does NOT propagate session context across domains. The `InvocationRequest` carries `input_payload` (verb args as JSON) and `authority` — that's sufficient for T3's DoD scenarios (simple verbs like `cbu.create`). Full session context propagation across the bus is a T5/Sage concern.

**Bus dispatch helper** — builds the outbox + pending rows atomically. Look at how `dsl_bus_storage::insert_outbox` works and how `bpmn-lite-store/src/pending.rs` `PendingInvocationStore::insert` works. Both need to be written in the same transaction (or at minimum the same tick — for MemoryStore they're just sequential inserts; for Postgres they should share a transaction).

**Do NOT** touch `engine.rs` or the bytecode tick path. The plan walker is a separate entry point.

Commit: `T3.2: plan walker — advance_internal walks WorkflowExecutionPlan nodes, dispatches callouts over bus`

**STOP. Unit tests against MemoryStore before proceeding.**

---

### T3.3 — Wire walker into the tick loop

**File to modify:** `bpmn-lite-engine/src/engine.rs`

Add a method on `BpmnLiteEngine`:

```rust
/// Attach a bus client for plan-based process execution.
pub fn with_bus_client(mut self, client: Arc<BusClient>) -> Self {
    self.bus_client = Some(client);
    self
}
```

In `tick_instance_inner`, after the existing bytecode fiber loop, add a check:

```rust
// After existing fiber_loop for bytecode instances...

// Plan-based instances: if instance has plan_hash and is Running,
// advance via PlanWalker.
if instance.plan_hash.is_some() && instance.state == ProcessState::Running {
    if let Some(bus_client) = &self.bus_client {
        let walker = PlanWalker::new(
            self.store.clone(),
            bus_client.clone(),
            self.tenant_id.clone(),
        );
        walker.advance(instance_id).await?;
        return Ok(()); // plan walker handled this instance
    }
}
```

**Key insight:** bytecode instances have `bytecode_version` set and `plan_hash = None`. Plan instances have `plan_hash` set. They never overlap. The tick loop dispatches to the right walker based on which hash is populated.

**File to modify:** `bpmn-lite-server/src/bus_runtime.rs`

The `StoreBackedAdvancer` currently sets `Running` and logs. No changes needed — the tick loop will pick up the `Running` instance. But verify that `StoreBackedAdvancer` correctly nulls `waiting_on_callout_id` and `waiting_on_execution_id` (it already does on lines 146-147).

**File to modify:** `bpmn-lite-server/src/main.rs`

Wire the `BusClient` into the engine via `.with_bus_client()` so the tick loop has access for plan-based instances.

Commit: `T3.3: wire plan walker into tick loop — plan-based instances advance via PlanWalker on tick`

**STOP. Integration test: create a plan-based instance, verify tick advances it through StartEvent to first ServiceTask callout.**

---

### T3.4 — Placeholder binding on result arrival

**File to modify:** `bpmn-lite-server/src/bus_runtime.rs` (`StoreBackedAdvancer`)

When a result arrives for a plan-based instance, the advancer needs to:
1. Extract output values from the result payload
2. Look up which node produced the result (from the pending row's `node_id`)
3. Load the plan, find that node's `produces_placeholder`
4. If the node produces a placeholder, write the output value into `instance.placeholder_values`

**Check what `ProcessAdvanceInput` carries:**
- `execution_id`, `source_domain`, `outcome_kind`, `outcome_detail`

The `outcome_detail` field needs to carry structured output (the verb's return value) so placeholder values can be extracted. Check the protobuf `DeliverResult` message in `dsl-bus-protocol/proto/dsl_bus.proto` — it should have an `output_payload` or similar field. If it doesn't, this is the one proto extension T3 needs.

**Placeholder extraction:**
```rust
// In StoreBackedAdvancer::advance(), after setting instance.status = Running:
if let Some(plan_hash) = instance.plan_hash {
    let plan = self.store.load_plan(plan_hash).await?;
    if let Some(plan) = plan {
        if let Some(node) = plan.nodes.get(&row.node_id) {
            let produces = match node {
                ExecutionNode::ServiceTask(n) => n.produces_placeholder.as_deref(),
                ExecutionNode::BusinessRuleTask(n) => n.produces_placeholder.as_deref(),
                _ => None,
            };
            if let Some(placeholder_name) = produces {
                // Parse outcome_detail as the output value
                let mut placeholders: HashMap<String, serde_json::Value> = instance
                    .placeholder_values
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                placeholders.insert(
                    placeholder_name.to_owned(),
                    serde_json::from_str(&input.outcome_detail)
                        .unwrap_or(serde_json::Value::String(input.outcome_detail.clone())),
                );
                instance.placeholder_values = Some(serde_json::to_value(&placeholders)?);
            }
        }
    }
}
```

Commit: `T3.4: placeholder binding — result arrival populates placeholder_values from node's produces_placeholder`

**STOP. Test: ServiceTask produces @cbu, result arrives with UUID, verify placeholder_values contains it.**

---

### T3.5 — Start process (plan-based path)

**File to modify:** `bpmn-lite-engine/src/plan_walker.rs` (or `engine.rs`)

```rust
/// Start a new plan-based process instance.
pub async fn start_process(
    &self,
    plan: &WorkflowExecutionPlan,
    initial_variables: HashMap<String, serde_json::Value>,
) -> Result<Uuid> {
    // 1. Serialize plan, compute BLAKE3 hash
    // 2. store_plan(hash, plan) — idempotent if already stored
    // 3. Create ProcessInstance with:
    //    - plan_hash = Some(hash)
    //    - current_node_id = Some(plan.start_node.clone())
    //    - placeholder_values = Some(serde_json::to_value(&initial_variables)?)
    //    - status = Running
    // 4. Save instance
    // 5. Return instance_id
    // The tick loop will pick it up and advance through StartEvent to first callout.
}
```

Commit: `T3.5: start_process for plan-based instances`

---

### T3.6 — Failure handling

**File to modify:** `bpmn-lite-engine/src/plan_walker.rs`

In the walker's `advance` method, handle error cases:
- Gateway with no matching flow → `Failed` with reason
- Node ID not found in plan → `Failed` with reason (data corruption)
- Bus submission failure → retry once, then `Failed`

In `StoreBackedAdvancer` (already partially there):
- `OptimisticConflict` / `LockTimeout` → stays `Running`, next tick retries (re-submits the callout with new idempotency key). Add a retry counter on the pending row or instance to cap retries (e.g., max 3).
- `VerbFailed` / `AuthorityDenied` / `TimedOut` etc. → `Failed` (already handled)
- `VersionMismatch` → `Failed` (already handled)

Commit: `T3.6: failure handling — gateway miss, bus errors, retry cap for transient failures`

---

### T3.7 — Integration tests (4 DoD scenarios)

**New test file:** `bpmn-lite-engine/src/tests/plan_walker_tests.rs` (or as integration tests)

**DoD #41 — ob-poc full round-trip:**
- Compile a simple bpmn-dsl workflow: StartEvent → ServiceTask(ob-poc:cbu.create) → EndEvent
- `start_process(plan, initial_vars)`
- Tick → advances through StartEvent → ServiceTask → outbox row created → WaitingOnSubmission
- Simulate outbox sender: mark_outbox_submitted with execution_id → WaitingOnInvocation
- Simulate result delivery: call advancer.advance() with Committed outcome + output payload
- Tick → advances through EndEvent → Completed
- Assert: process Completed, placeholder_values populated

**DoD #42 — dmn-lite full round-trip:**
- Same pattern but with BusinessRuleTask(dmn-lite:cbu_type_routing)
- Verify decision output binds to placeholder

**DoD #44 — crash mid-outbox-write recovery:**
- Start process, advance to ServiceTask → WaitingOnSubmission
- DO NOT deliver result. Simulate restart: create fresh engine instance from same store.
- Tick → outbox sender sees pending row → re-submits
- Deliver result → process completes
- Assert: no duplicate execution, idempotency key prevents double-submit

**DoD #45 — crash mid-ack reconciliation:**
- Start process, advance to ServiceTask → WaitingOnSubmission
- Simulate ack arrival → WaitingOnInvocation
- Simulate restart: fresh engine instance
- Deliver result → advancer picks up → sets Running → tick advances
- Assert: process completes cleanly

All tests use `MemoryStore` (no Postgres required). Postgres integration tests can be a follow-up.

Commit: `T3.7: integration tests — 4 DoD scenarios passing (ob-poc round-trip, dmn-lite round-trip, crash recovery × 2)`

---

### Final commit

After all T3 slices pass:

```
T3: plan walker complete — WorkflowExecutionPlan advance via tick loop + bus dispatch

- T3.0: rip 8 orphan files (2,338 LOC)
- T3.1: plan storage by BLAKE3 hash + ProcessInstance extended
- T3.2: PlanWalker.advance() — walks nodes, dispatches callouts
- T3.3: wired into tick loop (plan_hash discriminates bytecode vs plan)
- T3.4: placeholder binding on result arrival
- T3.5: start_process for plan-based instances
- T3.6: failure handling + retry cap
- T3.7: 4 DoD scenarios passing

Test count: [N] passing, [M] ignored
```

---

---

## What T3 does NOT touch (but you need to know exists)

### ob-poc bpmn_integration/ (16 files) — the REPL session ↔ BPMN bridge

ob-poc has a complete integration layer in `ob-poc/rust/src/bpmn_integration/` that bridges the REPL session to bpmn-lite via the **existing gRPC path** (Path 1 — `WorkflowDispatcher`, `JobWorker`, `EventBridge`). This is the ob-poc-side consumer. Key types:

- `WorkflowDispatcher` — implements `DslExecutorV2`, routes orchestrated verbs to bpmn-lite gRPC, parks REPL runbook entries
- `JobWorker` — polls bpmn-lite for activated jobs, executes ob-poc verbs, completes jobs
- `EventBridge` — subscribes to lifecycle events, resolves parked tokens
- `CorrelationRecord` — links BPMN process instance ↔ REPL session/runbook
- `SessionStackState` — copied by value from REPL session at dispatch time, carried on process instance
- `PendingDispatch` — queued BPMN dispatch for resilience when gRPC is down

**T3 does not modify any ob-poc code.** The bus path (T2) has its own handler on the ob-poc side (`ob-poc-bus-handler`), which is a separate crate in the bpmn-lite workspace receiving bus invocations. The ob-poc `bpmn_integration/` code handles the gRPC path.

When the plan walker dispatches `submit_invocation("ob-poc", request)` via the bus, it goes to `ob-poc-bus-handler` which currently returns `NOT_IMPLEMENTED` (A3 stubs). Making the ob-poc bus handler actually execute verbs is **post-T3 work** (T4/T5 scope). T3's DoD tests simulate result delivery — they don't require the receiver to actually execute anything.

---

## Files you will read (do this FIRST, before writing any code)

1. `CLAUDE.md` — workspace overview
2. `bpmn-lite-engine/src/engine.rs` — existing bytecode engine (understand the tick loop, store access, transition guards)
3. `bpmn-lite-engine/src/lib.rs` — module structure
4. `bpmn-lite-compiler/src/dsl/plan.rs` — `WorkflowExecutionPlan` and all node types (this is what T3 walks)
5. `bpmn-lite-store/src/store.rs` — `ProcessStore` trait (you're extending it)
6. `bpmn-lite-store/src/pending.rs` — `PendingInvocationStore` trait + `PendingInvocation` struct
7. `bpmn-lite-store/src/process_instance.rs` — `ProcessStatus`, `BpmnProcessInstanceStore`
8. `bpmn-lite-server/src/bus_runtime.rs` — `StoreBackedAdvancer` (the T3 handoff point)
9. `bpmn-lite-bus-handler/src/lib.rs` — `ProcessAdvancer` trait, `ProcessAdvanceInput`
10. `dsl-bus-storage/src/types.rs` — `OutboxEntry` builder (you'll construct these)
11. `dsl-bus-protocol/proto/dsl_bus.proto` — wire format (check if DeliverResult carries output payload)

## Files you will NOT modify

- `bpmn-lite-vm/` — the bytecode VM is not T3's concern
- `bpmn-lite-compiler/` — except adding serde derives to `plan.rs` types
- `bpmn-lite-authoring/` — not involved
- `dmn-lite-*` — not involved
- `ffi-*` — not involved
- `dsl-bus-server/`, `dsl-bus-client/` — T2 infrastructure, do not modify

## Type safety rules (from ob-poc CLAUDE.md, applies here too)

- Never use `serde_json::json!` for structured data — define typed structs
- `Result<T, E>` everywhere, never `.unwrap()` in production paths
- `Option<T>` for nullable fields, no sentinel values
- `#[must_use]` on decision types like `AdvanceOutcome`

## Discipline

- Each T3.N slice is one commit. STOP and verify `cargo build --workspace && cargo test --workspace` between each.
- Do not "while I'm here" refactor anything in the bytecode engine path.
- Do not create new crates.
- Do not modify the bus protocol (proto file) unless `DeliverResult` genuinely lacks an output payload field — and if it does, that's a minimal addition (one `bytes output_payload = N;` field), not a redesign.
