# EOP-PLAN-GRAPH-DSL-BRIDGE-001 — B0 receipt

Baseline: plan accepted at `04e7764` (branch `codex/bpmn-gameboard-refactor`).
This tranche's commits: `61eaf4c` (ir_plan header fix), `32e4de0` (ToSexpr
split-head defect fix + cement tests), plus this receipt and V&S amendments.
**Tier: CAREFUL.**

- **Scope delivered:** all four B0 work items, plus one production defect
  found by the plan's own stop condition and fixed as its own gated defect
  (as the plan prescribes), plus two ground-truth refinements folded back
  into the V&S.

## Work item 1 — `ir_plan.rs` stale header (`61eaf4c`)

Doc-comment-only. The out-of-scope list wrongly named `MessageWait` and
`MultiInstance`; both project (verified at ir_plan.rs:278-294 and :381-400
before editing). Supported-list updated to match the code.

## Defect found and fixed under the stop condition (`32e4de0`)

**`ToSexpr` split output never re-parsed.** The printer emitted
`parallel-gateway`/`inclusive-gateway` heads that no parser arm accepts,
and `exclusive-gateway`, whose legacy parse fn takes a different attribute
shape (no `:join`; join id synthesized as `{id}-join` —
parser.rs:369-390). All three split modes failed round-trip. Production
impact: `AstMutator`'s regenerate-and-recompile path was broken for any
workflow containing a split (its own tests only exercised task/loop
shapes, which is why this survived).

Fix: print the modern heads (`split-xor`/`split-or`/`split-and`),
matching `parse_split`'s attribute shape exactly. Parser untouched — the
DSL grammar is the H4-locked language contract; grammar changes belong to
the parity programme. Cement tests added (print→parse→print fixpoint, no
parser errors): all three split modes + the linear core shapes
(start/task/message-wait/terminate-end); `split-and` additionally
recompiles. **Red trace:** mutation-verified — reverting the And head
makes `split_and_print_reparse_roundtrip_and_recompiles` fail; restored,
6/6 pass; full crate suite 178/0. **Corrected after blind review:** the
first version of the linear-shapes fixture quoted the message-wait
values, which the parser silently dropped via its recovery loop (the
shared `parse_sexpr` helper swallowed the errors), so the message-wait
leg was initially vacuous — helper hardened to fail on any lex/parse
error, fixture corrected to bare symbols, re-verified green. Production
impact independently confirmed by the reviewer: `to_sexpr` output is fed
straight to `dsl::compile` in the designer server's macro-apply endpoint
(`rest.rs:1706-1709`), not only in tests.

Two parser/AST asymmetries surfaced (recorded for the parity programme,
not fixed): (a) the grammar cannot express a plug-less `Xor`/`Or` split,
but the AST admits one (and the legacy `exclusive-gateway` parse fn
produces one) — such an AST prints to source the parser rejects; (b) the
grammar cannot express a condition on an And-split flow
(`parse_split_flow(require_condition=false)` never parses one), but
`ir_plan` accepts and projects Eq-conditions on And-diverging edges — a
one-directional representability gap that becomes an emission refusal
(below).

## Work item 2 — frozen `DslEmitError` catalogue

One variant per row; B1 implements exactly this list. Additions found
during B1 must come back here as amendments, per the plan's receipt rule.

| Variant | Refuses | Trigger fact |
| --- | --- | --- |
| `UnsupportedNode { id, kind }` | The 10 out-of-core `IRNode` kinds: `GatewayXor`, `GatewayInclusive`, `TimerWait`, `HumanWait`, `BoundaryTimer`, `BoundaryError`, `DataObject`, `FfiServiceTask`, `SendTask`, `MultiInstance` | Kind not in core-5. **Deviation from plan prose** ("one variant per unsupported kind"): a single kind-carrying variant, mirroring `IrPlanError::UnsupportedNode`. Fail-closed is preserved structurally: the emitter's match on `IRNode` must have NO wildcard arm, so a 16th `IRNode` variant breaks compile rather than falling through. Ten distinct variants would add no diagnostic information beyond the kind name. |
| `MissingStart` | No `Start` node | Mirrors `IrPlanError::MissingStart`; topological emission needs the unique entry. |
| `MultipleStarts { ids }` | >1 `Start` | Defence in depth (`find_start` semantics); canonical order needs a unique root. |
| `CyclicGraph { id }` | Any cycle (named witness node) | Topological emission order is undefined on a cycle. `emit_dsl` takes a raw `IRGraph`; admission is not assumed. |
| `UnreachableNode { id }` | Node not reachable from `Start` | Fail closed, never silently skip (house rule); names the node. |
| `WrongOutDegree { id, count }` | Non-gateway node — **or converging gateway** — with ≠1 outgoing edge | Mirrors `IrPlanError::WrongOutDegree`, including its application to converging gateways (`single_successor` at ir_plan.rs:370); `JoinAst` carries a single `next`. **Wording corrected after blind review** — first freeze said "non-gateway node" only, leaving a 2-out And-join uncovered. |
| `UnmatchedGateway { id }` | `GatewayAnd` with no `gateway_pairs` partner | Reuses the existing oracle — no hand-rolled re-pairing. |
| `ConditionOnParallelFlow { gateway_id, edge_id }` | **Any** condition (any `ConditionOp`) on an And-diverging edge | The DSL grammar cannot express it (asymmetry (b) above) — `ir_plan` would accept an Eq one, so emitted-then-recompiled could never match; refuse instead. **Reworded after blind review** — first freeze said "Eq-condition", leaving `Neq`/`Lt`/`Gt` falling between this and the next variant's wording. |
| `UnrepresentableCondition { id }` | **Any** condition (any operator) on any other in-core edge | Mirrors `IrPlanError::UnrepresentableCondition`; no in-core linear `NodeAst` carries a condition field. Same any-operator rewording as above. |
| `DuplicateNodeId { id }` | Two in-core nodes sharing one BPMN id | **Added after blind review** — the plan's "at minimum" list mandates it and the first freeze omitted it without a deviation note. Defence in depth (`DesignerDag` fail-closes on duplicates, but `emit_dsl` takes a raw `IRGraph`); a duplicate would silently merge in the `BTreeMap`-shaped plan. |
| `UnrepresentableToken { node_id, field, value }` | Any pass-through string (BPMN id, `task_type`, message `name`/`correlation_key_source`) that does not lex as a DSL Symbol token (charset alnum/`_`/`=`/`-`/`.`, non-empty) | **Added after blind review.** §0.3 freezes verbatim id pass-through, and the printer emits these as bare symbol tokens (`parse_kw_symbol` on re-parse) — a string with a space/`@`/`:`/etc. would print to source the parser rejects. Refuse at emission, naming node, field, and offending value. |
| `ProcessDeclUnrepresentable { field }` | `default_guard_budget` / `default_retry_policy` set | Fork-G ruling: grammar audit confirmed no process-level syntax exists; refuse, never drop. |

**Refusal-check ordering (frozen — added after blind review, which showed
"first refusal wins in canonical node order" is undefined exactly where
canonical order doesn't exist):** checks run in two stages. **Stage 0,
whole-graph pre-checks, fixed order:** `MissingStart` → `MultipleStarts` →
`DuplicateNodeId` → `CyclicGraph` → `UnreachableNode`. Only after all five
pass does a canonical topological order exist. **Stage 1, per-node scan in
that canonical order**, per node: `UnsupportedNode` →
`UnrepresentableToken` → `WrongOutDegree` → `UnmatchedGateway` →
`ConditionOnParallelFlow`/`UnrepresentableCondition` (edge checks on the
node's outgoing edges, in canonical edge order) —
`ProcessDeclUnrepresentable` runs between the stages (graph-level field,
needs no node order). First refusal wins; the same graph always yields
the same refusal.

**B1 amendments (added post-B0-acceptance, per this receipt's own
"additions found during B1 go back into the B0 receipt as amendments"
rule — B1's blind review caught that the first B1 cut shipped these
silently):**
1. **`UnrepresentableToken` also covers the workflow id** (sentinel
   `node_id: "<workflow>"`), checked in the ordering slot between
   `ProcessDeclUnrepresentable` and Stage 1 — the `(workflow <name>` head
   token is a Symbol too.
2. **`WrongOutDegree` carries `{ id, count, expected }`** (not the
   originally frozen `{ id, count }`), with per-kind required out-degree:
   `End` = 0, diverging gateway ≥ 1 (reported as `expected: 1` when 0),
   everything else exactly 1. The frozen "≠1" wording left End and
   0-out-diverging shapes uncovered.
3. **`UnmatchedGateway` means "no UNIQUE partner"**: a converging
   gateway that several diverging gateways pair to (a non-SESE shape
   `gateway_pairs` happily produces — it pairs each split with its
   immediate post-dominator without SESE-integrity checking) refuses at
   the shared join, deterministically, when the canonical scan reaches
   it. B1's first cut instead picked one split via `HashMap`
   last-write-wins — measured THREE distinct emitted sources for one
   graph, all recompiling — the exact nondeterminism the canonical-form
   rule exists to forbid. Cement test:
   `red_shared_join_refuses_deterministically` (20 in-process runs, same
   refusal every time).
4. **`CyclicGraph`'s witness is content-derived** (smallest BPMN id on
   any strongly-connected component), not petgraph's arena-order
   toposort witness.

**Deviation (recorded, for ratification):** the plan's "at minimum" list
also names "non-`Eq` edge condition" as its own axis mirroring
`IrPlanError::UnsupportedConditionOperator`. No such variant is frozen —
in-core, EVERY condition is unrepresentable regardless of operator (And
flows can't carry one grammatically; linear nodes have no condition
field), so the two any-operator variants above subsume it. A dedicated
operator variant becomes necessary only when a conditioned-flow kind
(Xor/Or/Inclusive) enters scope — that is the parity programme's
concern.

Non-refusals, documented as contract notes rather than errors:
- **`ServiceTask.name` is dropped from emitted DSL** (new B0 finding):
  `TaskAst` has no name attribute, and `name` is plan-invisible on BOTH
  paths (`project_ir` drops it too — ir_plan.rs:243-256). Runtime-faithful;
  authoring-metadata-lossy; same class as the fork-E loop ruling.
  Documented in the emitted receipt header, not refused.
- Loop-unrolled copies emit as plain tasks (fork-E ruling, restated).

## Work item 3 — frozen plan-equality (fork D made concrete)

Comparison: `project_ir(to_ir(dag), wf_id)` vs
`dsl::compile(emitted_source, derived_registry)`, field-by-field.

**Registry discipline (the one genuinely new contract element — flagged
for explicit ratification at this gate):** the emitted source's task plugs
are the graph's `task_type` strings verbatim — the same identification
`ir_plan` itself already makes (`plug: task_type.clone()`,
ir_plan.rs:249). `dsl::compile` requires every plug to resolve, so
`EmittedDsl` carries `required_symbols: Vec<String>` (the distinct
task_types), and the equivalence contract compiles against a derived
registry declaring exactly those symbols with **empty** `BindingDecl`s
(no produces/consumes/effect_class) — the honest mirror of "no catalogue
signal exists for graph-authored tasks" that `ir_plan`'s own
`derive_delivery_mode(None, false, false)` call already encodes. Under
this discipline both paths compute `BestEffort` through the SAME shared
formula, and both produce empty placeholder wiring. Compiling emitted
source against any *other* registry is outside the equivalence contract
(a catalogue-registered symbol colliding with a task_type could
legitimately change delivery mode/placeholders — that is the DSL
catalogue doing its job, not a bridge defect).

| Field | Ruling | Reason |
| --- | --- | --- |
| `workflow_id`, `start_node` | compared | Same input id; unique Start. |
| `placeholder_schema` | compared | Empty on both sides under the registry discipline (`PlaceholderSchema::default()` at ir_plan.rs:453; empty decls → no slots on the DSL side). |
| `closure_manifest` | compared | Both construct `Some(json!({"dependencies": []}))` (ir_plan.rs:454, linter.rs:716). |
| `regime_version` | compared | Both read `BPMN_LITE_REGIME_VERSION` at construction; the harness runs both in one process/env. |
| `mathematically_proved`, `unsafe_breeches` | compared | Both derived by the shared `WorkflowExecutionPlan::new` → `analyze_safety` constructor. |
| `nodes` — every field of every in-core `ExecutionNode` | compared | Per-kind walk below. |
| `ExecutionNode::*.span` | **excluded by name** | DSL path stamps `Some(source_span)`; `ir_plan` stamps `None` on every node it builds. Source positions exist only for textual source. This is the ONLY excluded field. |

Per-kind convergence (all verified against the two construction sites,
linter.rs vs ir_plan.rs, this tranche — not assumed):
`StartExecNode{id,next}`; `TaskExecNode{id, plug(=task_type both sides),
delivery_mode(BestEffort via shared formula), static_args(empty —
`IRNode::ServiceTask` carries no args; emission prints no `:args`),
produces/consumes(empty), guards(empty — any `Boundary*` in the graph
already refused as `UnsupportedNode`), loop_origin(None both)};
`MessageWaitExecNode{id, name, correlation_key_source, next}` (emitted
verbatim as `:name`/`:correlation-source` — as bare Symbol tokens, which
is what `parse_message_wait` requires; non-symbol-lexable values refuse
via `UnrepresentableToken`, so "verbatim" is guarded, not assumed);
`SplitExecNode{id, mode=Parallel, routing_socket(None both — And splits
take no plug on either path), flows(placeholder/expected None — conditions
refused), join, produces_placeholder(None)}; `JoinExecNode{id, mode,
split, next}`; `EndExecNode{id, status}` — emission prints exactly
`"terminated"`/`"completed"` per the `terminate` bool, the same sentinel
pair `ir_plan` writes (ir_plan.rs:231-237) and `frontend.rs:380-393`
reads.

Stop-condition check (plan §2): the equality does NOT require comparing
any construct `project_ir` refuses — every in-core kind projects; every
`project_ir`-refused kind is emission-refused first. Alignment holds; no
stop.

## Work item 4 — fixture catalogue

Green (must emit, recompile, and pass plan-equality + `graph_state_hash`
witness; all `DesignerDag`-constructed in test code, `ir_plan.rs` cement-
test discipline):
G1 linear `start→task→end(completed)`; G2 `start→task→message-wait→
task→end`; G3 And block, 2 branches of one task each; G4 And block, 3
branches; G5 `start→task→end(terminate)`; G6 nested And blocks (block
inside a branch); G7 = G1 with several tasks sharing one `task_type`
(dedup check on `required_symbols`).

Red (one per frozen variant; each must hit its EXACT variant, not
`is_err()`): R1–R10 `UnsupportedNode` × each of the 10 kinds (minimal
graph containing that kind); R11 `MissingStart`; R12 `MultipleStarts`;
R13 `CyclicGraph` (hand-built `IRGraph` — `emit_dsl` takes the raw graph,
admission not assumed); R14 `UnreachableNode`; R15 `WrongOutDegree`;
R16 `UnmatchedGateway` (diverging And, no converging partner);
R17 `ConditionOnParallelFlow` (one Eq, one non-Eq sub-case);
R18 `UnrepresentableCondition` (Connect-style conditioned edge between
two tasks); R19/R20 `ProcessDeclUnrepresentable` × both fields;
R21 `DuplicateNodeId` (hand-built `IRGraph`, two tasks named `t1`);
R22 `UnrepresentableToken` × BPMN id with a space; R23
`UnrepresentableToken` × message `correlation_key_source` containing
`@` (the exact class the blind review's scratch test hit); R24
`WrongOutDegree` on a converging gateway (2 outgoing). Every red
fixture also asserts no partial artifact and unchanged
`graph_state_hash`.

## Ground-truth refinements folded into the V&S this tranche

1. **`GatewayInclusive` row made precise** (was "no DSL surface" — in
   fact `split-or` exists but grammatically requires `:plug` + fully
   conditioned flows, and the DSL path lowers plug →
   `routing_socket: Some` while `project_ir` emits `None`): verdict
   unchanged — plan-equality unreachable, not in the lossless core. Core
   remains 5.
2. `ServiceTask.name` loss documented (above).

## Verification

- `cargo test -p bpmn-lite-compiler --lib`: 178 passed / 0 failed
  (includes the 4 new cement tests).
- Mutation red-trace on the And-head fix: recorded above.
- `cargo check --workspace --all-targets`: clean (same 2 pre-existing
  unrelated `bpmn-lite-server-designer` warnings as every prior tranche).
- Public API diff: none this tranche (doc comments, a private printer
  match, tests only).

- **Known deviations or explicitly parked work:**
  - `UnsupportedNode` as one kind-carrying variant vs the plan prose's
    "one variant per kind" — rationale in the catalogue table; ratify or
    reverse at this gate.
  - Parser/AST asymmetries (a)/(b) and the `ServiceTask.name` drop —
    recorded for the DSL-parity programme's backlog.
  - Registry discipline for plug/task_type — the one new contract
    element; explicitly listed for ratification.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived the defect fix (including
  reading the pre-fix printer via `git show 32e4de0^`, reproducing the
  mutation red-trace, and confirming the production impact claim by
  finding the real non-test `to_sexpr` callers —
  `bpmn-lite-server-designer/src/rest.rs:1706` feeding `dsl::compile`,
  plus `bpmn-lite-authoring/src/diagnostics_executor.rs`), verified every
  "compared" ruling in the equality table against both construction
  sites, verified the registry discipline (`register_verb` + empty
  `BindingDecl` → `Known`, no hidden defaults), and reproduced all test
  runs. Verdict: four findings, all disposed by edits (not argument):
  1. **The message-wait cement leg was vacuous** — the fixture quoted
     values the parser requires as bare symbols; the error-swallowing
     `parse_sexpr` helper let the parser's recovery loop silently drop
     the node, so the fixpoint was proven on a workflow *without* the
     message-wait (the reviewer proved this empirically with a scratch
     parse). Disposed: helper hardened to fail on any lex/parse error,
     fixture corrected to symbol tokens, 6/6 re-verified — the
     message-wait leg is now genuinely proven.
  2. **Two plan-mandated catalogue axes were missing without a deviation
     note** (duplicate-id; non-Eq operator). Disposed: `DuplicateNodeId`
     added; the non-Eq axis recorded as an explicit
     subsumed-by-any-operator-wording deviation for ratification, with
     both condition variants reworded to "any operator."
  3. **No refusal axis existed for non-symbol-lexable pass-through
     strings** (ids, task_types, message names/correlation sources)
     despite §0.3's verbatim rule. Disposed: `UnrepresentableToken`
     added, with red fixtures R22/R23.
  4. **Refusal ordering was undefined exactly where canonical order
     doesn't exist** (no/multiple starts, cycles). Disposed: two-stage
     ordering frozen (whole-graph pre-checks in fixed order, then the
     canonical per-node scan). Also corrected `WrongOutDegree`'s wording
     to cover converging gateways (reviewer's finding), with fixture R24.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate B0's own text: "Peer review ratifies the frozen error catalogue,
the field-by-field equality definition, and the fixture list." All three
tables are above, in full. B1 does not start until this gate is accepted.
