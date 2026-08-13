# EOP-VS-DSL-PARITY-001 — Full graph↔DSL parity (vision & scope)

| Field | Value |
| --- | --- |
| Status | **All forks A–G ruled as recommended (Adam, "agree", 2026-08-13). Implementation plan: `EOP-PLAN-DSL-PARITY-001.md`.** |
| Predecessor | `EOP-PLAN-GRAPH-DSL-BRIDGE-001` (CLOSED, B0–B3 accepted 2026-08-13) — the staging skeleton whose refusal catalogue is this document's backlog. |
| Mandate | Fork-A ruling (Adam, 2026-08-13): full parity is the required end state — "it's simply not done otherwise." Fabric-V&S criterion 19: every runtime artifact emittable as canonical DSL and rebuildable to identical artifact identity. |
| Decides | Nothing. §4's forks carry recommendations; peer review rules them. |

---

## 1. Where parity stands after the bridge (all claims B-series-verified, receipts cited)

The bridge proved the frame: emission, typed refusal, plan-level
recompile-equivalence (CI-gated), and the receipt endpoint all exist and
are cement-locked. Parity is now a **coverage** problem: every
`UnsupportedNode` refusal in B1's catalogue is one missing DSL surface.

### 1.1 The 10 refused kinds, tiered by what's actually missing

| Tier | Kinds | Missing | Already present |
| --- | --- | --- | --- |
| **P1 — DSL surface only** (plan representation exists on BOTH paths; grammar+printer+linter+emission are the whole job) | `TimerWait` (→`Wait`), `MultiInstance`, boundary guards (`BoundaryTimer`/`BoundaryError` → `GuardExecSpec` on host) | DSL grammar heads/attributes, `NodeAst` variants, `ToSexpr` impls, linter lowering, emission arms, B2 fixtures | `ExecutionNode` variants and `project_ir` support all landed (WS-D D1, G5.4a) |
| **P1½ — surface exists, semantics misaligned** | `GatewayInclusive` | `split-or` parses but grammatically REQUIRES `:plug` + per-flow `:condition`, and lowers plug→`routing_socket: Some`, while `project_ir` emits `None` (B0 receipt). Needs either grammar relaxation (optional plug) or graph-side plug carriage — a real design fork (§4-D) | `project_ir` support, `SplitModeAst::Or`, pairing oracle |
| **P2 — no plan representation at all** | `HumanWait`, `SendTask`, `FfiServiceTask` | Everything in P1 **plus** new `ExecutionNode` variants — i.e. changes to the stored `WorkflowExecutionPlan` contract itself (plan-store artifacts, kernel lowering). Strictly larger blast radius | IR variants, XML import |
| **P3 — structural design work** | `GatewayXor` (no join-pairing oracle in either direction — fork-B ruling parks it here), `DataObject` (structural-only node; needs a DSL declaration form and an emission story for a node with no sequence flow) | A traced join-inference design (XOR); a declaration grammar (DataObject) | `ConditionExpr` Eq machinery; `project_ir` omits DataObject by design |

### 1.2 Non-kind backlog items surfaced by the B-series

1. **Process-level `default_guard_budget`/`default_retry_policy` syntax**
   (fork-G: no grammar exists; emitter refuses a DAG that sets either).
2. **IR-side loop provenance** (fork-E follow-up): `unroll.rs` already
   stamps `loop_origin` and the plan carries it — only the `IRNode`-side
   carrier is missing; with it, emission could fold copies back into
   `LoopAst` instead of the ruled runtime-faithful-but-sugar-lossy form.
3. **Parser/AST asymmetries** (B0): plug-less `Xor`/`Or` splits are
   constructible in the AST (legacy `exclusive-gateway` parse fn) but
   unprintable-as-parseable; conditioned And-flows are IR-expressible but
   grammar-inexpressible (currently an emission refusal).
4. **`ServiceTask.name` loss**: `TaskAst` has no name attribute;
   plan-invisible on both paths, but authored metadata is dropped from
   emitted source (documented, not fixed).
5. **`project_ir` flow-order non-canonicality** (B2 finding, surfaced for
   ruling): flows are arena-order; two equivalent edit orders yield
   different stored plan bytes, different `V2Fork` target order,
   different fiber-ID/tape ordering — outcome-equivalent, not
   replay-tape-identical. Touches G6 replay-equivalence territory.
6. **`session.name` vs UUID workflow-id inconsistency** (B3): the graph
   endpoint feeds `project_ir` the free-text name; the receipt endpoint
   uses the UUID.
7. **Diagnostic precision for boundary constructs** (B3): guards refuse
   as `UnreachableNode` (no incoming flow edge) before `UnsupportedNode`
   — correct per the frozen ordering, but the diagnostic will improve
   for free once guards join the core.
8. Stale `bpmn_board.rs` "seven actions" comment (bridge-V&S §2.4) —
   trivial, unfixed.

---

## 2. Proposed shape

One parity tranche per kind (or coherent kind-group), each delivering the
full vertical: grammar + parser + `ToSexpr` + linter lowering + emission
arm (removing the `UnsupportedNode` row) + `project_ir` alignment where
needed + **B2-harness green fixtures for the new kind** + red fixtures
for its new failure axes. The B2 harness is the standing acceptance
machine: a kind is "in parity" exactly when its fixtures pass the
four-proof round trip. B1's no-wildcard match guarantees each tranche
consciously touches the emitter.

Sequencing recommendation: P1 kinds first (guards, TimerWait,
MultiInstance — highest value per unit risk, zero plan-contract change),
then P1½ (Inclusive, after fork D below), then P3-XOR (the oracle
unblocks both projections), then P2 (plan-contract expansion — possibly
its own sub-programme given the plan-store/kernel blast radius), with
DataObject and the §1.2 items slotted where their dependencies land.

---

## 3. Exclusions (unchanged from the bridge V&S unless re-ruled)

DSL→graph import; XML/`NodeDto` path changes; Sage/palette exposure;
`CreateRace`/`CallSubprocess` (no IR exists — excluded by design,
ops.rs); fuzzing (U4 is the fuzz plan's own tranche, already unblocked).

---

## 4. Forks to rule (recommendations attached; none decided)

**A. Tranche granularity and order.** Per-kind tranches in the §2
sequence, P1 first. *Recommendation: as stated.*

**B. P2's plan-contract expansion.** New `ExecutionNode` variants change
the stored artifact schema consumed by the plan store and kernel.
Options: (1) in-scope as late tranches here; (2) split into its own
programme with its own V&S once P1/P1½ close. *Recommendation: (2) —
decide with real P1 experience in hand; this document's successor plan
covers P1/P1½/P3-XOR only.*

**C. Grammar style for new forms.** New node heads following the
existing idiom (`timer-wait`, `multi-instance`, `guard`/`boundary-timer`
nested under the host or top-level with `:host`), attributes as
keyword/symbol pairs, str-lits only where free text is legitimate.
*Recommendation: freeze per-kind grammar at each tranche's own B0-style
contract gate — not globally here; but rule now that heads are NEW
keywords (never overloading existing ones) and that every new form must
be printable by `ToSexpr` and re-parseable to fixpoint before its
tranche closes (the B2 discipline).*

**D. Inclusive alignment.** Either relax the grammar (optional `:plug`,
optional conditions — making DSL-Or expressible without a decision
socket) or carry a plug on the graph side. The routing-socket concept is
load-bearing for the typed-routing thesis ("routing lives in the box"),
which suggests the graph side should eventually carry the decision
socket rather than the grammar dropping it. *Recommendation: defer
Inclusive until after P1; open it with a dedicated design note tracing
the routing-socket/OR-named-subset settled decisions — this is exactly
the "OR gateways use named-subset output types" territory where a quick
grammar hack would undercut the thesis.*

**E. Loop provenance (§1.2-2).** *Recommendation: in-scope, one tranche,
after P1 — the machinery is mostly built; ruling needed on whether
emission then folds copies back into `LoopAst` (changing emitted source
for existing graphs — a bridge-contract version bump per the frozen
canonical-form rule).*

**F. `project_ir` flow-order canonicalisation (§1.2-5).**
*Recommendation: rule it here (it blocks replay-tape determinism claims
elsewhere): make `project_ir` sort flows by edge id — a stored-artifact
byte change, so it needs its own migration/impact check on existing
plan-store contents; if accepted, B2's multiset comparison can tighten
back to ordered equality.*

**G. Small items (§1.2-4/6/8).** *Recommendation: fold into whichever
tranche touches the file, receipt-noted — no dedicated tranches.*

---

## 5. Peer-review checklist

- [ ] Confirm §1's tiering matches the code (spot-check the receipts).
- [ ] Rule forks A–G.
- [ ] Confirm §3 exclusions.
- [ ] Authorise drafting of the implementation plan for the ruled scope
      (expected: P1 guards/timer/MI first, per-kind gates, B2 harness as
      the acceptance machine).

**Forks A–G ruled as recommended, 2026-08-13. Scope of the successor plan: P1 + P1½ + P3-XOR + loop-provenance + flow-order canonicalisation; P2 deferred to its own future programme.**
