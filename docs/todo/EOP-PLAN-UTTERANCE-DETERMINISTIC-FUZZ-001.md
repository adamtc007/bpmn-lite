# EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 — Deterministic discovery-pipeline fuzzing

| Field | Value |
| --- | --- |
| Status | **U0/U1/U2/U3 accepted — plan complete except U4 (blocked by design, no production graph-to-DSL bridge exists)** |
| Baseline reviewed | `efda5ad` (2026-08-13); U0 closed against `be9a86f` (2026-08-13), see `docs/receipts/EOP-UTTERANCE-DETERMINISTIC-FUZZ-001-U0-receipt.md`; U1 `996bb68`/`3d7431d`; U2 `c57341f`; U3 `982c49b`/`d604241`, accepted "ok next" (2026-08-13) |
| Scope | Model-free, deterministic path from an utterance-derived evidence packet to governed DSL/graph proposal preview. |
| Execution | One tranche per change set; STOP for review at every tranche gate. |
| Does not authorise | Live-model fuzzing, raw HTTP fuzzing, capture changes, public test helpers, or automatic proposal application. |

---

## 0. Objective

Add coverage-guided fuzzing for the **composition** of the deterministic
utterance-discovery pipeline. The target is not arbitrary natural language or a
live model. It is the part of the pipeline which must give the same result for
the same graph, policy, history, utterance normalisation, and evidence.

The primary target follows the **implemented graph-Repl path**. It must not
claim to cover the textual `bpmn-dsl` AST path: today the Repl produces graph
operations, not `WorkflowSource` AST nodes or DSL source. The compiler's DSL
path remains a separate capability with its own fuzz target. A conditional
equivalence tranche is included only if a real production graph-to-DSL bridge
is separately designed and accepted.

```text
bounded utterance family + valid/hostile evidence tape + fixed graph fixture
  → semantic board and design position
  → complete position-bound move evidence
  → bounded belief
  → deterministic game disposition
  → [server-owned] compound resolution / workbook construction
  → materialisation, dry application, compiler admission and preview
```

The result must strengthen the existing component fuzz suite without replacing
it. It must answer one specific question:

> Can an adversarial but bounded evidence/utterance/history combination cause
> the deterministic discovery path to panic, mutate the authoritative graph,
> emit an off-board/stale proposal, become non-deterministic, or bypass preview
> and compiler admission?

---

## 1. Scope boundary

### 1.1 What this plan fuzzes

The discovery pipeline has two ownership layers.

| Layer | Owner | Deterministic responsibility | Planned target |
| --- | --- | --- | --- |
| Core gameboard | `utterance-engine` | Board/position construction, evidence finalisation, belief update, disposition, record closure. | `deterministic_discovery_pipeline` |
| DSL proposal composition | `bpmn-lite-server-designer` | Compound-chain fold, workbook construction, materialisation, preview, compiler admission. | `utterance_proposal_pipeline` (subject to §3 decision) |

The core target is the mandatory outcome of this plan. The server target is
conditional because its useful input is not an unstructured HTTP request and
must not create a test-only public API.

### 1.2 Graph Repl versus textual DSL AST

These are distinct, non-interchangeable paths. This distinction is a boundary
rule for every tranche and receipt.

```text
Implemented graph-Repl path (primary scope)
utterance/evidence → ProposalWorkbook → Operation tape → DesignerDag → IR
                  → dry apply / verify / lower / compiler preview

Textual DSL compiler path (separate capability)
bpmn-dsl source → lexer → WorkflowSource / NodeAst → lint/lower
                → WorkflowExecutionPlan
```

`start_workbook` and `materialize_bpmn_workbook` produce a typed workbook and
`Operation` tape. They do not build, parse, serialise, or mutate a
`bpmn_lite_compiler::dsl::WorkflowSource`. Conversely, the existing
`dsl_compile` target fuzzes source admission without involving utterance
evidence, a semantic board, or a Repl workbook.

Therefore:

- U0–U3 cover the **utterance → graph Repl → IR/compiler-preview** route.
- The existing compiler target remains the coverage owner for raw DSL
  text → AST → DSL compile safety.
- U4 is conditional and must not begin until a production graph-to-DSL
  projection/serializer exists with a stated compatibility contract. A fuzz
  target must never invent that bridge just to make an equivalence assertion.

### 1.3 What this plan does not fuzz

- Tier-1 Candle loading, weights, tokenisation, inference, or network/cache I/O.
- Embedding model loading/inference.
- Arbitrary natural-language generation as a measure of semantic accuracy.
- Postgres, HTTP sockets, session locking, or capture persistence in a
  high-throughput fuzz loop.
- Ratification or mutation of a real design session.
- The quality of ranking/model accuracy; model outputs are evidence only.
- Existing decoder, serializer, history, workbook-transition, and compiler
  fuzz targets except where this plan composes their already-tested contracts.

Random UTF-8 remains valid at lexical parsing and normalisation boundaries;
`phrase_index` already covers that class. At this pipeline boundary it would
mostly produce no governed match and no deep coverage, so it is deliberately
not the primary generator.

---

## 2. Existing coverage and the composition gap

The existing fuzz suite is substantial and must be retained:

| Existing target | Current responsibility |
| --- | --- |
| `phrase_index` | Raw UTF-8 / normalisation and governed exact-match collision safety. |
| `evidence_fusion` | `SlmResult` validation, deterministic evidence finalisation, lane completeness, history influence, ordering invariance. |
| `disposition_workbook_state`, `clarification_policy` | Deterministic disposition and workbook-state paths. |
| `game_turn_replay` | Content-addressed game-turn closure and hostile cross-field refusal. |
| `legal_move_enumeration`, `preview_compilation` | Candidate enumeration; all executable materialisers; dry apply and compiler admission. |
| `model_boundary`, `v3_route_admission` | Model request/card admission without loading a model. |
| `bpmn-lite-compiler/fuzz:dsl_compile` | Raw `bpmn-dsl` source → lexer/parser/AST/lint/compile admission. |
| `bpmn-lite-compiler/fuzz:plan_deserialize` | Workflow-plan deserialisation boundary. |
| `history_belief_state` | Board/position/evidence/belief composition (U0 finding: already builds a real `DesignerDag`/board/position, fuzzes `SlmResult`, calls `finalize_bpmn_move_evidence`, and asserts replay-determinism through a belief-update loop — stops short of `decide_bpmn_game_disposition`. **U1 extends this target rather than adding a new one**; see U0 receipt.) |

**U0 finding**: `utterance-engine/fuzz` declares 15 targets, not the 9
above plus `history_belief_state` — the full 15-target inventory,
corpus/regression/seed counts, and the additional finding that the pinned
`semantic-decision-contracts` dependency has its own 8-target
contract-level (not pipeline-composition) fuzz suite, are recorded in the
U0 receipt rather than duplicated here.

The current application path composes these stages in
`bpmn-lite-server-designer/src/rest.rs::session_utterance_endpoint`, but no
target executes the core stages as one deterministic run from a generated,
board-valid ranking through a governed disposition. The server's
`resolve_compound_chain` adds a second composition seam: span two must be
resolved against the compiler-admitted hypothetical graph created by span one,
not against the original graph.

This plan closes the first gap and presents a bounded, hygiene-preserving path
to close the second.

---

## 3. Decisions required before execution

No production change may begin before these decisions are reviewed.

1. **Target ownership — proposed:** add the mandatory target to
   `utterance-engine/fuzz`; retain server-only compound/workbook orchestration
   in `bpmn-lite-server-designer`, never moving it into `utterance-engine`.
2. **Server composition target — proposed:** use the existing public router as
   a black-box, bounded integration fuzz target only if a benchmark proves it
   sustains a useful execution rate. Otherwise retain server-owned property
   tests and add no test-only visibility. Do not publish
   `resolve_compound_chain` or `start_workbook` merely for cargo-fuzz.
3. **Evidence producer — proposed:** the mandatory target constructs complete
   valid `SlmResult` values locally. It must not call lexical/embed/Candle
   retrieval as a substitute for generated evidence. Retrieval selection is
   separately tested; finalisation and disposition are the subject here.
4. **Text generator — proposed:** select a small governed phrase family, then
   fuzz bounded formatting and Unicode decoration. Include a separate
   abstention family. Do not claim language-understanding coverage.
5. **Failure oracle — proposed:** valid tapes must satisfy deterministic
   closure assertions; hostile tapes must return a typed error/refusal before
   any proposal can be staged. Panics, silent candidate dropping, and fallback
   proposal creation are failures.
6. **AST-equivalence policy — proposed:** U4 remains blocked until a real
   product requirement adds a graph-Repl → canonical DSL source/AST bridge.
   `dsl_compile` continues as the DSL-AST safety target; no synthetic source
   serializer is introduced by this fuzz plan.
7. **Resource envelope — proposed, amended by U0 ruling:** input maximum
   4 KiB, at most 8 graph nodes, and a fixed finite fixture catalogue.
   History-receipt count: retain `history_belief_state.rs`'s existing,
   already-asserted 64-receipt cap (ruled at Gate U0 — the target being
   extended already has a working, corpus-backed bound; shrinking it to
   16 would invalidate existing coverage for no benefit to U1's actual
   purpose). Any larger resource-abuse case belongs in a dedicated target
   with a documented ceiling and handcrafted seed.

---

## 4. Input model

`deterministic_discovery_pipeline` consumes a compact byte tape. It is not a
serde decoder fuzz target. It must construct mostly valid states so the fuzzer
reaches policy and proposal code rather than rejecting at parsing.

| Tape field | Generated value | Purpose |
| --- | --- | --- |
| Fixture selector | One of a bounded set of admitted DAGs and anchors. | Varies legal-move shape while preserving graph validity. |
| Focus selector | Valid anchor, absent focus, unknown-reference refusal. | Exercises position/focus boundaries. |
| Utterance family | Exact operation, duration, count, node reference, compound delimiter, abstention. | Reaches governed normalisation without unbounded language fuzzing. |
| Text mutations | Case, whitespace, punctuation, bounded valid/invalid UTF-8 decoration. | Exercises normalisation and strict compound syntax. |
| Ranking tape | Complete in-board ranking with fuzzed finite scores and input order. | Exercises canonicalisation, evidence fusion and policy topology. |
| Lane selector | Lexical, embedding, or Candle evidence *identity only*. | Ensures lane metadata does not alter deterministic governance. |
| History tape | Valid, correction, irrelevant, and bounded hostile history. | Exercises belief/history influence and refuse paths. |
| Hostile-axis selector | Exactly one invalid relation. | Makes failures attributable and avoids all-invalid inputs. |

### Valid-tape construction

For a valid tape the target must:

1. Build a real `DesignerDag`, semantic board, context projection and design
   position using production public APIs.
2. Start from a ranking containing every board candidate exactly once, carrying
   the actual board hash and finite score values.
3. Call `finalize_bpmn_move_evidence`, then belief update and
   `decide_bpmn_game_disposition`.
4. If it produces a serialisable game-turn closure, construct/replay it using
   the normal public game-turn API.

### Hostile-axis construction

One axis at a time is changed from that valid base. Minimum axes are:

- foreign board hash;
- omitted candidate, duplicate candidate, and off-board candidate;
- rank-order permutation with identical scores;
- stale/unknown focus;
- invalid correction reference or resource-limited history;
- exact-match formatting equivalent versus non-equivalent;
- compound delimiter with an unresolvable span.

Non-finite scores are already rejected by `FiniteScore` construction; they
remain covered by the existing boundary target and must not be smuggled around
that contract with unsafe or deserialisation-only construction.

---

## 5. Invariants and oracles

Every fuzz iteration must make a precise assertion; “does not crash” alone is
insufficient.

### P1 — Graph non-mutation

The source DAG's canonical IR/content identity is identical before and after
every core pipeline attempt, whether it succeeds or is refused.

### P2 — Replay determinism

For a valid tape, running the core pipeline twice yields equal canonical move
evidence, belief, disposition, selected move IDs, and record hashes. Permuting
the input ranking without changing candidate/score pairs yields the same
canonical result.

### P3 — Closed-world evidence

On success, evidence has exactly one entry for every legal move in the
position; every move ID is on that position; all scores/probabilities are
finite; probabilities sum to one within a documented epsilon.

### P4 — Position-bound decision

The disposition validates against the generated position. It cannot name an
off-board or stale move, and any terminal attempt receipt names the same
position state ID.

### P5 — Fail closed

Each hostile axis yields the expected typed refusal at or before its owning
boundary. A rejection must not mutate the DAG, change a prior deterministic
result, or yield a constructible proposal/workbook.

### P6 — Governed text equivalence

For the exact-match families, normalisation-equivalent variants yield the same
resolved candidate/evidence identity. Non-equivalent text is never upgraded to
an exact match merely by normalisation.

### P7 — Server composition, if ratified

For a fully bound generated discovery result, the proposal path either:

- produces a preview whose delta is deterministic and whose operation tape
  admits against a clone; or
- returns the expected typed compiler/legality refusal.

It must never persist a graph edit or ratify a proposal during fuzzing.

### P8 — Graph/DSL equivalence, only after a real bridge exists

This is explicitly **not currently assertable**. Once a production
graph-Repl → DSL source/AST bridge is accepted, an admitted operation tape and
its projected DSL must compile to an equivalent canonical IR/plan under a
declared equivalence relation. A refusal must preserve the source DAG and must
not emit a partial DSL artifact. Until then P8 is a blocked future contract,
not a claimed test gap in the implemented pipeline.

---

## 6. Tranche map

```text
U0  Baseline and target contract       evidence only; no code changes
U1  Core deterministic target          utterance-engine cargo-fuzz target + seeds
U2  CI, regressions and coverage        smoke, nightly discovery, semantic counters
U3  Server-owned composition decision   benchmark then either bounded router target or close as deferred
U4  Graph-Repl ↔ DSL-AST equivalence    BLOCKED: needs a real production projection contract
```

U0 is mandatory. U1 precedes U2. U3 is independent of the core target's
delivery and cannot block U1/U2; it is deliberately a decision tranche, not
an implicit requirement to expose server internals. U4 does not follow merely
because U1–U3 close: it requires a separately ratified product capability.

---

## U0 — Baseline and target contract

**Tier:** CAREFUL. **Code changes:** forbidden.

### Work

1. Record the current target list, corpus size, and PR/nightly execution
   policy. Confirm automatic discovery from `[[bin]]` entries is still the
   source of truth.
2. Map each P1–P7 invariant to existing coverage or to U1/U3. Do not duplicate
   an existing target merely because its name is similar.
3. Select the bounded DAG fixtures and utterance families; show that each
   produces at least one legal move and that the corpus reaches exact,
   non-exact, abstention, and compound syntax cases.
4. State exactly which hostile axes are expected to error at which public API.

### Gate U0

Peer review approves the target boundary, input budget, fixture catalogue, and
P1–P6 assertions. No production API change is approved by this gate.

---

## U1 — Core deterministic discovery target

**Tier:** CAREFUL.

**Amended by U0 ruling** (see `docs/receipts/EOP-UTTERANCE-DETERMINISTIC-FUZZ-001-U0-receipt.md`):
U1 extends the existing `utterance-engine/fuzz/fuzz_targets/history_belief_state.rs`
target in place. No new `[[bin]]` entry or new target file. Every reference
below to "the core target" means this file. The U0 receipt's corrected
work item 2/4 tables narrow the residual gap: P3 (closed-world evidence)
and P6 (governed text equivalence) need no new design, only reuse of
already-proven `finalize_bpmn_move_evidence` properties already asserted
in `evidence_fusion.rs`; P2's rank-permutation and P4's hostile-axis
patterns should be ported from `evidence_fusion.rs` /
`disposition_workbook_state.rs`, not redesigned. The 64-history-receipt
cap already asserted in `history_belief_state.rs` (line 240) is retained
as-is — §3 item 7's proposed "16" is advisory headroom for new axes, not a
rewrite of this existing, already-corpus-backed bound.

### Work

1. Extend `history_belief_state.rs`: no new `[[bin]]` entry.
2. Add the missing tape fields from §4 not already present (Text mutations,
   the remaining §4 Hostile-axis-construction axes not already covered
   elsewhere per the U0 receipt) to the existing generator. It must depend
   only on supported public `utterance-engine`, `designer-graph`, compiler,
   and semantic-contract APIs — unchanged constraint.
3. Wire `decide_bpmn_game_disposition` into the existing board → position →
   generated ranking → `finalize_bpmn_move_evidence` → belief loop, using
   `disposition_workbook_state.rs`'s pattern (real call +
   `validate_for_position` + off-board-move assertion) as the reference
   implementation, and add game-turn record closure where applicable.
4. Implement P1 (genuinely new — full-sequence content-hash assertion) and
   the residual P5 hostile axes, narrowed by the U0 receipt's second
   correction (verified empirically before implementation, not assumed):
   only **off-board-candidate injection** (extending
   `evidence_fusion.rs`'s existing duplicate/omit malformed-ranking
   pattern with the one sub-case it doesn't cover) and **foreign/stale
   board revision** (`build_bpmn_design_position`'s
   `BpmnBoardError::StaleBoardRevision`, confirmed exercised by zero
   existing fuzz targets) are genuinely new. "Foreign board hash" is not
   a real refusal at `finalize_bpmn_move_evidence` (the field is stamped
   on output, never validated on input — verified by reading
   `fusion.rs:632`); "stale/unknown focus" is not a refusal at all
   (`DesignFocus::unknown(...)` succeeds — verified by reading
   `disposition_workbook_state.rs:201-203`); "invalid correction
   reference" is already exhaustively covered by the existing
   `correction_history.rs` target. Use independent expected
   classifications for hostile axes; do not treat every `Err` as success.
   Do not re-add P2/P3/P4/P6 assertions that already exist in sibling
   targets against the same production boundary — port the pattern into
   this target's own tape/assertions instead of re-deriving it.
5. Add small, named seeds covering each new utterance family and each new
   hostile-axis family, on top of the 4 seeds `history_belief_state.rs`
   already has. Seed input must be valid for the target grammar and
   reviewable as source bytes/text.

### Public-surface rule

Do **not** add `pub`, `pub(crate)`, feature-gated test hooks, fixture exports,
or a model adapter merely for this target. If a necessary core composition step
is not externally callable, U1 stops and reports it. Peer review must decide
whether it is a real supported capability or should remain un-fuzzed from the
external cargo-fuzz harness.

### Required verification

- `cargo check --manifest-path utterance-engine/fuzz/Cargo.toml --bins --locked`
- Target build under the repository-pinned nightly/cargo-fuzz toolchain.
- Isolated seed replay and a bounded live run with final libFuzzer statistics.
- Existing `phrase_index`, `evidence_fusion`, `game_turn_replay`,
  `disposition_workbook_state`, and `preview_compilation` regression replays.

### Gate U1

Every valid seed satisfies P1–P6. Every hostile seed reaches its intended
refusal rather than a generic early failure. The public API diff is empty.

---

## U2 — Regression governance and CI

**Tier:** GRIND, authorship-blind review at close.

### Work

1. Add the target to the existing PR-time smoke discipline with a temporary,
   isolated corpus and bounded `-runs`/`-max_len`; do not let CI mutate the
   committed seed directory.
2. Rely on the existing nightly target discovery for the 20-minute evolving
   fuzz run; prove that the new `[[bin]]` is discovered.
3. Add semantic counters only where they prove a specific family reached a
   real branch. Counters are observability, never a substitute for P1–P6
   assertions.
4. Add an entry to the fuzz-coverage receipt documenting seeds, limits,
   coverage dimensions, command output, and any discovered reduction.
5. If a crash or assertion failure is found, minimise it, add it as a governed
   regression input, and repair the production invariant in a separate
   remediation commit before closing this tranche.

### Gate U2

PR smoke, regression replay, nightly discovery, and an isolated live fuzz run
are evidenced. The target has no model/network dependency and does not write a
graph/session outside its own local fixture.

---

## U3 — Server-owned compound/workbook composition decision

**Tier:** CAREFUL. **Default disposition:** defer rather than widen visibility.

### Work

1. Benchmark a hermetic router-level harness over a memory-backed
   `DesignerState`, explicit workbook rollout, no model environment, and a
   fixed graph-backed session. The input remains the U1 bounded phrase family,
   not arbitrary request JSON.
2. Measure setup cost, iterations/second, allocation growth, and whether the
   harness can reset all in-memory proposal/session state per iteration.
3. If it is fast and hermetic enough, add a server fuzz target invoking only the
   existing public router/API. Assert P7 and that no ratification/GraphEdit is
   persisted. Give it a separate small CI smoke budget from U1.
4. If it is not fast enough, retain the existing core target plus
   `preview_compilation` and add focused server property tests for
   `resolve_compound_chain` / `start_workbook`. Record the reason as a
   reviewed non-fuzzable integration boundary.

### Prohibited shortcut

Do not make `resolve_compound_chain`, `start_workbook`, `PendingProposal`, or
any server test helper public solely to enable cargo-fuzz. The application
router is the only allowed external server entry point. A proposed production
facade requires a separate capability/API review and is out of scope here.

### Gate U3

Peer review accepts either the bounded black-box target with evidence of useful
throughput, or the explicit deferred disposition with property-test coverage.
No visibility change is accepted as a substitute for this decision.

---

## U4 — Graph-Repl ↔ DSL-AST equivalence (blocked future tranche)

**Status:** BLOCKED BY DESIGN — no production graph-to-DSL bridge exists.

### Precondition

Before U4 can be opened, a separately reviewed implementation plan must define
and land a genuine production capability that projects an admitted
`DesignerDag` or operation-applied graph to canonical `bpmn-dsl` source or a
stable DSL AST. That plan must state:

- the owner crate and supported public entry point;
- source-format canonicalisation and identifier/order rules;
- which graph constructs are representable and the typed refusal for those
  that are not;
- the equivalence relation (canonical IR, `WorkflowExecutionPlan`, or another
  explicit semantic contract);
- round-trip/version compatibility and diagnostic ownership.

Do not infer that such a projection already exists from `DesignerDag::to_ir()`:
IR projection is not textual DSL emission or AST construction.

### Work, once unblocked

1. Add a dedicated fuzz target owned by the new bridge's capability crate—not
   by `utterance-engine` merely because an utterance initiated the operation.
2. Generate admitted operation tapes/Designer DAGs using the same bounded
   fixture discipline as U1, then project to DSL source/AST and compile through
   the real `dsl::compile` route.
3. Assert P8 after every generated tape. Include a one-axis hostile mode for
   unsupported constructs, stale identity, invalid serialisation, and
   non-canonical order.
4. Differentially compare the graph-Repl compiler path and DSL compiler path
   only where the bridge declares them equivalent. Do not paper over an
   unsupported construct by silently dropping it from DSL output.

### Gate U4

The bridge is a real supported capability with its own API review, and the
fuzzer proves equivalence/refusal against that contract. `cargo public-api`
records and peer review approve any new bridge surface. Otherwise this tranche
remains blocked and `dsl_compile` remains the complete fuzz owner for textual
DSL AST admission.

---

## 7. Stop conditions

Stop and return to review if execution finds:

- the target needs a live model, network, database, or non-hermetic clock;
- a test-only public API would be required;
- a fixture cannot exercise a real public production path without copying
  production logic into the fuzzer;
- an invariant needs an unspecified semantic accuracy oracle rather than a
  deterministic contract;
- an existing fuzz target already proves the same property at the same seam;
- a target mutates a real or persistent design session;
- expected fuzz throughput is too low for coverage-guided search.
- a proposed U4 implementation tries to use `DesignerDag::to_ir()` as though it
  were a DSL serializer, or requires an invented graph-to-DSL bridge.

---

## 8. Tranche receipt template

```markdown
## U<id> receipt

- Scope delivered:
- Target(s) and owner crate:
- Public API diff: none / approved capability change
- Input grammar, maximum size, and fixture catalogue:
- Valid and hostile families reached:
- Invariants asserted:
- Seeds/regressions added or minimised:
- Focused checks and live-fuzz command/statistics:
- PR smoke and nightly-discovery result:
- Blind peer-review findings and dispositions:
- STOP-gate decision: accepted / blocked / deferred
```

---

## 9. Pre-execution review checklist

- [ ] Ratify the core target boundary and the explicit exclusion of live models.
- [ ] Ratify P1–P6 and the one-axis hostile-input methodology.
- [ ] Confirm the fixture/utterance family is representative without claiming
      natural-language coverage.
- [ ] Confirm zero new public or test-only API is acceptable for U1.
- [ ] Approve the PR smoke time/resource budget and nightly discovery policy.
- [ ] Decide whether U3's router benchmark is warranted now or should be
      deferred after U2.
- [ ] Confirm no proposal may be ratified or persisted by either fuzz target.
- [ ] Ratify that U0–U3 cover the graph-Repl route, while `dsl_compile` owns
      raw DSL source/AST fuzzing today.
- [ ] Confirm U4 is blocked until a separate product decision creates a real
      graph-Repl ↔ DSL-AST projection contract.

**Status:** no implementation approved. Begin with U0 only after peer review.
