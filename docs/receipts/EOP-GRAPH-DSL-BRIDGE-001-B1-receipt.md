# EOP-PLAN-GRAPH-DSL-BRIDGE-001 — B1 receipt

Baseline: Gate B0 accepted at `2cbd450` (branch
`codex/bpmn-gameboard-refactor`). **Tier: CAREFUL.**

- **Scope delivered:** all four B1 work items — `emit.rs` implementing
  exactly the B0-frozen contract, the `DesignerDag` wrapper, exact-variant
  red tests + recompiling green tests, and the module doc stating the
  projection's contract and its relationship to `ir_plan.rs`.

## Work item 1 — `bpmn-lite-compiler/src/dsl/emit.rs`

- `DslEmitError`: all 12 frozen variants, wordings per the corrected B0
  catalogue (`WrongOutDegree` carries `expected` and covers converging
  gateways and End's required 0; both condition variants any-operator;
  `UnrepresentableToken` with node/field/value).
- Two-stage refusal ordering implemented exactly as frozen: Stage 0
  (`MissingStart` → `MultipleStarts` (sorted ids) → `DuplicateNodeId`
  (smallest duplicate) → `CyclicGraph` (toposort witness) →
  `UnreachableNode` (smallest unreachable id)), then
  `ProcessDeclUnrepresentable` (guard-budget before retry-policy, fixed),
  then the workflow-id token check, then the Stage-1 per-node scan in
  canonical order (Kahn rooted at the unique Start, ready-set keyed by
  BPMN id — smallest first; split flows sorted by edge id).
- Emission: core-5 only; the `IRNode` match has **no wildcard arm** — the
  10 out-of-core kinds are one explicit `|`-pattern arm, so a 16th
  variant breaks the compile (B0's structural fail-closed rule).
  `gateway_pairs` reused for And-pairing (never re-derived); `ToSexpr` is
  the only printer; End emits exactly the `"terminated"`/`"completed"`
  sentinel pair; `ServiceTask.name` dropped per the documented B0 ruling.
- `is_symbol_token` mirrors `lexer::is_symbol_start`/`is_symbol_continue`
  exactly (start: alnum/`_`/`=`/`-`; continue adds `.`/`:`), applied to
  workflow id, every node id, `task_type`, message `name` and
  `corr_key_source`.
- `EmittedDsl { source, ast, required_symbols }` — sorted, deduped
  distinct task_types for the equivalence registry.

## Work item 2 — `DesignerDag::emit_dsl` wrapper (`designer-graph/src/schema.rs`)

Field plumbing only, as the plan requires: `to_ir()` + set-ness of the
two process-level declarations + `graph_state_hash` witness, returning
`DslReceipt { emitted, graph_state_hash }`.

**Deviation from the plan's §0.6 signature, recorded for ratification:**
- B0/the plan placed `graph_state_hash` inside the compiler's
  `EmittedDsl`. The hash is `designer-graph`'s content-derived identity
  (`DesignerDag::graph_state_hash`), and the compiler sits *below*
  `designer-graph` — it cannot compute or even name it. The witness
  therefore rides the wrapper-layer `DslReceipt` instead. No information
  is lost; it moves one layer up, to the layer that owns it.
- `ProcessLevelDecls` carries set-ness booleans, not the values: refusal
  needs only "is it set", and the concrete `RetryPolicyDecl` type lives
  above the emitter. (Values would add a downward type dependency for a
  field the emitter only ever refuses on.)

## Work item 3 — tests

`bpmn-lite-compiler` `dsl::emit::tests`, 20 tests (18 initial + 2
blind-review cement tests):
- Greens: linear (emits → **recompiles** with the derived empty-bindings
  registry → byte-identical on second emission — idempotence);
  message-wait + terminate-end (recompiles, sentinel asserted); And block
  2 branches (recompiles, `split-and`/`:join` shape asserted).
- Reds, every one asserting its EXACT variant and payload (never bare
  `is_err()`): MissingStart; MultipleStarts (sorted ids asserted);
  DuplicateNodeId (smallest); CyclicGraph; UnreachableNode
  (smallest-id determinism asserted); ProcessDeclUnrepresentable (both
  fields, fixed order asserted); **UnsupportedNode × all 10 kinds in one
  exhaustive loop** (R1–R10, each kind's name asserted);
  UnrepresentableToken × id-with-space, corr-source-with-`@` (R22/R23),
  and workflow-id; WrongOutDegree × task-with-2 and
  **converging-gateway-with-2** (R24); UnmatchedGateway;
  ConditionOnParallelFlow (gateway+edge ids asserted);
  UnrepresentableCondition.
- `designer-graph`: wrapper test — emission for a real `DesignerDag`,
  witness equals `graph_state_hash(to_ir())`, required_symbols correct,
  and a set `default_guard_budget` refuses through the wrapper.

Green fixtures with full B0 plan-equality assertions (field-by-field vs
`project_ir`) are **B2's** harness, per the tranche map — B1's greens
prove emit + recompile + idempotence, exactly its scope.

## Work item 4 — module documentation

`emit.rs`'s module doc states: sibling-projection relationship to
`ir_plan`, the frozen canonical-form rules, the two-stage refusal
ordering, the equivalence contract including the empty-bindings registry
discipline and its `derive_delivery_mode(None,false,false)` grounding,
and the process-decl refusal rationale.

## Public API diff — exactly the enumerated additions, nothing else

`python3 scripts/check-semantic-gameboard-boundaries.py`: **pass**
(workspace-wide baseline ratchet green after update). Baseline diff is
purely additive, zero removals:
- `bpmn-lite-compiler` (+38 API lines): `dsl::emit_dsl` fn, `DslEmitError`
  enum + 12 variants/fields + Error/Display impls, `EmittedDsl` + 3
  fields, `ProcessLevelDecls` + 2 fields (+ derive-generated impls).
- `designer-graph` (+4 lines): `DesignerDag::emit_dsl`, `DslReceipt` + 2
  fields.
Consumer/facade/contract (the gate's required naming): consumer is B3's
server receipt endpoint (and B2's proof harness); owning facade is
`bpmn_lite_compiler::dsl` (curated re-export list, same as `project_ir`)
and `designer_graph::schema`; stability contract is the B0-frozen bridge
contract — canonical-form changes are a version bump.
`python3 scripts/check-test-only-pub.py`: `ok: 0`.

## Verification

- `cargo test -p bpmn-lite-compiler --lib`: **196 passed / 0 failed**
  (178 prior + 18 new).
- `cargo test -p designer-graph --lib`: **72 passed / 0 failed**.
- `cargo check --workspace --all-targets`: clean (same 2 pre-existing
  unrelated `bpmn-lite-server-designer` warnings).
- Boundary gates: both pass (above).

- **Refusal catalogue delta vs B0's frozen list: four amendments,
  recorded in the B0 receipt** (its own amendment rule) — the first
  version of this receipt claimed "none", which the blind review refuted:
  workflow-id token coverage; `WrongOutDegree`'s `expected` payload and
  per-kind semantics; `UnmatchedGateway` = "no *unique* partner"
  (shared-join refusal); content-derived `CyclicGraph` witness.

- **Known deviations or explicitly parked work:** the two signature
  deviations under work item 2 (witness location; set-ness booleans) —
  both structural consequences of the crate direction, flagged for
  ratification at this gate.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived the implementation against the
  frozen B0 contract, reproduced all test runs, verified the no-wildcard
  claim, the lexer token-mirror (including confirming the lexer has no
  numeric token kind, so numeric ids re-parse), the Kahn algorithm's
  parallel-edge correctness, keyword-collision non-exploitability
  (ids named `flow`/`workflow`/`condition`, task_type `=`/64-hex all
  emit and recompile — checked empirically), and hunted for an
  emit-green/recompile-red counterexample (none found). Verdict: seven
  findings, all disposed:
  1. **Stage-1 order violated — id token checked before the kind gate**
     (a `GatewayXor` with a bad id refused `UnrepresentableToken`, not
     `UnsupportedNode`). Fixed: the out-of-core arm returns first; token
     checks moved inside each in-core arm. Cement:
     `stage1_order_kind_before_token_and_degree_before_pairing`.
  2. **Stage-1 order violated — pairing before out-degree on gateways.**
     Fixed: both gateway arms check degree first; `single_next` split
     into `single_out_edge` (degree) + `uncond_next` (condition), so the
     frozen WrongOutDegree → UnmatchedGateway → conditions order holds.
     Same cement test.
  3. **MAJOR: nondeterministic emission on shared-join topologies** —
     `gateway_pairs` pairs every split with its immediate post-dominator,
     so several splits can share one join; the reverse map's `HashMap`
     last-write-wins picked one arbitrarily (reviewer measured three
     distinct emitted sources for one graph, all recompiling). Fixed:
     shared joins are detected up front and refuse `UnmatchedGateway` at
     the join in canonical scan order — never pick one. Cement:
     `red_shared_join_refuses_deterministically` (20 runs, identical
     refusal). B0 amended (its item 3).
  4. **"Catalogue delta: none" was false** — four silent extensions.
     Disposed by amending the B0 receipt (above) and this line.
  5. **`CyclicGraph` witness was arena-order dependent.** Fixed:
     SCC-derived smallest-id witness, content-deterministic.
  6. **Wrapper red test was a substring check** through the anyhow
     boundary. Fixed: downcast to `DslEmitError` and exact-variant match.
  7. Baseline count nit: compiler baseline is +38 API lines (+2 header
     lines rewritten), not "+40 lines". Corrected here.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate B1's own text: "All green fixtures emit; all red fixtures hit
their named variant; public API diff is exactly the enumerated additions;
workspace check + boundary gates clean." All four evidenced above. B2
does not start until this gate is accepted.
