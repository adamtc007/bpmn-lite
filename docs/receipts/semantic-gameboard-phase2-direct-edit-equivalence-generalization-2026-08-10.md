# Semantic gameboard Phase 2 item 9 — direct-edit equivalence generalization (v0.8)

Date: 2026-08-10

Phase: 2 — deterministic legal-move engine (direct-edit semantic-IDE baseline)

Closes red-receipt item 2 (`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`):
"the single direct deletion... broader operation-to-move equivalence remains
to be qualified before this route can claim full convergence." Generalizes
`resolve_direct_edit` from `op.delete_subgraph` only to the 12 other
single-`Operation` candidates. Full design ratified in
`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 2 item 9 (v0.8 amendment).

## Mechanism

Recover → match → synthesize → materialize → compare, replacing the old
"search `position.legal_moves()` for a `Complete` binding" strategy (which
structurally only ever worked for `delete_subgraph`, the one candidate with
no argument beyond the auto-bound anchor):

1. `crate::proposal::recover_candidate_shape` — one structural arm per
   `Operation` variant, pulling typed argument values from the operation's
   own content fields. Refuses (`None`) when the operation's shape isn't one
   `materialize_workbook` could ever produce (e.g. a `create_branch`
   condition that isn't `Eq`/`Bool(true)`, a `set_guard_budget` with
   `failure_budget: None`).
2. Locate the matching `LegalMove` by candidate id + anchor (`Incomplete`
   binding expected and accepted).
3. Build a `ProposalWorkbook` directly (no utterance-lexical extraction —
   there is no utterance for a raw edit) and drive it through the real
   `apply_explicit_answers` typed-answer validation with the recovered
   values.
4. Materialize through `utterance_engine::bpmn_board::materialize_bpmn_workbook`
   — the single production materializer (confirmed: `proposal::materialize_operations`
   is test-only and itself delegates to this facade).
5. Apply both the raw submitted operation(s) and the materialized
   operation(s) to separate clones of the same base DAG, reconstruct via
   `to_ir()`, and compare resulting **graph state** — `DesignerDag::ir_graphs_equivalent`
   (new, `designer-graph/src/schema.rs`) — same node set by BPMN id, same
   per-node `IRNode` content, same edges by `(from_bpmn_id, to_bpmn_id, condition)`.
   Internal `NodeKey` handles and wiring-only synthesized ids (`edge_id`,
   `guard_id`, `fork_key`, `join_key`, `entry_edge_id`) never enter this
   comparison — they aren't part of `IRGraph`'s BPMN-visible content.

Considered and rejected: comparing `GraphDeltaPreview` values directly
(already `PartialEq`/`Eq`). Its `payload_hash` is a SHA-256 of the raw
serialized `Operation` struct (`legal_moves.rs::preview_operations`),
synthesized fields and all — the same problem one layer up, with no
partial-match granularity to recover from it.

## Foundational change

Added `PartialEq` to `IRNode`, `TimerSpec`, `ConditionExpr`, `IrLiteral`,
`Expression`, `FfiInputBinding`, `FfiOutputBinding`, `IREdge`
(`bpmn-lite-compiler/src/ir.rs`) — mechanical, additive, no behavioural
change; every field type already supported it.

## Scope

Covers all 13 candidates reachable via a single `Operation`: `delete_subgraph`
(folded into the same general mechanism, no longer a special case),
`append_node`, `insert_before`, `insert_after`, `replace_node`, `connect`,
`create_branch`, `create_parallel_region`, `create_inclusive_region`,
`create_multi_instance_region`, `set_guard_trigger`, `set_guard_budget`,
`set_correlation_source`. The 6 multi-operation candidates (`attach_guard`,
`attach_rearming_guard`, the 4 `prod.*` productions) remain refused by the
existing `let [operation] = operations else {...}` single-op guard,
unchanged — a separate tranche needing N-op tape comparison, explicitly out
of scope here.

## Tests

- `designer_graph::schema::tests::ir_graphs_equivalent_ignores_synthesized_key_and_edge_identity` —
  two independently authored DAGs, different `NodeKey`s and edge ids, same
  content → equivalent.
- `ir_graphs_equivalent_catches_node_content_divergence` — same topology,
  different declared task name → not equivalent.
- `ir_graphs_equivalent_catches_edge_condition_divergence` — same nodes,
  divergent edge condition → not equivalent.
- `bpmn_lite_server_designer::proposal::tests::recover_candidate_shape_delete_node_is_unchanged` —
  regression proof that folding delete into the general table didn't change
  its recovered shape.
- `recover_candidate_shape_connect_resolves_the_far_endpoint`,
  `recover_candidate_shape_create_branch_refuses_unproducible_condition`,
  `recover_candidate_shape_set_guard_budget_refuses_none`,
  `condition_render_round_trips_through_parse`.
- `rest::tests::test_direct_edit_recovers_append_node_equivalence` — real
  HTTP round-trip: a raw `InsertAfter` matching what `op.insert_after` would
  materialize resolves `edit_kind: "semantic_move_equivalent"` with a
  populated `semantic_move_id`.
- `rest::tests::test_direct_edit_diverges_on_content_a_workbook_cannot_produce` —
  RED: a raw edit with content no workbook could ever produce (task type
  other than `"noop"`) resolves `lower_level_direct_edit` /
  `recovered_shape_diverges`, not a false-positive equivalence by
  name/anchor alone.

## Note on test scope for delete specifically

An HTTP-round-trip regression test for `delete_subgraph` was attempted and
dropped: this codebase's `DeleteNode` never auto-bridges predecessor to
successor, so deleting any plain flow task from a simple reachable chain
leaves the graph unreachable and fails full compiler admission — a
pre-existing, orthogonal graph-validity property, not something this change
touches. The only "delete trivially admits" shape in the existing test
corpus is a `DataObject` node, which can only be seeded at DAG-construction
time (`DesignerDag::seed`), not through any exposed `Operation`/HTTP path,
so no cheap end-to-end fixture exists. Regression coverage for the delete
path is therefore at the `recover_candidate_shape` unit level, which is
precisely the part this change touched; the resulting-graph comparison and
HTTP-level wiring are already proven generically by the `append_node` and
divergence tests above.

## Results

- `cargo test -p bpmn-lite-server-designer --all-features`: 70/70 (was 63/63
  before this change; net +7 new tests, 0 regressions).
- `cargo test -p bpmn-lite-server-designer` (default features): 68/68.
- `cargo test -p designer-graph --all-features`: 62/62 (+3 new).
- `cargo test -p utterance-engine --all-features`: 109/109 (unchanged).
- `cargo check --workspace --all-targets --all-features`: clean.

No Phase 7 gate receipt is written by this change. Phase 7 remains RED
pending the Sage audit/history compatibility boundary (red-receipt item 4)
and the libFuzzer smoke (item 8, host-blocked).
