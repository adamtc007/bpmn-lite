# Semantic gameboard Phase 2 item 9 — multi-operation tranche (v0.9)

Date: 2026-08-10

Phase: 2 — deterministic legal-move engine (direct-edit semantic-IDE baseline)

Closes the deferred scope from
`docs/receipts/semantic-gameboard-phase2-direct-edit-equivalence-generalization-2026-08-10.md`:
the 6 multi-`Operation` candidates (`attach_guard`, `attach_rearming_guard`, 4 `prod.*`
productions) that were refused outright by `resolve_direct_edit`'s single-op guard.
Full design ratified in `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 2 item 9
(v0.9 amendment).

## Mechanism

`recover_candidate_shape` generalizes from `&Operation` to `&[Operation]`. The 13
existing single-op arms fold unchanged into a `[operation] => ...` case (renamed
`recover_single_operation_shape`). `resolve_direct_edit`'s comparison machinery
(`designer_graph::productions::apply_production` + `DesignerDag::ir_graphs_equivalent`,
both added in the v0.8 change) required **zero** changes — both already operate on
`&[Operation]`/resulting `IRGraph` content, never on operation count or identity.

Of the 6 candidates:

- **5 are mechanical**: `op.attach_guard`, `op.attach_rearming_guard` (2-op,
  `AttachGuard`/`AttachRearmingGuard` chained to an `AppendNode` whose `anchor` equals
  the guard op's minted `key`), `prod.request_and_wait` (2-op, chained `InsertAfter`s
  ending in `IRNode::MessageWait`), `prod.interrupting_timeout` (3-op, `AttachGuard`
  chained through two `AppendNode`s, the last an `IRNode::End`).
- **1 pair is structurally ambiguous**: `prod.reminder_then_escalate` and
  `prod.non_interrupting_notification` materialize byte-for-byte identical
  `Vec<Operation>` shapes (`AttachRearmingGuard`/`Cycle` → `AppendNode` → `End`) —
  differing only in which workbook slot *name* (`escalation`/`max_reminders` vs.
  `notification`/`max_fires`) supplied the same typed values, which never reaches the
  operation content. Confirmed by reading both production functions in
  `designer-graph/src/productions.rs` — the emitted op sequences are identical field for
  field.

## Fork surfaced and ruled

Presented to the user: guess a canonical label (deterministic first-match) or fail
closed. **Ruled: fail closed** — matches CLAUDE.md's "fail closed; reject, don't skip"
and "no trap doors." A new `ShapeRefusal` enum (`bpmn-lite-server-designer/src/proposal.rs`):

```rust
pub(crate) enum ShapeRefusal {
    NotProducible,
    Ambiguous,
}
```

lets `recover_candidate_shape` name the ambiguous case as a distinct, real defect class
rather than folding it into "no candidate matched." `resolve_direct_edit` reports
`"ambiguous_candidate_shape"` — a raw tape reproducing this exact 3-op shape is never
resolved to either candidate's move id.

## Test-scope finding, matching the `op.delete_subgraph` precedent

Three of the five mechanical candidates' standalone materializations independently fail
full compiler admission (`DesignerDag::admit`) when applied alone to a plain seeded
session — discovered while writing the first HTTP round-trip test for `attach_guard`,
which failed staging with `"Bytecode target out of bounds"` before ever reaching
`resolve_direct_edit`'s own logic:

- `attach_guard` / `attach_rearming_guard`: their 2-op materialization is the guard plus
  a bare escape task with no further outgoing edge. A task that isn't itself an `End`
  and has no successor fails bytecode lowering — this is a property of the candidate's
  own materialized shape (real production ratification of `op.attach_guard` alone hits
  the same wall), orthogonal to this change.
- `request_and_wait`: its `MessageWait.corr_key_source` must reference an existing
  `IRNode::DataObject`. No `designer_graph::ops::Operation` variant can create one —
  confirmed by grep across `ops.rs` — only `DesignerDag::seed` can, at DAG-construction
  time, never through `/graph-edit`. No cheap end-to-end HTTP fixture exists.

Regression coverage for these three is therefore at the `recover_candidate_shape` unit
level — precisely the part this change touched — matching the established
`op.delete_subgraph` precedent from the v0.8 receipt. `prod.interrupting_timeout`
(self-contained: guard + task + its own `End`, no external data dependency) gets a full
HTTP round-trip proof, as does the ambiguous-shape refusal (which never needs
materialization to succeed — it refuses before reaching that stage).

## Tests

- `proposal::tests::recover_candidate_shape_attach_guard_and_attach_rearming_guard` —
  both 2-op arms recover the correct candidate id, anchor, and typed answers
  (escape identifier, duration/interval/max_fires).
- `recover_candidate_shape_request_and_wait_resolves_the_far_endpoint` — 2-op chained
  `InsertAfter` recovers `prod.request_and_wait` with request identifier and
  correlation-source answers.
- `rest::tests::test_direct_edit_recovers_interrupting_timeout_equivalence` — real HTTP
  round-trip: a raw 3-op `AttachGuard`→`AppendNode`→`AppendNode(End)` tape matching what
  `prod.interrupting_timeout` would materialize (including the synthesized
  `{escape}_guard` and `{escape}_end` ids) resolves `edit_kind: "semantic_move_equivalent"`.
- `rest::tests::test_direct_edit_refuses_ambiguous_reminder_or_notification_shape` — RED:
  a raw 3-op `AttachRearmingGuard`/`Cycle`→`AppendNode`→`End` tape resolves
  `lower_level_direct_edit` / `"ambiguous_candidate_shape"`, never a false-positive label.
- `rest::tests::test_session_graph_edit_refuses_invalid_ops_and_persists_nothing`
  (pre-existing, updated): its 2-`AppendNode` tape now correctly resolves
  `"no_supported_semantic_counterpart"` (the retired `"multi_operation_tape"` reason no
  longer exists — every operation-count is now dispatched through the same recovery
  path).

## Results

- `cargo test -p bpmn-lite-server-designer --all-features`: 75/75 (was 71/71; net +4,
  0 regressions).
- `cargo check --workspace --all-targets --all-features`: clean.

Closes red-receipt item 2's full scope (`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`):
all 19 candidates reachable via `materialize_workbook` now resolve through the general
direct-edit equivalence mechanism — 18 by proof, 1 pair by ruled fail-closed refusal.
