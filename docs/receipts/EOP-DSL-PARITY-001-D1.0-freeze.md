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
  at parse. **A malformed integer is a NAMED parse error** — this is a
  deliberate invention over the nearest precedent (`parse_loop` silently
  turns `:ceiling 10x` into ceiling **0**, parser.rs:490-491 — a real
  trap door, surfaced by this freeze's blind review as a standalone
  pre-existing defect for a separate ruling, not fixed by D1).
- `:interrupting` accepts exactly the Symbol tokens `true`/`false`; any
  other symbol is a NAMED parse error. (No boolean attribute exists
  anywhere in the grammar today — this is a new convention, stated as
  such, per the review.)
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
- **Three axes added after blind review** (the DSL pipeline is
  `unroll → lint → validate_dag` ONLY — the verifier and admission never
  run on it, so every graph-path refusal must be re-owned by LINT or it
  silently defers to lowering-time failure):
  1. `:budget 0` refuses at lint. The freeze's first draft said "range
     owned by verifier (0 refused at admission)" — **both halves false
     on the DSL path**: the verifier has no budget-range check at all
     (`failure_budget: _` ignored), and the admission-time zero refusal
     covers only `SetDefaultGuardBudget`. Without a lint check, `:budget
     0` compiles green and dies at `lower_plan` — refusal moved to lint.
  2. A second TIMER guard on one host refuses at lint (mirror of
     verifier §7d, which never runs here; today it would only surface at
     lowering's `validate_guards`). Multiple ERROR guards remain legal.
  3. `:cycle-ms` + `:interrupting true` refuses at lint (an interrupting
     cycle timer is contradictory — rearm-only; today refused only at
     lowering, frontend.rs:541-546).
- Escape-into-a-split-branch contract (review question, answered): the
  shape yields a `BPMN_NON_SESE_TOPOLOGY` proof BREACH via
  `analyze_safety`, not a refusal — identical on both paths (same shared
  `analyze_safety` in the plan constructor), so plan equality is
  unaffected; frozen as breach-not-refusal, matching the graph path.
- Escape subgraph nodes are ordinary plan nodes; `validate_dag` and
  `analyze_safety` apply to them unchanged.

## 3. Emission (frozen — the B0-catalogue and canonical-form amendments)

1. **Stage-0 reachability redefinition** (amends B0's `UnreachableNode`
   rule): reachable = fixpoint of { flow-DFS from `Start` } ∪ { guard
   nodes whose `attached_to` host is reachable } ∪ { flow-DFS from each
   such guard's escape edge }. A guard on an unreachable host, or an
   escape island no guard reaches, still refuses `UnreachableNode`.
1b. **Stage-0 cycle check runs over the EFFECTIVE graph** (amended after
   blind review, which found the serious hole here): flow edges ∪
   escape edges ∪ one implicit host→guard edge per attachment — exactly
   the graph the amended scan below walks, and exactly the adjacency
   `validate_dag` already uses (`build_adjacency` appends
   `guard_escape_entries()` to the host's successors). Without this, an
   escape edge back into the guard's own host/ancestor (acyclic to
   plain toposort, since `attached_to` is a field, not an edge) passes
   stage-0 and DEADLOCKS the scan — and `canonical_order` today has no
   totality check, so it would silently return a partial order: a
   fail-open truncation on the emit side of shapes `validate_dag`
   refuses on the plan side. Additionally frozen: a **hard totality
   assert** — `order.len() == node_count` or refuse (`CyclicGraph`
   witness from the effective graph), never truncate.
2. **Canonical order amendment** (B0 §canonical-form): guard nodes have
   no incoming flow edge, so plain Kahn would misplace them as roots.
   Frozen rule: the topological scan covers the effective graph of 1b;
   guard nodes are EXCLUDED from the initial ready set and become ready
   immediately after their host emits (the implicit host→guard edge); a
   host's guards emit directly after it, ordered by guard id;
   escape-successor in-degrees count the guard's escape edge normally.
   Deterministic and — by 1b — total: same content → same complete
   order. (Implementation note, per review: this changes the ready-set
   mechanism from the current single BTreeMap tie-break; the totality
   assert is the guard rail while doing so.)
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
R30 timer-shape ambiguity and missing `:interrupting` (parse errors);
**added after blind review:** R31 `:budget 0` (lint); R32 second timer
guard on one host (lint); R33 interrupting cycle timer (lint); R34
malformed integer (`:budget 1x`, named parse error); R35
`:interrupting maybe` (named parse error); R36 escape edge back into
the guard's own host/ancestor (emission `CyclicGraph` from the
effective graph — the fail-open shape 1b closes, proven refusing not
truncating).
Plus: the B3 endpoint's guard test flips from refusing to green —
rewritten as a named cement update in the same commit.

## 6. Explicitly out of scope for D1

Guards on non-ServiceTask hosts (host-restriction widening); XML
importer changes; kernel/lowering changes (none needed — `GuardExecSpec`
consumption already exists); the workflow-default budget syntax
(fork-G backlog, unchanged).

## 7. Blind peer-review findings and dispositions

An independent reviewer (no prior context) verified the three
ground-truth claims (linter's single empty-guards site; ServiceTask-only
consumption with the fail-closed leftover sweep; `validate_dag` passing
escape subgraphs via `build_adjacency`'s host→escape appending, with the
WS-D cement tests located) and the full plan-equality delta (including
tracing `interrupting` from `AttachGuard`/`AttachRearmingGuard` through
`ops.rs` to prove both paths carry the same bool). Five findings, all
disposed by amendment above, none by argument:
1. **Kahn totality hole (serious)** — escape-to-own-ancestor shapes
   deadlock the amended scan, and `canonical_order` has no totality
   check, so emission would silently TRUNCATE shapes `validate_dag`
   refuses. Disposed: §3.1b (effective-graph cycle check + hard
   totality assert), fixture R36.
2. **Budget-0 ownership claim refuted** — the verifier has no range
   check and never runs on the DSL path anyway. Disposed: lint owns it
   (§2 axis 1), fixture R31.
3. **Two-timers-per-host and interrupting-cycle axes missing** —
   verifier §7d / frontend `validate_guards` never run at DSL compile.
   Disposed: lint owns both (§2 axes 2-3), fixtures R32/R33.
4. **Integer convention was an unlabelled invention** — the nearest
   precedent (`parse_loop`) silently zeroes malformed ceilings.
   Disposed: named parse error frozen, fixture R34, and the
   `parse_loop` silent-zero trap door surfaced as a standalone
   pre-existing defect for a separate ruling.
5. **`:interrupting` boolean under-specified** (no boolean precedent
   exists in the grammar). Disposed: exact `true`/`false` symbols,
   named parse error otherwise, fixture R35.
The reviewer also answered the escape-into-branch contract question
(breach-not-refusal via shared `analyze_safety`, both paths identical) —
frozen in §2.

**STOP-gate: this freeze awaits ratification. D1 code begins only after
acceptance.**
