# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D1: boundary-guard vertical

**Status:** awaiting acceptance
**Branch:** `codex/bpmn-gameboard-refactor`
**Freeze:** `docs/receipts/EOP-DSL-PARITY-001-D1.0-freeze.md` (ratified) — this tranche
implements it verbatim; deviations are recorded below as **amendments** per the
freeze's own amendment rule.

## What was built

The full boundary-guard vertical: `BoundaryTimer`/`BoundaryError` become DSL-representable
end-to-end — grammar, parser, printer (ToSexpr), AstMutator, unroll, linter lowering,
emitter projection, B2 round-trip fixtures, and the flipped B3 endpoint expectation.
`emit_dsl`'s unsupported set drops from 10 kinds to 8 (`GatewayXor`, `GatewayInclusive`,
`TimerWait`, `HumanWait`, `DataObject`, `FfiServiceTask`, `SendTask`, `MultiInstance`).

### Grammar (frozen forms, implemented exactly)

```
(boundary-timer :id g :host t (:duration-ms N | :deadline-ms N | :cycle-ms N :max-fires M)
                :interrupting true|false [:budget N] :next esc)
(boundary-error :id g :host t [:error-code "E"] [:budget N] :next esc)
```

Top-level heads referencing their host by `:host`. `:interrupting` is REQUIRED, no
default. Malformed integers and non-`true`/`false` booleans are NAMED parse errors
(`parse_kw_u64`/`parse_kw_u32`/`parse_kw_bool` in `parser.rs` — "not a valid
non-negative integer" / "not a valid u32 integer" / "not a boolean"); exactly-one timer
shape enforced with a named "more than one timer shape" error.

### Per-layer changes

| Layer | File | Change |
|---|---|---|
| AST | `bpmn-lite-compiler/src/dsl/ast.rs` | `NodeAst::BoundaryTimer(BoundaryTimerAst)`, `NodeAst::BoundaryError(BoundaryErrorAst)` + `id()`/`span()` arms |
| Parser | `bpmn-lite-compiler/src/dsl/parser.rs` | heads + typed keyword helpers + shape/required-field enforcement |
| Printer/Mutator | `bpmn-lite-compiler/src/dsl/refactor.rs` | ToSexpr prints the exact frozen forms; AstMutator rewire/insert_after arms (inserting a guard via `insert_after` is an error) |
| Unroll | `bpmn-lite-compiler/src/dsl/unroll.rs` | `g.next` retarget; iteration clone remaps `id` and `host` through `id_map` |
| Linter | `bpmn-lite-compiler/src/dsl/linter.rs` | new Pass 3.4: lowers guard AST nodes to `GuardExecSpec` on the host `TaskExecNode.guards`; lint owns budget-0, interrupting-cycle-timer, second-timer-per-host, non-task-host, unknown-host refusals; guard-id sort per host |
| Emitter | `bpmn-lite-compiler/src/dsl/emit.rs` | guard collection (`collect_guards` by-host, guard-id-sorted), effective-graph stage 0 (flow ∪ escape ∪ implicit host→guard edges) with hard totality assert, canonical scan emits guards directly after their host, guard emission arms with ServiceTask-host / single-uncond-escape / budget / interrupting-cycle checks |
| Diagnostics | `bpmn-lite-authoring/src/diagnostics_executor.rs` | guard arms in `RemoveUnusedNode` + predecessor walk |
| B2 harness | `designer-graph/src/b2_roundtrip_receipts.rs` | fixtures G8–G12 (below) |
| Endpoint | `bpmn-lite-server-designer/src/rest.rs` | B3 guard test flipped red→green (below) |

## Red→green trace

**Green (B2 four-proof harness — hash witness, byte-idempotence, print→parse→print
fixpoint, span-stripped plan equality — all passed FIRST RUN):**
- G8 interrupting duration timer
- G9 rearming cycle timer with `:max-fires`
- G10 error guard with `:error-code` and `:budget`
- G11 two error guards on one host, guard-id order proven by source-position assertion (`g-a` before `g-b`)
- G12 escape chain routing through a task before End

**Red (emit-side, `emit.rs` tests — 26 total):**
- `red_guard_on_message_wait_host` — `GuardOnUnsupportedHost`
- `red_guard_escape_out_degree_zero_and_two` — escape must be exactly one edge
- `red_guard_conditioned_escape_edge` — conditioned escape refused
- `red_guard_escape_into_own_host_refuses_cyclic` — R36: effective-graph cycle check catches escape-to-own-host (the shape the freeze review showed would deadlock/truncate a naive Kahn)
- `red_flow_into_guard` — `FlowIntoGuard`, smallest guard-id/edge-id
- `red_guard_budget_zero_and_interrupting_cycle` — emission-side mirrors keeping emit-green ⇒ recompile-green
- `red_unsupported_node_all_remaining_kinds` — renamed from `..._all_ten_kinds`, now 8 kinds (named cement update: the two boundary kinds left the unsupported set — that is the point of the tranche)

**Red (parse/lint-side, `refactor.rs` test `guard_red_axes_refuse_at_parse_or_lint`):**
R29–R35 — missing `:interrupting`, double timer shape, malformed int, non-boolean,
budget 0, second timer per host ("already carries timer guard"), non-task host ("not a
service task"), unknown host ("references an unknown node") — all compile-driven
`expect_err` with message needles. Plus `guard_forms_print_reparse_roundtrip_and_recompile`
fixpoint + guard-order cement.

**Endpoint (B3):** `test_dsl_receipt_guard_graph_emits_boundary_timer` flipped from
asserting refusal to asserting the emitted source contains exactly
`(boundary-timer :id timeout :host review_documents :duration-ms 60000 :interrupting true :next timeout_end)`,
recompiles via the derived registry, and the graph is not mutated.

## Amendments to the D1.0 freeze (recorded per its amendment rule)

1. **Three new `DslEmitError` variants beyond the B0 catalogue + one from the freeze:**
   `FlowIntoGuard{guard_id, edge_id}` (freeze-mandated), plus `GuardOnUnsupportedHost`,
   `GuardBudgetZero`, `InterruptingCycleTimer` — the latter two are emission-side
   mirrors of lint refusals, required so emit-green ⇒ recompile-green holds (an emitted
   source the linter would refuse is a broken bridge).
2. **`:error-code` is NOT quote-exempt.** The freeze said str-lits were exempt from
   token checks; implementation refuses `"` and control characters in `:error-code` via
   `UnrepresentableToken` (field `"error_code"`) because the printer cannot escape them
   — an unescaped quote would emit source that re-parses differently. Escaping support
   is out of D1 scope; refusal is the fail-closed choice.
3. **`:next <guard-id>` defers to validate_dag.** `node_ids` includes guard ids, so a
   guard escaping to another guard is not caught at parse; it is caught by the
   `FlowIntoGuard` check on the emit side and by validate_dag's dangling/edge rules on
   the DSL side. No trap door: both paths refuse, just at different stages.

## Public-API baseline

The boundary gate (`scripts/check-semantic-gameboard-boundaries.py`) flagged drift in
`bpmn-lite-compiler`; baseline regenerated at
`docs/generated/public-api-baselines/bpmn-lite-compiler.txt` (header names this receipt).
Diff: **+13 lines, −0 removals** — the two `NodeAst` variants, `BoundaryTimerAst`/
`BoundaryErrorAst` structs+fields, and the four `DslEmitError` variants.
- **Consumer:** `designer-graph` (B2 harness), `bpmn-lite-server-designer` (session
  receipt endpoint), `bpmn-lite-authoring` (diagnostics executor).
- **Owning facade:** `bpmn_lite_compiler::dsl` (ast/parser/emit), same facade as the
  existing core-5 surface.
- **Stability contract:** forms are frozen by the ratified D1.0 doc; any future change
  is a freeze amendment, not a drive-by.
- **Reason:** D1 makes boundary guards DSL-representable; the AST/error variants are
  the minimum public surface for that.

## Verification sweep (all green before commit)

- `cargo test -p bpmn-lite-compiler` — 206 passed, 0 failed (26 emit, 8 refactor)
- `cargo test -p designer-graph` — 87 passed, 0 failed (G8–G12 included)
- `cargo test -p bpmn-lite-server-designer` — 97 passed, 0 failed, 1 ignored
- `cargo check --workspace --all-targets` — clean
- `scripts/check-semantic-gameboard-boundaries.py` — pass (after baseline regen)
- `scripts/check-test-only-pub.py` — pass

## Parked (not decided here)

- `parse_loop` silent-zero ceiling (pre-existing trap door, surfaced in D1.0) — awaiting separate ruling.
- P2 kinds (`HumanWait`, `SendTask`, `FfiServiceTask`) + `DataObject` — future programme per parity V&S Fork rulings.

## STOP

D2 (TimerWait) begins only after Adam accepts this gate.
