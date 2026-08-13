# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D2: TimerWait vertical

**Status:** awaiting acceptance
**Branch:** `codex/bpmn-gameboard-refactor`
**Freeze:** `docs/receipts/EOP-DSL-PARITY-001-D2.0-freeze.md` (ratified) — implemented
verbatim; NO amendments were needed (the freeze survived implementation untouched).

## What was built

`IRNode::TimerWait` is DSL-representable end-to-end; `emit_dsl`'s unsupported set
drops from 8 kinds to 7 (`GatewayXor`, `GatewayInclusive`, `HumanWait`, `DataObject`,
`FfiServiceTask`, `SendTask`, `MultiInstance`).

Frozen grammar, implemented exactly:

```
(timer-wait :id w (:duration-ms N | :deadline-ms N | :cycle-ms N :max-fires M) :next n)
```

All three `TimerSpec` shapes representable (parity mandate); no `:interrupting`, no
`:budget`; the timer-shape grammar is now a SHARED parser helper
(`parse_timer_shape`) used by both `boundary-timer` and `timer-wait`, so the
exactly-one-shape rule, named errors, and the D1-amendment-4 positional caveat are
one mechanism, not two copies.

## Per-layer changes

| Layer | File | Change |
|---|---|---|
| AST | `dsl/ast.rs` | `NodeAst::TimerWait(TimerWaitAst { id, spec, next, span })` |
| Parser | `dsl/parser.rs` | `parse_timer_shape` factored out of `parse_boundary_timer` (behaviour-identical, head-parameterised messages); `parse_timer_wait`; head dispatch |
| Printer/Mutator | `dsl/refactor.rs` | ToSexpr prints the frozen form; ordinary-node arms in `rewire_next`/`insert_after` |
| Unroll | `dsl/unroll.rs` | retarget + iteration-clone arms (id via `id_map`, next via `remap_next`) |
| Linter | `dsl/linter.rs` | lowers to the EXISTING `ExecutionNode::Wait(WaitExecNode)` — the same exec node `ir_plan` projects to, so plan equality is field-identical by construction; `check_next_ref` (incl. the D1 guard-target refusal) applies |
| Emitter | `dsl/emit.rs` | `TimerWait` joins the core: token check → `single_out_edge` → `uncond_next` → frozen form; out-of-core arm now 7 kinds; module doc updated |
| Diagnostics | `bpmn-lite-authoring/src/diagnostics_executor.rs` | ordinary-node arms |
| B2 harness | `designer-graph/src/b2_roundtrip_receipts.rs` | G13–G15 |

## Red→green trace

**Green (B2 four-proof harness — hash witness, byte-idempotence, print→parse→print
fixpoint, span-stripped plan equality — all passed FIRST RUN):**
- G13 duration wait; G14 date (deadline) wait; G15 cycle wait round-tripping BOTH
  integers (`interval_ms`, `max_fires`).
- `timer_wait_forms_print_reparse_roundtrip_and_recompile` (refactor.rs): all three
  shapes, fixpoint + compile.
- `green_timer_wait_all_three_shapes` (emit.rs): exact frozen forms asserted byte-wise
  (`(timer-wait :id w-dur :duration-ms 1000 :next w-date)` etc.), recompile through
  derived registry, idempotence.

**Red (all exact-variant or discriminating-needle, never bare `is_err`):**
- R-D2.1 `timer_wait_red_axes_refuse_at_parse`: double shape ("timer-wait carries
  more than one timer shape"), malformed u64 ("not a valid non-negative integer"),
  malformed u32 ("not a valid u32 integer"), missing shape ("timer-wait requires
  exactly one timer shape").
- R-D2.5 `red_timer_wait_out_degree_zero_and_two` — `WrongOutDegree` with count 0
  and 2, id asserted.
- R-D2.6 `red_timer_wait_conditioned_edge` — `UnrepresentableCondition`.
- R-D2.7 `red_timer_wait_bad_id_token` — `UnrepresentableToken`, field `"id"`.
- R-D2.4 (`:next` unknown / targets a guard) is owned by the existing lint checks —
  no new mechanism, covered by D1's fixtures.

**Named cement updates (the only two prior tests touched):**
1. `red_unsupported_node_all_remaining_kinds` (emit.rs): TimerWait row removed —
   it joined the core; 7 kinds remain.
2. `red_refusal_leaves_identity_untouched` (b2): the refusal VEHICLE was TimerWait;
   the invariant (refusal ⇒ unchanged `graph_state_hash`) is kind-agnostic, so the
   vehicle is now `HumanWait`. Assertion strength unchanged.

## No new semantic refusals

Per the freeze: `max_fires: 0` remains admitted on BOTH paths (neither verifier,
ir_plan, nor lint validates it) — a DSL-only refusal would break emit-green ⇒
recompile-green. The symmetric gap stays surfaced in the D2.0 freeze §5 for a
separate ruling.

## Public-API baseline

Gate flagged drift; baseline regenerated. Diff: **+1 line, −0 removals** —
`pub bpmn_lite_compiler::dsl::NodeAst::TimerWait(...)`. (`TimerWaitAst` itself is
pub-in-private-module, not in the baseline — same as the D1 structs.)
- **Consumer:** `designer-graph` (B2), `bpmn-lite-authoring` (diagnostics).
- **Owning facade:** `bpmn_lite_compiler::dsl`, unchanged.
- **Stability contract:** form frozen by the ratified D2.0 doc.
- **Reason:** minimum public surface for TimerWait DSL representability.

## Verification sweep (all green before commit)

- `cargo test -p bpmn-lite-compiler` — 212 passed, 0 failed (was 206; +6 D2 tests)
- `cargo test -p designer-graph` — 90 passed, 0 failed (was 87; +3: G13–G15)
- `cargo test -p bpmn-lite-server-designer` — 97 passed, 0 failed, 1 ignored
- `cargo check --workspace --all-targets` — 0 errors
- `scripts/check-semantic-gameboard-boundaries.py` — pass (after baseline regen)
- `scripts/check-test-only-pub.py` — pass

## STOP

D3 (MultiInstance — D3.0 freeze surfaces the per-element `inputs` question) begins
only after Adam accepts this gate.
