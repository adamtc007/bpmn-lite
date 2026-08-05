# BPMN pack plane ledger

This ledger classifies identifiers by their implemented typed effect. Prefixes
are descriptive only; they are not the classification rule. The explicit
bridges at the end are the only permitted crossings between planes.

## Designer authoring plane

Source of truth is `designer_graph::OperationKind` / `ProductionId`, legality
is `PositionalLegality`, implementation is `designer-graph/src/ops.rs` or
`productions.rs`, and authority is an admitted graph-edit session followed by
human ratification. All live rows are eligible only when returned by positional
legality and policy.

Authority column cites the BPMN semantic registry's `HarmClass`
(`utterance-engine/src/bpmn_pack.rs`) alongside the uniform admission gate —
every row requires the same admitted-session + ratification gate; `HarmClass`
is the one per-row signal that differs (25 of 26 are `Reversible`; only
`op.delete_subgraph` is `Destructive`).

| Canonical id | Typed input/output | Effect | Authority | Implementation/status |
|---|---|---|---|---|
| `op.append_node` | anchor + `IRNode` → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::AppendNode`; live |
| `op.insert_before` | target + `IRNode` → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::InsertBefore`; live |
| `op.insert_after` | target + `IRNode` → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::InsertAfter`; live |
| `op.replace_node` | target + `IRNode` → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::ReplaceNode`; live |
| `op.connect` | existing endpoints + typed edge → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::Connect`; live |
| `op.create_branch` | gateway/target/outcome + branch node → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CreateBranch`; live |
| `op.create_race` | anchor + declared race arms → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CreateRace`; live |
| `op.create_parallel_region` | anchor + branch specs → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CreateParallelRegion`; live |
| `op.close_parallel_region` | split + join placement → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CloseParallelRegion`; live |
| `op.create_inclusive_region` | anchor + conditioned branches → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CreateInclusiveRegion`; live |
| `op.create_multi_instance_region` | collection + ceiling + body → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CreateMultiInstanceRegion`; live |
| `op.attach_guard` | host + trigger + escape → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::AttachGuard`; live |
| `op.attach_rearming_guard` | host + bounded timer + escape → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::AttachRearmingGuard`; live |
| `op.attach_rollback_guard` | host + error/escape → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::AttachRollbackGuard`; live |
| `op.set_guard_trigger` | guard + typed trigger → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::SetGuardTrigger`; live |
| `op.set_guard_budget` | guard + bounded count → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::SetGuardBudget`; live |
| `op.set_correlation_source` | wait + declared data reference → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::SetCorrelationSource`; live |
| `op.call_subprocess` | anchor + pinned subprocess ref → operation | graph edit | admitted session + ratification; `HarmClass::Reversible` | `Operation::CallSubprocess`; live |
| `op.delete_subgraph` | node or closed region → operation | graph edit | admitted session + ratification; `HarmClass::Destructive` (the one destructive designer op) | `Operation::DeleteSubgraph`; live |
| `prod.request_and_wait` | anchor + request + correlation source → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(RequestAndWait)`; live |
| `prod.timer_message_race` | anchor + message + timer + branches → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(TimerMessageRace)`; live |
| `prod.reminder_then_escalate` | host + bounded cycle + escalation → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(ReminderThenEscalate)`; live |
| `prod.interrupting_timeout` | host + duration + escape → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(InterruptingTimeout)`; live |
| `prod.non_interrupting_notification` | host + bounded schedule + notification → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(NonInterruptingNotification)`; live |
| `prod.human_review_with_rework` | anchor + review/rework bound → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(HumanReviewWithRework)`; live |
| `prod.call_durable_subprocess` | anchor + pinned subprocess ref → operations | graph edit macro | admitted session + ratification; `HarmClass::Reversible` | `apply_production(CallDurableSubprocess)`; live |

## BPMN service invocation plane

Source is `manifests/bpmn.dag.yaml`; authority strings and signatures are in
that DAG. The implementation is the exact arm in
`BpmnLiteBusHandler::handle_invocation`. These identifiers are never eligible
for a Designer authoring board.

| Canonical id | Typed input/output | Effect / authority | Implementation/status |
|---|---|---|---|
| `define-template` | template key + plan body → template identity | idempotent publish; `bpmn.template.write` | bus-handler arm; live |
| `spawn-instance` | template + idempotency/lineage ids → instance id | idempotent instance mutation; `bpmn.instance.write` | bus-handler arm; live |
| `deliver-message` | instance + name + payload → void | instance mutation; `bpmn.instance.write` | bus-handler arm; live |
| `correlate-message` | name + correlation key + payload → void | instance mutation; `bpmn.instance.write` | bus-handler arm; live |
| `cancel-instance` | instance + reason → void | instance mutation; `bpmn.instance.write` | bus-handler arm; live |
| `inspect-instance` | instance → details | read; `bpmn.instance.read` | bus-handler arm; live |
| `message-wait` | message name → void | structural wait, not invocation | stale manifest-only entry; obsolete/forbidden |
| `timer-wait` | duration → void | structural wait, not invocation | stale manifest-only entry; obsolete/forbidden |

## SemOS runtime-instance operations plane

Source contracts are the named `ob-poc` verb YAML entries; implementations are
registered `SemOsVerbOp`s. Eligibility requires the BPMN operations workspace,
the relevant subject/arguments and its policy. These ids are ineligible for the
Designer authoring board.

Authority column is each verb's `three_axis.consequence.baseline` from its
own `rust/config/verbs/*.yaml` contract (`benign` / `reviewable` /
`requires_confirmation`) plus the workspace-scope gate every row in this
plane already requires (see intro).

| Canonical id | Typed effect | Authority | Implementation/status |
|---|---|---|---|
| `bpmn.compile` | XML → bytecode/diagnostics (external observational compile) | `bpmn` workspace scope; consequence `benign` | `BpmnCompile`; live |
| `bpmn.start` | process key/payload → instance UUID | `bpmn` workspace scope; consequence `reviewable` | `BpmnStart`; live |
| `bpmn.signal` | instance/message/payload → durable outbox effect | `bpmn` workspace scope; consequence `reviewable` | `BpmnSignal`; live |
| `bpmn.cancel` | instance/reason → durable outbox effect | `bpmn` workspace scope; consequence `requires_confirmation` | `BpmnCancel`; live |
| `bpmn.inspect` | instance → runtime inspection | `bpmn` workspace scope; consequence `benign` | `BpmnInspect`; live |
| `workflow.start-process` | governed process registry request → instance | `bpmn` workspace scope; consequence `reviewable` | `WorkflowStartProcess`; live |
| `bpmn-controller.start-instance` | tenant/process/payload/idempotency → instance | `bpmn` workspace scope; consequence `requires_confirmation` | `BpmnControllerStartInstance`; live |
| `bpmn-controller.instance-status` | instance → status | `bpmn` workspace scope; consequence `benign` | `BpmnControllerInstanceStatus`; live |
| `bpmn-controller.list-instances` | tenant/filter → instance set | `bpmn` workspace scope; consequence `benign` | `BpmnControllerListInstances`; live |
| `bpmn.timer-wait` | duration → void | n/a — forbidden, not a legal invocation | successful no-op; forbidden and removed in Phase 0 |
| `bpmn.message-wait` | message → void | n/a — forbidden, not a legal invocation | successful no-op; forbidden and removed in Phase 0 |
| `bpmn.boundary-timer` | host/timer → void | n/a — forbidden, not a legal invocation | successful no-op; forbidden and removed in Phase 0 |
| `bpmn.boundary-error` | host/error → void | n/a — forbidden, not a legal invocation | successful no-op; forbidden and removed in Phase 0 |

## Infrastructure-control plane

Source contracts are `rust/config/verbs/bpmn-controller.yaml` and
implementations are `bpmn_controller_ops.rs`. Eligibility requires the
separate lifecycle-resources/infrastructure position and stronger policy. These
ids cannot appear while authoring or controlling an ordinary instance.

Authority column is each verb's `three_axis.consequence.baseline` from the
same contract file plus the `lifecycle_resources` workspace-scope gate this
plane's intro requires; `bpmn-infrastructure.yaml`'s pack-level
`risk_policy.max_steps_without_confirm: 2` applies on top of every row here.

| Canonical id | Typed effect | Authority | Implementation/status |
|---|---|---|---|
| `loader.provision-pool` | pool/image/tenant/scaling config → K8s + DB mutation | `lifecycle_resources` workspace scope; consequence `requires_confirmation` | `LoaderProvisionPool`; live |
| `loader.deprovision-pool` | pool → K8s + DB mutation | `lifecycle_resources` workspace scope; consequence `requires_confirmation` | `LoaderDeprovisionPool`; live |
| `loader.pool-status` | pool → health/status | `lifecycle_resources` workspace scope; consequence `benign` | `LoaderPoolStatus`; live |
| `loader.list-pools` | none → pool set | `lifecycle_resources` workspace scope; consequence `benign` | `LoaderListPools`; live |

## Typed bridges

```text
Semantic authoring candidate + proposal workbook
  -> exhaustive designer_graph::Operation materialisation
  -> apply/stage/admit against the current DesignerDag
  -> explicit human ratification and GraphEdit append

Ratified, admitted graph
  -> deterministic IR/execution-plan projection
  -> define-template/compile service invocation
  -> pinned published template or bytecode identity

Runtime or infrastructure utterance in the corresponding SemOS workspace
  -> plane-scoped operations board candidate
  -> exact registered bpmn.*, workflow.*, bpmn-controller.* or loader.* handler
```

There is no bridge from an authoring candidate to an arbitrary runtime verb.
Timer/message waits and boundary events cross the first bridge as typed graph/IR
nodes only; they never enter either executable verb registry.
