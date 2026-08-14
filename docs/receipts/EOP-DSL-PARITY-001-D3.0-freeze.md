# D3.0 Freeze — MultiInstance DSL form (EOP-PLAN-DSL-PARITY-001)

**Status:** RATIFIED (Adam, 2026-08-14, "(b) go" — §2 ruled (b) wave-1
exclusion with `InputsUnrepresentable` refusal; §1's `name == id` and R-D3.8
up-front recommendations stand as no objection was raised to them).
**Scope:** make `IRNode::MultiInstance` DSL-representable; `emit_dsl` unsupported
set 7 → 6.

## 0. Source of truth (verified against code, not memory)

`IRNode::MultiInstance` (`bpmn-lite-compiler/src/ir.rs:187-221`):

```rust
MultiInstance {
    id: String,
    name: String,
    task_type: String,             // inner activity dispatch identity (ServiceTask convention)
    collection_flag_name: String,  // flag naming the Value::Array data object
    declared_max: u32,             // required ceiling — ruling K deviation from Zeebe, not an oversight
    #[serde(default)]
    inputs: Vec<FfiInputBinding>,  // G4.0/G4.1 — per-element input bindings
}
```

`FfiInputBinding { target_field: String, expression: Expression }`;
`Expression = Literal(IrLiteral) | VarRef(Vec<String>)`.

The plan-level projection already exists (G5.4a,
`bpmn-lite-compiler/src/dsl/plan.rs:524`, `MultiInstanceExecNode`) and is
narrower than the IR node:

```rust
pub struct MultiInstanceExecNode {
    pub id: String,
    pub task_type: String,
    pub collection_flag_name: String,
    pub declared_max: u32,
    pub next: String,
    pub span: Option<SourceSpan>,
}
```

Confirmed by reading the struct directly: **no `name`, no `inputs` field on
the exec node.** `inputs` is authoring-time/manifest-derivation data only —
`lower()` never reads it; MI element delivery at runtime is the synthesized
`{node_id}_mi_element_{index}` flag, unrelated to `inputs`. This matters for
§2 below: whatever the DSL AST carries for `inputs`, it is carried for
round-trip fidelity only, never for lowering — the linter's projection to
`MultiInstanceExecNode` mirrors the exec node's own field set and drops
`inputs` regardless of which option is chosen.

## 1. Frozen grammar (mandatory fields)

```
(multi-instance :id m :task-type "review-doc" :collection @docs :max 50 :next n)
```

- Head `multi-instance`, ordinary top-level sequence node (like
  `message-wait`/`timer-wait`) — no split/join pairing, matching the exec
  node's single `next`.
- `:task-type` — string token, same convention as `ServiceTask`/`TaskAst.plug`
  (the "which activity dispatch identity" field). Free-form string, not a
  `Symbol` — task types are dispatch strings, not DSL identifiers (mirrors
  how `TaskAst.plug` is already parsed).
- `:collection` — a flag reference, `@name` token (mirrors the `@flag`
  convention `ConditionExpr` already uses for XOR/inclusive-gateway
  conditions — reused, not invented). Resolves to `collection_flag_name`
  (the `@` is stripped, same as guard/condition parsing does elsewhere).
- `:max` — non-negative integer token, `declared_max` (u32). Parsed via the
  existing `parse_kw_u32` helper (same one D1/D2's `:max-fires` uses) —
  named error on overflow/malformed, never silent-zero.
- `:next` — ordinary next-node reference, existing `check_next_ref` machinery
  applies (D1's guard-target refusal included).
- `name` (`IRNode::MultiInstance.name`) — **NOT represented.** No sibling DSL
  node carries a separate display `name` distinct from `id` today (`TaskAst`
  has none either); adding one here would be new grammar surface with no
  existing convention to anchor it to. Recommend: DSL-authored MI nodes get
  `name == id` at emission/projection (mirrors how `TaskAst`/`WaitExecNode`
  already have no `name` field at all — `MultiInstance` is unusual in
  carrying one). **Surfaced as a fork below (§5), not decided here.**

## 2. The per-element `inputs` question — Adam's ruling

`inputs: Vec<FfiInputBinding>` is the one field with no existing DSL grammar
analog: `TaskAst.args` is `Vec<(String, String)>` (flat string pairs, no
`Expression` type — no `Literal`/`VarRef` distinction exists anywhere in the
DSL grammar today). Representing `inputs` faithfully means introducing
`Expression` as DSL grammar for the first time. Two options:

**(a) New nested sub-form**, e.g.:
```
(multi-instance :id m :task-type "review-doc" :collection @docs :max 50
  :inputs ((:target "doc_id" :expr (var element doc_id))
           (:target "priority" :expr (lit i64 3)))
  :next n)
```
Requires: a new `Expression`-shaped grammar (two more sub-forms, `var`/`lit`,
each with their own red axes), a new `AstMutator`/printer/unroll arm for the
nested list, and — since the exec node never reads `inputs` — this entire
sub-form exists purely for round-trip fidelity (graph MI node → DSL text →
recompile must reproduce it byte-for-byte per B2, even though the compiled
plan is unaffected). Real work for zero behavioral payoff at this gate.

**(b) Explicit wave-1 exclusion.** DSL representability covers `inputs ==
[]` only. A graph-authored MI node with non-empty `inputs` REFUSES at DSL
emission — fail closed, never silently drop the bindings — via a NEW named
`DslEmitError` variant (mirrors `ProcessDeclUnrepresentable`'s existing
precedent: "the grammar has no syntax to carry this field, refuse rather
than drop it"):
```rust
#[error("node '{id}' carries {count} per-element input binding(s), but the DSL grammar has no syntax to represent them — refusing rather than silently dropping them")]
InputsUnrepresentable { id: String, count: usize },
```
On the DSL→graph direction, a DSL-authored `multi-instance` node simply has
no `inputs` field to parse — always empty, no refusal needed there (nothing
to lose).

**Recommendation: (b).** `inputs` is authoring-time metadata the exec node
itself discards; spending a new `Expression` grammar surface on round-trip
fidelity for a field with zero runtime effect is scope creep past what D3 is
for (closing the *emission* gap, not inventing a general expression
language). If per-element bindings need DSL authoring later, that is its own
tranche once a concrete caller needs it — not speculative now. (b) also
composes cleanly with the parity mandate: it fails closed and is honestly
named, not a silent gap.

**This is Adam's call, not mine — ruling requested at freeze ratification.**

## 3. Lowering and emission

- **AST:** `NodeAst::MultiInstance(MultiInstanceAst { id, task_type,
  collection_flag_name, declared_max, next, span })` — no `inputs` field if
  (b) is ruled; `inputs: Vec<...>` field if (a).
- **Linter:** lowers to the EXISTING `ExecutionNode::MultiInstance
  (MultiInstanceExecNode)` — same exec node `ir_plan`'s G5.4a projection
  produces, so plan equality is field-identical by construction (same
  pattern as D2's `WaitExecNode` reuse). `check_next_ref` applies.
- **Unroll / printer / AstMutator / diagnostics:** ordinary-node arms,
  mirroring `message-wait`/`timer-wait` (retarget `next`, id remap through
  `id_map`, ToSexpr prints the frozen form, fixpoint-proven). No
  split/join-pair bookkeeping — MI is a single sequence node at the DSL
  level, same as the exec node.
- **Emitter:** `MultiInstance` leaves the unsupported set (7 → 6: remaining
  `GatewayXor | GatewayInclusive | HumanWait | DataObject | FfiServiceTask |
  SendTask`). Emission arm: id token check → `task_type` token/string check
  → `collection_flag_name` token check (must lex as `@name`) → exactly one
  outgoing flow edge (mirror `single_out_edge`) → unconditioned edge → print
  frozen form. If (b): `inputs.is_empty()` check before emission, else
  `InputsUnrepresentable`.
- **MultiInstance as a boundary-guard host?** NO — same as TimerWait (D2
  ratified precedent): guards attach to ServiceTask hosts only.
  `GuardOnUnsupportedHost` mechanism covers an MI host; a red test for it
  is REQUIRED at D3 code time, added up front — not deferred to a review
  correction like D2's TimerWait-host gap was.

## 4. Refusal axes (red fixtures)

| # | Axis | Owner |
|---|---|---|
| R-D3.1 | missing `:task-type` / `:collection` / `:max` / `:next` | parse (named, expected-keyword) |
| R-D3.2 | `:collection` value not an `@`-prefixed token | parse (named) |
| R-D3.3 | `:max` malformed / overflow u32 | parse (`parse_kw_u32`, named — never silent-zero) |
| R-D3.4 | `:next` unknown / targets a guard | lint (existing checks, D1 precedent) |
| R-D3.5 | emit: out-degree 0 and 2 | emit (`WrongOutDegree`) |
| R-D3.6 | emit: conditioned outgoing edge | emit (`UnrepresentableCondition`) |
| R-D3.7 | emit: non-token id / task-type | emit (`UnrepresentableToken`) |
| R-D3.8 | emit: guard attached to an MI host | emit (`GuardOnUnsupportedHost`, host_kind `"MultiInstance"`) — red test written AT D3, not deferred |
| R-D3.9 | emit: non-empty `inputs` on a graph MI node | emit (`InputsUnrepresentable`) — **only if (b) is ruled** |

No NEW semantic refusal for `declared_max: 0`: parity with D1/D2's
`max-fires`/`max_fires` disposition — needs its own symmetric ruling if
raised, not invented here (see §5).

## 5. Surfaced, NOT decided (separate rulings)

- **§2 above — the `inputs` disposition itself** is the primary fork this
  freeze exists to surface. Recommendation: (b).
- **`name` field disposition** (§1): recommend DSL-authored MI nodes set
  `name == id`; the graph→DSL direction for a node whose `name != id` would
  then either silently normalize (name lost) or need its own refusal. Needs
  an explicit ruling alongside (a)/(b) above, not assumed.
- **`declared_max: 0`** — is it validated anywhere today (verifier, ir_plan,
  lint)? Not checked as part of this freeze; if unvalidated, same
  disposition class as D2.0 §5's `max_fires: 0` — pre-existing, symmetric,
  own ruling, not blocking this freeze.

## STOP

Ratify §1, §2 (the (a)/(b) choice), and §1's `name` disposition to begin D3
code.
