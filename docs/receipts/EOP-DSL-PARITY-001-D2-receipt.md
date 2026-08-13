# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D2: TimerWait vertical

**Status:** awaiting acceptance
**Branch:** `codex/bpmn-gameboard-refactor`
**Freeze:** `docs/receipts/EOP-DSL-PARITY-001-D2.0-freeze.md` (ratified). This receipt
originally claimed "implemented verbatim, zero amendments" — the blind review refuted
that: the freeze itself contained a false claim (§2 said a guard-on-TimerWait-host red
test "exists"; none did), corrected in the freeze with a marked CORRECTION block and
cemented with two new red tests (see the review disposition below).

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
- `green_timer_wait_all_three_shapes` (emit.rs): the three frozen forms asserted as
  byte-exact substrings (`(timer-wait :id w-dur :duration-ms 1000 :next w-date)` etc.);
  whole-source canonical placement is pinned indirectly via idempotence + the B2 hash
  witness, not by a full-source byte assertion. Plus recompile through the derived
  registry.

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

## Blind-review disposition

The review of the initial commit (`27604ed`) returned **ACCEPT-WITH-CORRECTIONS**
(2 MAJOR, 2 MINOR, 1 NOTE). All corrections in the follow-up commit; each verified
personally before applying:

| # | Severity | Finding | Disposition |
|---|---|---|---|
| 1 | MAJOR | `repeat.rs::find_all_predecessor_ids_rec` had a `_ => {}` wildcard silently skipping TimerWait (and D1 guard) predecessors — `repeat_n_times` returned Ok with a dangling `:next` and mis-spliced loop, reachable via the REST `apply_dsl_macro` BoundedRetry path | **Fixed**: exhaustive match, no wildcard (B0 rule); red→green cement `repeat_n_times_rewires_a_timer_wait_predecessor` (full recompile assertion) + guard-predecessor no-dangle cement |
| 2 | MAJOR | Freeze §2 falsely claimed a guard-on-TimerWait-host red test existed; receipt propagated it as "implemented verbatim, zero amendments" | **Fixed**: freeze corrected in place (marked CORRECTION block); two new reds — `red_guard_on_timer_wait_host` (emit, exact-variant with `host_kind: "TimerWait"`) and the timer-wait-host lint axis in `guard_red_axes_refuse_at_parse_or_lint`; receipt header rewritten |
| 3 | MINOR | dsl-receipt endpoint behaviour for TimerWait graphs flipped refused→green with no witness | **Fixed**: `test_dsl_receipt_timer_wait_graph_emits_timer_wait` — exact frozen form `(timer-wait :id cooldown :duration-ms 60000 :next end)`, recompile, non-mutation |
| 4 | MINOR | Freeze status line still read "awaiting ratification" | **Fixed**: flipped to RATIFIED with provenance note |
| 5 | NOTE | "asserted byte-wise" overstated the green emit assertion | **Fixed**: receipt wording corrected (byte-exact substrings; placement pinned via idempotence + B2 hash) |

**Surfaced by correction #1, NOT decided here (pre-existing, separate ruling):**
`repeat_n_times` has a multi-predecessor splice defect — only the anchor predecessor
is routed through the new loop; every other predecessor (two service-tasks in a
diamond show it; not guard-specific) is left rewired to the exit, silently bypassing
the retry. Pre-dates D2 (G3.3 vintage). The guard-predecessor cement asserts
no-dangle + recompile only and carries a do-not-strengthen note pending the ruling.
Recommendation: rewire ALL predecessors to the loop id and splice by scope injection
rather than `insert_after`-on-one-anchor; needs its own red→green tranche.

## STOP

D3 (MultiInstance — D3.0 freeze surfaces the per-element `inputs` question) begins
only after Adam accepts this gate — which now also includes the multi-predecessor
`repeat_n_times` fork above.
