# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D3: MultiInstance vertical

**Status:** awaiting acceptance
**Branch:** `codex/bpmn-gameboard-refactor`
**Freeze:** `docs/receipts/EOP-DSL-PARITY-001-D3.0-freeze.md` (ratified "(b) go").

## What was built

`IRNode::MultiInstance` is DSL-representable end-to-end; `emit_dsl`'s
unsupported set drops from 7 kinds to 6 (`GatewayXor`, `GatewayInclusive`,
`HumanWait`, `DataObject`, `FfiServiceTask`, `SendTask`).

Frozen grammar, implemented exactly per the ratified freeze:

```
(multi-instance :id m :task-type t :collection c :max N :next n)
```

`:task-type` and `:collection` are bare Symbol tokens (`parse_kw_symbol`) —
this deviates from the freeze doc's own grammar *sketch*, which wrote
`:task-type "review-doc"` (quoted) and speculated `:collection @docs`
(`@`-placeholder token). Both were corrected during implementation, before
any code was written against them, after checking the primary source: (a)
`TaskAst.plug`/`service-task :verb` — the existing convention for a
dispatch-identity field — is a bare Symbol, never a quoted string; (b)
`collection_flag_name`'s actual construction sites (`ir.rs`, `verifier.rs`,
`lowering.rs`, `dsl/ir_plan.rs`) all use plain names (`"doc_count"`,
`"directors"`, `"items"`), never `@`-prefixed — the `@placeholder`
convention belongs to `ConditionAst`'s inferred split-flow mechanism, a
different feature this field has no relation to. This is an
implementation-detail correction, not a re-litigation of the ratified §2
((b) inputs disposition) or §1 (`name == id`) rulings — those stand as
ratified.

Per the D3.0 freeze's ruled (b): `inputs` (`Vec<FfiInputBinding>`) is
representable only when empty. A graph `MultiInstance` node with non-empty
`inputs` refuses at DSL emission via a new `DslEmitError::InputsUnrepresentable`
variant — never silently dropped.

## Per-layer changes

| Layer | File | Change |
|---|---|---|
| AST | `dsl/ast.rs` | `NodeAst::MultiInstance(MultiInstanceAst { id, task_type, collection_flag_name, declared_max, next, span })` — no `name`, no `inputs` field (freeze §1/§2) |
| Parser | `dsl/parser.rs` | `parse_multi_instance`: `:id`/`:task-type`/`:collection` via `parse_kw_symbol`, `:max` via `parse_kw_u32` (shared with D1/D2's `:max-fires`), `:next` |
| Printer/Mutator | `dsl/refactor.rs` | `ToSexpr` prints the frozen form; ordinary-node arms in `NodeAst`'s dispatch, `rewire_next`, `insert_after` |
| Unroll | `dsl/unroll.rs` | retarget + iteration-clone arms |
| Linter | `dsl/linter.rs` | lowers to the EXISTING `ExecutionNode::MultiInstance(MultiInstanceExecNode)` — the same exec node G5.4a's `ir_plan` projects to (itself carrying neither `name` nor `inputs`), so plan equality is field-identical by construction; `check_next_ref` applies |
| Emitter | `dsl/emit.rs` | `MultiInstance` joins the core: token checks (id, task_type, collection_flag_name) → `inputs.is_empty()` check (else `InputsUnrepresentable`) → `single_out_edge` → `uncond_next` → frozen form. No `required_symbols` entry (verified: `task_type` is never registry-resolved on either lowering path — unlike `ServiceTask.plug`). Out-of-core arm now 6 kinds; module doc updated |
| Diagnostics | `bpmn-lite-authoring/src/diagnostics_executor.rs` | ordinary-node arms in `RemoveUnusedNode` handling and `find_all_predecessors_rec` |
| `repeat.rs` | `bpmn-lite-compiler/src/dsl/repeat.rs` | `find_all_predecessor_ids_rec`'s exhaustive match gained the `MultiInstance` arm (compiler forced this — no wildcard, per B0) |
| B2 harness | `designer-graph/src/b2_roundtrip_receipts.rs` | G16 |

## Red→green trace

**Green (unit + B2 four-proof harness, all passed first run once the
grammar-token correction above was made):**
- `green_multi_instance_declared_max_round_trip` (emit.rs): exact frozen
  form `(multi-instance :id review-all :task-type review-doc :collection docs
  :max 50 :next end)`, empty `required_symbols`, recompile, idempotence.
- `multi_instance_forms_print_reparse_roundtrip_and_recompile` (refactor.rs):
  fixpoint + compile.
- G16 `g16_multi_instance_declared_max_round_trip` (b2): hash witness,
  byte-idempotence, print→parse→print fixpoint, span-stripped plan equality.

**Red (all exact-variant or discriminating-needle, never bare `is_err`):**
- R-D3.1 `multi_instance_red_axes_refuse_at_parse`: missing `:task-type` /
  `:collection` / `:max` (named expected-keyword errors).
- R-D3.3 (same test): malformed `:max` (`10x`) and u32 overflow
  (`4294967296`) — both "not a valid u32 integer", never silent-zero.
- R-D3.5 `red_multi_instance_out_degree_zero_and_two`: `WrongOutDegree`
  with count 0 and 2, id asserted.
- R-D3.6 `red_multi_instance_conditioned_edge`: `UnrepresentableCondition`.
- R-D3.7 `red_multi_instance_bad_id_token`: `UnrepresentableToken`, field
  `"id"`.
- R-D3.8 `red_guard_on_multi_instance_host` (emit.rs) and the
  multi-instance-host axis in `guard_red_axes_refuse_at_parse_or_lint`
  (refactor.rs, lint path) — written UP FRONT this tranche (D2's
  equivalent TimerWait-host red was missed at freeze time and only added
  as a review correction; not repeated here). No new emit/lint code was
  needed: `GuardOnUnsupportedHost`'s `!matches!(ServiceTask)` host check is
  generic over `IRNode` kind — only the fixtures were new.
- R-D3.9 `red_multi_instance_non_empty_inputs_unrepresentable`: exact
  `InputsUnrepresentable { id, count: 1 }` — the freeze §2 (b) ruling's own
  enforcement mechanism.
- R-D3.4 (`:next` unknown / targets a guard) owned by existing lint checks —
  no new mechanism, covered by D1's fixtures (same disposition as D2).

**Named cement update:**
`red_unsupported_node_all_remaining_kinds` (emit.rs): the `MultiInstance`
row removed — it joined the core; 6 kinds remain.

## Surfaced, NOT decided — a pre-existing, cross-cutting gap this tranche made visible

Written up front, not discovered by review: while building the D3 REST
endpoint witness, a REALISTIC designer-admitted MI graph (one that
satisfies G7.4's requirement that `collection_flag_name` name a declared
`IRNode::DataObject`) turned out to **refuse at DSL emission every time**,
via `UnreachableNode` naming the DataObject — not any MI-specific error.

Root cause, verified against the primary sources, not inferred:
- `Operation::CreateDataObject` inserts with "no anchor, no edge" (its own
  doc comment) — a `DataObject` node is **permanently edgeless** in the
  graph model (mirrors `ir_plan.rs`'s "structural-only, zero bytecode").
- `emit_dsl`'s Stage-0 reachability check (D1.0 §3.1, unchanged by D2 or
  D3) is a flow-DFS from `Start` with **no exemption** for structural
  declaration nodes — so any graph containing a `DataObject` refuses with
  `UnreachableNode` regardless of what (if anything) legitimately
  references it.

This is **not a D3 defect**: the DFS has refused every DataObject-containing
graph since B1, predating both G7.4 (which mandates the DataObject for MI)
and MultiInstance's move into the emission core. D3 is simply the first
tranche where a *core, emittable* node kind has a *mandatory* dependency on
a structurally-unreachable node kind, making the collision practically
unavoidable rather than incidental.

Practical consequence: the green MI-emits-to-DSL path this tranche opens is
reachable only for a hand-built `IRGraph` that skips the G7.4 admission gate
(the unit tests and the B2 G16 fixture correctly do this — they are honest
about testing the bridge contract, not full graph-admission legality, same
convention G13-G15 already used). It is **not** reachable for any session
that went through the real `/graph-edit` REST admission path with a
verifier-legal MI node. The endpoint witness
(`test_dsl_receipt_multi_instance_graph_refuses_on_unreachable_data_object`)
proves this refusal directly rather than asserting a green flip that
doesn't exist for graphs built the real way — a receipt for the actual
current behavior, not a fabricated success case.

Needs its own ruling — candidates, not decided here: (a) a Stage-0
reachability exemption for `DataObject` nodes specifically (structural
declarations are not "flow", so "reachable from Start" may be the wrong
test for them), or (b) rethink whether `:collection` should reference a
DataObject at all on the DSL path. Recommend addressing before any future
DSL-parity gate that similarly requires a DataObject reference (there may
be others), rather than per-gate workarounds.

## Public-API baseline

Gate flagged drift; baseline regenerated. Diff: **+4 lines, −0 removals**:
- `pub bpmn_lite_compiler::dsl::NodeAst::MultiInstance(...)`
- `pub bpmn_lite_compiler::dsl::DslEmitError::InputsUnrepresentable` (+2 fields: `count`, `id`)

(`MultiInstanceAst` itself is pub-in-private-module, not in the baseline —
same as D1/D2's structs.)
- **Consumer:** `designer-graph` (B2), `bpmn-lite-authoring` (diagnostics),
  `bpmn-lite-server-designer` (dsl-receipt endpoint).
- **Owning facade:** `bpmn_lite_compiler::dsl`, unchanged.
- **Stability contract:** form frozen by the ratified D3.0 doc.
- **Reason:** minimum public surface for MultiInstance DSL representability
  plus its one new named refusal.

## Verification sweep (all green before commit)

- `cargo test -p bpmn-lite-compiler` — 225 passed, 0 failed (was 217; +8 D3 tests)
- `cargo test -p designer-graph` — 91 passed, 0 failed (was 90; +1: G16)
- `cargo test -p bpmn-lite-server-designer` — 99 passed, 0 failed, 1 ignored (was 98; +1 endpoint witness)
- `cargo test -p bpmn-lite-authoring` — 69 passed, 0 failed (untouched)
- `cargo check --workspace --all-targets` — 0 errors
- `scripts/check-semantic-gameboard-boundaries.py` — pass (after baseline regen)
- `scripts/check-test-only-pub.py` — pass

## STOP

D4 (loop provenance IR carriage, fork E) begins only after Adam accepts this
gate — which includes ruling on the surfaced DataObject-reachability gap
above (or explicitly deferring it to its own tranche).
