# D2.0 Freeze — TimerWait DSL form (EOP-PLAN-DSL-PARITY-001)

**Status:** RATIFIED (Adam, 2026-08-13, "go" in-session after the freeze was
presented at the D2 open; status line flipped during the D2 blind-review
corrections — the ratification itself predates D2 code).
**Scope:** make `IRNode::TimerWait` DSL-representable; `emit_dsl` unsupported set 8 → 7.

## 1. Frozen grammar

```
(timer-wait :id w (:duration-ms N | :deadline-ms N | :cycle-ms N :max-fires M) :next n)
```

- Head `timer-wait`, top-level node form (ordinary sequence node, like
  `message-wait`).
- Timer shape: EXACTLY the D1 machinery — same three shape keywords, same
  exactly-one-shape rule, same named parse errors ("more than one timer shape",
  "not a valid non-negative integer" / "not a valid u32 integer") via the same
  parser helpers (`parse_kw_u64`/`parse_kw_u32`). The positional caveat recorded
  as D1 amendment 4 applies identically.
- NO `:interrupting` — that is guard semantics; a sequential wait has none.
- NO `:budget` — `WaitExecNode` carries no budget field; inventing one would be
  scope creep past the projection contract.
- All three `TimerSpec` shapes are representable. Parity is the mandate: the graph
  path admits all three (`ir_plan.rs` projects any spec; the WS-D D1 cement proves
  standalone TimerWait projects first-class), so the DSL path represents all three.

## 2. Lowering and emission

- **AST:** `NodeAst::TimerWait(TimerWaitAst { id, spec: TimerSpec, next, span })`.
- **Linter:** lowers to the EXISTING `ExecutionNode::Wait(WaitExecNode { id, spec,
  next, span })` — the same exec node `ir_plan` produces, so plan equality is
  field-identical by construction. `check_next_ref` applies (including the D1
  guard-target refusal).
- **Unroll / printer / AstMutator / diagnostics:** ordinary-node arms, mirroring
  `message-wait` (retarget `next`, id remap through `id_map`, ToSexpr prints the
  frozen form, fixpoint-proven).
- **Emitter:** `TimerWait` leaves the unsupported set. Ordinary canonical-scan
  member (no special placement). Emission arm: id token check → exactly one
  outgoing flow edge (mirror of `ir_plan`'s `single_successor`) → unconditioned
  edge → print frozen form. `TimerWait` may host boundary guards? NO — guards
  attach to ServiceTask hosts only (D1 frozen; the `GuardOnUnsupportedHost`
  MECHANISM covers a TimerWait host). ~~and its red test exists~~ **CORRECTION
  (D2 blind review):** no TimerWait-host red test existed at ratification — the
  only emit-side red used a MessageWait host, the only lint-side red used a
  Start host. Both TimerWait-host reds were added as a D2 review correction
  (`red_guard_on_timer_wait_host` in emit.rs; the timer-wait-host axis in
  refactor.rs's `guard_red_axes_refuse_at_parse_or_lint`).

## 3. Refusal axes (red fixtures)

| # | Axis | Owner |
|---|---|---|
| R-D2.1 | double timer shape | parse (named, positional per D1 am. 4) |
| R-D2.2 | malformed integer | parse (named — never silent-zero) |
| R-D2.3 | missing shape entirely | parse (expected-keyword) |
| R-D2.4 | `:next` unknown / targets a guard | lint (existing checks) |
| R-D2.5 | emit: out-degree 0 and 2 | emit (`UnsupportedTopology`-family, whichever B0-catalogue variant fits; deviation recorded if a new variant is needed) |
| R-D2.6 | emit: conditioned outgoing edge | emit |
| R-D2.7 | emit: non-token id | emit (`UnrepresentableToken`) |

No NEW semantic refusals (e.g. `max-fires 0`): neither path validates it today, and
a DSL-only refusal would break emit-green ⇒ recompile-green on a graph the graph
path admits.

## 4. Green fixtures (B2 four-proof harness)

- G13: duration wait in a linear flow.
- G14: date (deadline) wait.
- G15: cycle+max-fires wait, round-tripping both integers.
- Fixpoint cement in `refactor.rs` covering all three shapes.

## 5. Surfaced, NOT decided (separate rulings)

- **`max_fires: 0` is unvalidated on BOTH paths** (verifier, ir_plan, lint — no
  site refuses it; a zero-fire cycle timer's runtime meaning is undefined). Same
  disposition class as `parse_loop` silent-zero: pre-existing, symmetric, needs
  its own ruling — fixing it only on the DSL side would create path asymmetry.

## STOP

Ratify to begin D2 code.
