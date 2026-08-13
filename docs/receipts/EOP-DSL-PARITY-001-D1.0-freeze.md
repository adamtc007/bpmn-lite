# EOP-PLAN-DSL-PARITY-001 — D1.0 contract freeze (boundary guards)

Baseline: Gate D0 accepted at `6f0d2c6`. **Paper freeze, gated like B0 —
no D1 code before this gate accepts.** Amendments found during
implementation come back here, per the B0 amendment rule.

Ground truth this freeze rests on (verified, not assumed):
- The DSL path produces `TaskExecNode.guards: Vec::new()` today
  (linter.rs:438) — no grammar, no lowering exists.
- `project_ir` consumes `guards_by_host` in the **ServiceTask arm only**
  (ir_plan.rs:267); a guard on any other host is `GuardHostUnprojected`.
  D1 mirrors this host restriction exactly — widening it is a separate
  capability decision, not this tranche's.
- Both paths must produce the SAME `GuardExecSpec { guard_id, trigger,
  failure_budget, escape_entry }`, guard-id-sorted per host (D0's
  canonical rule) — that identity IS the plan-equality proof.
- `validate_dag`/lowering already accept plans whose guard escape
  subgraphs hang off `escape_entry` (WS-D landed with tests) — the DSL
  path reuses that machinery unchanged.

## 1. Grammar (frozen; fork-C rules: new heads, additive only)

Two new top-level node heads — top-level with a `:host` reference, NOT
host-nested, mirroring the graph model (guard nodes are `attached_to`-
decorated siblings) and keeping the flat node-list shape `ToSexpr`
already prints:

```text
(boundary-timer :id <sym> :host <sym>
                ( :duration-ms <int>
                | :deadline-ms <int>
                | :cycle-ms <int> :max-fires <int> )   ; exactly one shape
                :interrupting true|false               ; REQUIRED, no default
                [:budget <int>]                        ; optional u32; range owned by verifier (0 refused at admission, G5)
                :next <sym>)                           ; escape-flow entry

(boundary-error :id <sym> :host <sym>
                [:error-code "<str-lit>"]              ; free text → str-lit
                [:budget <int>]
                :next <sym>)
```

- `:interrupting` is REQUIRED on `boundary-timer` — an implicit default
  would silently choose interrupting-vs-rearming semantics (the exact
  G5-era distinction `attach_guard`/`attach_rearming_guard` make
  explicit). Parse error if absent.
- Exactly one timer shape; mixing (e.g. `:duration-ms` + `:cycle-ms`) is
  a parse error naming both attributes.
- `:id`/`:host`/`:next` are Symbol tokens; `:error-code` is a str-lit
  (legitimate free text); integers are plain symbols lexing as digits
  (the lexer has no numeric kind — B1-review fact) validated to u64/u32
  at parse.
- AST: two new `NodeAst` variants (`BoundaryTimer(BoundaryTimerAst)`,
  `BoundaryError(BoundaryErrorAst)`), `ToSexpr` impls printing exactly
  the forms above, print→parse→print fixpoint cement mandatory.

## 2. Linter lowering (frozen)

- Guard forms are NOT plan nodes — no `ExecutionNode`; they lower to
  `GuardExecSpec` pushed onto the host's `TaskExecNode.guards`, then
  each host's Vec sorts by guard id (both paths sort — D0).
- `trigger`: `GuardTriggerExec::Timer { spec, interrupting }` /
  `Error { error_code }` — field-identical to `ir_plan`'s construction.
- `escape_entry` = the guard's `:next`, validated by the existing
  `check_next_ref`.
- New lint refusals (LintError, DSL-side analogues of `IrPlanError`'s
  guard axes): `:host` references an unknown node; `:host` resolves to a
  non-Task node (message-wait/split/etc. — mirror of
  `GuardHostUnprojected`); guard id collides with any node id or other
  guard id (plan nodes are id-keyed and `guard_id` must be workspace-
  unique for budget-key resolution — same reason `DesignerDag`
  fail-closes duplicates).
- Escape subgraph nodes are ordinary plan nodes; `validate_dag` and
  `analyze_safety` apply to them unchanged.

## 3. Emission (frozen — the B0-catalogue and canonical-form amendments)

1. **Stage-0 reachability redefinition** (amends B0's `UnreachableNode`
   rule): reachable = fixpoint of { flow-DFS from `Start` } ∪ { guard
   nodes whose `attached_to` host is reachable } ∪ { flow-DFS from each
   such guard's escape edge }. A guard on an unreachable host, or an
   escape island no guard reaches, still refuses `UnreachableNode`.
2. **Canonical order amendment** (B0 §canonical-form): guard nodes have
   no incoming flow edge, so plain Kahn would misplace them as roots.
   Frozen rule: the topological scan covers flow+escape edges; guard
   nodes are EXCLUDED from the initial ready set and become ready
   immediately after their host emits; a host's guards emit directly
   after it, ordered by guard id; escape-successor in-degrees count the
   guard's escape edge normally. Deterministic: same content → same
   order.
3. **Stage-1 arms**: `BoundaryTimer`/`BoundaryError` leave the
   `UnsupportedNode` `|`-pattern (the no-wildcard match forces the
   catalogue row deletion consciously). Per-guard checks in the frozen
   per-node order: token checks (id; host; error-code exempt — str-lit);
   **new refusal `GuardOnUnsupportedHost { guard_id, host, host_kind }`**
   (host missing, out-of-core, or not a ServiceTask — B0-catalogue
   amendment); escape out-degree exactly 1 (existing `WrongOutDegree`,
   expected 1); condition on the escape edge → existing
   `UnrepresentableCondition`.
4. Interrupting bool and timer shape emit verbatim; budget emits only
   when `Some` (`:budget`), `None` = attribute absent (inherits the
   workflow default — which remains `ProcessDeclUnrepresentable` if set,
   unchanged by this tranche).

## 4. Plan-equality delta (amends the B0 equality table)

`TaskExecNode.guards` moves from "empty both sides" to **compared,
ordered** (both paths guard-id-sorted); every `GuardExecSpec` field
compared; no new exclusions — `span` remains the only excluded field.

## 5. Fixtures (B2 harness additions)

Greens: G8 interrupting duration-timer guard, escape task→end; G9
non-interrupting (rearming) cycle timer with `:max-fires`; G10 error
guard with code + budget; G11 two error guards on one host (guard-order
canonicality through the full round trip, extending D0's projection-side
cement); G12 escape chain containing a task (escape subgraph is ordinary
flow). Reds: R25 guard on a `MessageWait` host
(`GuardOnUnsupportedHost`); R26/R27 escape out-degree 0 and 2; R28
conditioned escape edge; R29 duplicate guard id (DSL lint side);
R30 timer-shape ambiguity and missing `:interrupting` (parse errors).
Plus: the B3 endpoint's guard test flips from refusing to green —
rewritten as a named cement update in the same commit.

## 6. Explicitly out of scope for D1

Guards on non-ServiceTask hosts (host-restriction widening); XML
importer changes; kernel/lowering changes (none needed — `GuardExecSpec`
consumption already exists); the workflow-default budget syntax
(fork-G backlog, unchanged).

**STOP-gate: this freeze awaits ratification. D1 code begins only after
acceptance.**
