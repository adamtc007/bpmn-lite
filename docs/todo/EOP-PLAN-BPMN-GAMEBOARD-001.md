# EOP-PLAN-BPMN-GAMEBOARD-001 — Refactor BPMN-Lite around the design-game model

**Version:** v0.13
**Status:** IMPLEMENTATION PLAN — blocked only on ratification of the companion vision
**Date:** 2026-08-10
**Vision:** `docs/todo/EOP-VS-BPMN-GAMEBOARD-001.md`
**Coordinating repository:** `/Users/adamtc007/dev/bpmn-lite`
**Reviewed baseline:** `feat/dir-002-phase-c-slm-training` at `22ba055`

**v0.13 amendment — performance-budget measurement harness added:**
`docs/receipts/semantic-gameboard-phase8-perf-budget-2026-08-10.md` adds
`utterance-engine/benches/gameboard_perf.rs`, closing the *measurement* half of
Phase 8's performance-budget bullet (legal move enumeration, full disposition, belief
update, rule/feedback retrieval, serialized position/evidence size). It does not close
Gate 8's "P95 latency meets the ratified budget" - no budget numbers are ratified
anywhere in this repo, and inventing thresholds unilaterally would be deciding a fork
that isn't this agent's to decide. Preview-compilation latency and learned-lane
scoring latency are named as deliberately deferred, with reasons, not silently
dropped. Only PostgreSQL fault-tape replay remains completely untouched in Phase 8.

**v0.12 amendment — remaining Phase 8 property bullets audited and closed:**
`docs/receipts/semantic-gameboard-phase8-property-audit-2026-08-10.md` audits the 11
property bullets left open by v0.11 rather than writing 11 new tests unconditionally: 7
already had real coverage (fuzz targets or Phase 7's fault-tape suite, cited not
duplicated), 1 is type-guaranteed (needs no test), 2 genuine gaps closed with new
`proptest!` cases (`feedback_recoveries_resolve_to_legal_moves_or_governed_focus_change`,
`policy_hidden_explanation_never_names_the_hidden_candidate`), and 1 ("production and
reference-model outcomes agree... not only at final state") is left explicitly open —
the differential methodology is proven in two fuzz targets but not claimed universal.
13 of ~15 Phase 8 property bullets are now closed. Performance budgets and PostgreSQL
fault-tape replay remain the only untouched Phase 8 work.

**v0.11 amendment — Phase 8 fuzz-target and property-test tranches closed:** both
`docs/receipts/semantic-gameboard-phase8-fuzz-target-tranche-2026-08-10.md` (the 5
missing fuzz targets) and
`docs/receipts/semantic-gameboard-phase8-property-tests-2026-08-10.md` (4 new
`proptest!` cases grounding "legal move set deterministic/canonically ordered",
"move-set hash sensitive to focus/policy/revision/profile drift", "history/belief
cannot change legality", "off-board/duplicate/incomplete evidence always refused") are
now closed. The property-test receipt corrects an overstatement in the fuzz-target
receipt: the pre-existing `property_tests.rs` cases all test the pre-gameboard
compatibility surface, not the gameboard model — none of the ~15 Phase 8 property
bullets had genuine coverage before this tranche; 4 are closed now, ~11 remain open
(several already have *some* non-`proptest!` coverage from unit tests or the fuzz
tranche, not re-audited here). Performance budgets and PostgreSQL fault-tape replay
remain untouched.

**v0.10 amendment — Phase 8 scope rulings:** Phase 7 closed GREEN
(`docs/receipts/semantic-gameboard-phase7-gate-2026-08-10.md`); Phase 8 (§14) starts
with two ownership decisions rather than blind execution of every listed bullet.
(1) The differential-test bullets "native versus Wasm compilation/admission where
supported" and "Python versus Candle learned-lane parity" name infrastructure that does
not exist anywhere in this product today — no `wasm32` build target, no `pyo3` binding
between `utterance-engine/train_py/*.py` and the Rust runtime. Building either would be
new product infrastructure adopted solely to satisfy a qualification-phase checklist
bullet, not a qualification pass over something that exists. Ruled: **out of scope,
marked N/A** in the Phase 8 gate with this reasoning recorded, not silently dropped.
(2) Given the phase's real size (11 named fuzz targets, ~15 property invariants, perf
budgets with zero existing benches for any gameboard crate, PostgreSQL fault-tape
replay), this pass is scoped to the fuzz-target gap first: of 11 named targets, 2 exist
exact-match, 4 have partial overlap, 5 are genuinely missing
(`clarification_policy`, `move_attempt_feedback`, `correction_history`,
`rule_explanation_decode`, `game_turn_replay`). Property-test invariant coverage,
performance budgets and PostgreSQL fault-tape replay remain open Phase 8 work, not
closed by this amendment.

**v0.9 amendment:** closes the multi-operation tranche deferred by v0.8 — extends
`recover_candidate_shape` from `&Operation` to `&[Operation]` and adds 5 mechanical
multi-op arms (`op.attach_guard`, `op.attach_rearming_guard`, `prod.request_and_wait`,
`prod.interrupting_timeout`, plus the ambiguous pair below), all reusing the same
`ir_graphs_equivalent`/`apply_production` machinery unmodified (it already compares
resulting graph state, not operation count). One genuine fork surfaced and ruled:
`prod.reminder_then_escalate` and `prod.non_interrupting_notification` materialize
byte-for-byte identical `Vec<Operation>` shapes — nothing in the operation content
distinguishes them — so recovery fails closed (`ShapeRefusal::Ambiguous` →
`"ambiguous_candidate_shape"`) rather than guessing a label the content can't prove.
Full contract under Phase 2 item 9's v0.9 amendment.

**v0.8 amendment:** generalizes Phase 2 item 9's direct-edit equivalence beyond
`op.delete_subgraph` to the 12 other single-`Operation` candidates via a
recover-synthesize-materialize-compare reverse-materializer, comparing resulting
`to_ir()` graph state rather than raw edit/`Operation` representations. Full contract
under Phase 2 item 9.

**v0.7 owner-authorized amendment:** the session unit Sage and the REPL author and
converse in is the pack-level runbook — a template invocation (explicit, power-user) or
a matched motif (inferred, generic-utterance) — not the atomic DSL verb. This clarifies
emphasis, not authority: every step inside a runbook still resolves through the
unchanged atomic preview/compiler-admission/ratification path (I-4, I-12). It is chess,
and the sealed pack DAG (SESE, exact pins) is the rulebook, not the line: the DAG bounds
which move *combinations* are structurally possible, but is not itself a line-legality
oracle. A `CompoundPlan`/motif/template line must be chain-previewed — each hypothetical
step re-verified against the *resulting* position of the prior hypothetical step through
the same non-mutating admission boundary — before any step is offered; reachable-in-the-
DAG is necessary but not sufficient, the same standard the keystone already sets for a
single move. Phase 9 rollout gates by utterance-style/user-population in addition to
capability surface: the deterministic power-user runbook-dictation tier is the REPL
baseline and must be live and stable before generic non-power-user, Candle/SLM-assisted
utterance interpretation is exposed to that population. This does not reorder Phase 0-6
engineering, which builds the complete evidence stack — deterministic and statistical —
regardless of rollout sequencing, and does not block Phase 7.

**v0.4 amendment:** capability/crate visibility is a release boundary. Only audited
facades and stable contracts are `pub`; implementation defaults to `pub(crate)` or
narrower. Tests, fuzz harnesses, examples, benches and `xtask` use the same supported
facades as applications and cannot force public-scope growth.

**v0.3 amendment:** fuzzing is distributed through every phase as a design gate. Phase
8 is integration and durability qualification, not the point at which fuzzability is
first added.

**v0.5 owner-authorized amendment:** Gate 6 is split into a structural-infrastructure
gate and a promotion-evidence lane. A green structural gate permits Phase 7 to proceed
without live user-test data, provided unavailable real-turn measures are explicitly
receipted as not measured. The data lane remains mandatory before any learned-policy
promotion or release: it cannot be satisfied by synthetic fixtures, training corpora or
unadjudicated interaction logs.

## 1. Objective

Refactor BPMN-Lite Designer so that the complete session is represented as a
compiler-governed design position and every interaction ranks, completes, clarifies,
previews or applies typed legal graph moves.

The programme must replace the current effective shape:

```text
utterance -> candidate verb ranking -> top-one disposition -> workbook -> AstMutator
```

with:

```text
canonical graph + focus + history + semantic snapshot
    -> deterministic concrete legal moves
    -> per-move graph/language/argument/history evidence
    -> versioned fusion and belief update
    -> propose | clarify | feedback | abstain | palette
    -> typed completion and preview
    -> human ratification
    -> production compile/admission OR typed non-transition outcome
    -> next canonical graph revision + next session/learning state
```

This is a refactor of the design and resolver plane. It is not a rewrite of the
runtime, compiler, verifier or persistence kernel.

## 2. Opening procedure for every implementation session

The repository currently contains concurrent training and server-runner work. Every
implementation session must begin with:

1. record branch, HEAD and upstream;
2. record `git status --short` and preserve every pre-existing modification;
3. identify files owned by concurrent work;
4. do not regenerate corpora, bundles, reports or lockfiles unless the active phase
   explicitly requires it;
5. do not discard, stage or reformat unrelated changes;
6. establish a red test before changing behaviour;
7. finish with an exact changed-file ledger and commands/receipts.

At plan authorship, known unrelated/experimental changes include `.DS_Store`,
`bpmn-lite-server-runner/src/bus_runtime.rs`, v2/v3 corpus artifacts, bundle reports,
training manifests and training logs. They are not part of this programme unless
explicitly adopted through a later receipt.

Use one branch for the programme, recommended:

```text
codex/bpmn-gameboard-refactor
```

No phase may mix model-training artifacts with contract or serving refactors in one
commit.

## 3. Non-negotiable invariants

- **I-1 Graph authority:** the canonical graph and revision are the design truth.
- **I-2 Compiler authority:** only production compiler-admitted transformations become
  graph revisions.
- **I-3 Legal before statistical:** candidate moves are generated deterministically
  before any probabilistic scoring.
- **I-4 Same move set:** palette, language, clarification and direct manipulation use
  the same move contracts.
- **I-5 One snapshot:** candidate meaning, phrases, motifs, arguments and applicability
  resolve from one admitted semantic-pack snapshot.
- **I-6 Evidence is not authority:** models and statistical policies emit evidence;
  deterministic policy emits dispositions; users ratify changes.
- **I-7 Complete board:** a scoring producer covers every legal move exactly once or is
  rejected.
- **I-8 No implicit context:** missing focus, anchor or argument remains unknown.
- **I-9 Canonical identity:** state, move set, move, evidence, proposal and delta are
  content-addressed.
- **I-10 Replay:** stored historical decisions consume recorded evidence; model
  re-inference is forensic only.
- **I-11 Configuration ownership:** BPMN semantics and motif definitions live in YAML;
  Rust implements generic typed mechanisms.
- **I-12 No automatic apply:** every graph mutation remains behind preview and explicit
  ratification.
- **I-13 Safe core:** shared/core crates retain `unsafe` prohibition and deterministic
  canonical encoding.
- **I-14 Backward safety:** legacy sessions degrade explicitly; a v3/gameboard bundle
  can never be scored through a legacy textualisation.
- **I-15 Wrong moves are normal:** every attempt receives a typed outcome and useful
  governed feedback; expected human iteration is not an opaque exception path.
- **I-16 State separation:** refused attempts advance session history/learning state but
  never authoritative graph state.
- **I-17 Correction provenance:** legal-but-unwanted moves are corrected through linked
  undo/replacement moves; history is not rewritten.
- **I-18 Retrievable rules:** Sage and UI obtain pieces, rules, explanations and recovery
  options through typed SemOS/Repl capability APIs, not parsed Rust errors.
- **I-19 Federated reuse:** shared contracts support multiple bounded domain boards and
  governed board transitions without containing BPMN or `ob-poc` vocabulary.
- **I-20 Fuzzable by construction:** the game kernel is pure over explicit inputs; each
  new contract and state transition lands with generators, shrinkable operation tapes,
  invariants, target receipts and regression governance.
- **I-21 Capability-scoped visibility:** `pub` is restricted to explicitly reviewed
  facade/contract items; implementation defaults to `pub(crate)` or narrower, including
  code used by tests, fuzzers and tooling.

## 4. Target contracts

Names are provisional but responsibilities are normative.

### 4.1 Shared domain-neutral contracts

Preferred owner: `/Users/adamtc007/dev/dsl/crates/semantic-decision-contracts`.

```rust
pub struct DesignStateId(String);
pub struct GameDomainId(String);
pub struct BoardPath(Vec<String>);
pub struct LegalMoveId(String);
pub struct MoveAttemptId(String);
pub struct MoveSetHash(String);
pub struct DesignTurnId(String);
pub struct GraphDeltaHash(String);
pub struct RuleExplanationId(String);

pub struct DesignPosition {
    schema_version: u32,
    state_id: DesignStateId,
    domain: GameDomainId,
    board_path: BoardPath,
    semantic_snapshot: SnapshotIdentity,
    graph_revision: GraphRevision,
    graph_hash: String,
    focus: DesignFocus,
    history_hash: String,
    legal_moves: Vec<LegalMove>,
    move_set_hash: MoveSetHash,
}

pub struct LegalMove {
    move_id: LegalMoveId,
    candidate_id: CanonicalCandidateId,
    anchor: Option<GraphElementRef>,
    arguments: Vec<MoveArgument>,
    binding_state: MoveBindingState,
    applicability: Vec<ApplicabilityFact>,
    preview: Option<GraphDeltaPreview>,
    semantic_hash: String,
}

pub struct MoveEvidence {
    move_id: LegalMoveId,
    lanes: Vec<LaneScore>,
    final_score: FiniteScore,
    probability: FiniteScore,
    explanation_codes: Vec<String>,
}

pub struct DesignBelief {
    position_id: DesignStateId,
    likely_moves: Vec<MoveProbability>,
    motifs: Vec<MotifHypothesis>,
    unresolved_dimensions: Vec<UnresolvedDimension>,
    producer_hash: String,
    belief_hash: String,
}

pub enum MoveAttemptOutcome {
    Applied,
    Incomplete,
    Ambiguous,
    Inapplicable,
    DisclosureSafeRefusal,
    Stale,
    CompilerRefused,
    RejectedByUser,
    Corrected,
    SystemFailure,
}

pub struct MoveAttemptReceipt {
    attempt_id: MoveAttemptId,
    position_id: DesignStateId,
    attempted_move: Option<LegalMoveId>,
    outcome: MoveAttemptOutcome,
    rule_explanations: Vec<RuleExplanationId>,
    feedback_options: Vec<FeedbackOption>,
    correction_of: Option<MoveAttemptId>,
    receipt_hash: String,
}

pub struct RuleExplanation {
    explanation_id: RuleExplanationId,
    rule_code: String,
    message_key: String,
    parameters: Vec<ExplanationParameter>,
    provenance: String,
    disclosure: DisclosureClass,
}

pub struct FeedbackOption {
    kind: FeedbackOptionKind,
    move_id: Option<LegalMoveId>,
    prompt_key: String,
    rule_explanation: Option<RuleExplanationId>,
}
```

These are public contract types, not public representations. Fields remain private;
validated constructors, read-only accessors and serde implementations preserve
invariants. Public fields require a specific versioned wire-contract justification.

The shared crate must contain no BPMN candidate names, workflow motifs or UI code.

### 4.2 BPMN adapter contracts

Owner: `utterance-engine` plus `designer-graph`/compiler integration.

Responsibilities:

- project a Designer graph and focus into a canonical `DesignPosition`;
- enumerate concrete BPMN `LegalMove`s from the semantic candidates;
- bind anchors and arguments;
- calculate graph-local and motif features;
- preview moves through the existing mutation machinery;
- prove preview admission through the production parser/compiler/verifier path;
- chain-preview a pack-level runbook line (template invocation or matched motif) by
  walking each hypothetical step's *resulting* position through the same non-mutating
  preview/admission path, proving the whole line before any step is offered — not
  merely that each move is legal in isolation against the current real position;
- translate a ratified move into the existing `AstMutator` operation.

### 4.3 Host/session contracts

Owner: `bpmn-lite-server-designer`.

Responsibilities:

- persist the authoritative graph revision and append-only design turns;
- track explicit focus and proposal state;
- store recorded evidence/belief as historical facts;
- expose the palette, proposal, clarification and preview APIs;
- expose board/piece/rule discovery, attempt evaluation and feedback APIs for Sage and
  other application surfaces;
- persist non-transition attempt receipts and correction links;
- preserve rollout, consent, identity and access controls.

### 4.4 Required public semantic capability API

The shared SemOS/DSL boundary must allow consumers to implement, under policy:

```rust
fn describe_board(position: &DesignPosition) -> BoardDescription;
fn enumerate_moves(position: &DesignPosition) -> Vec<LegalMove>;
fn explain_move(position: &DesignPosition, move_ref: MoveRef)
    -> MoveApplicabilityExplanation;
fn evaluate_attempt(position: &DesignPosition, attempt: MoveAttempt)
    -> MoveAttemptReceipt;
fn feedback_for(receipt: &MoveAttemptReceipt) -> Vec<FeedbackOption>;
fn preview_move(position: &DesignPosition, bound: BoundMove)
    -> TransitionPreview;
```

Names may change, but equivalent typed capability must exist. A boolean `is_legal` or
unstructured compiler error is insufficient. Responses must be pack/snapshot-bound,
content-addressed and filtered by disclosure policy.

### 4.5 Target Rust capability boundary

The target is a small public facade over private implementation, not a public module
tree.

| Crate/capability | Permitted public responsibility | Internal by default |
|---|---|---|
| `semantic-decision-contracts` | Versioned cross-crate identities, board/move/attempt/evidence/feedback wire contracts | Validation helpers, canonical preimage builders, migrations |
| semantic-pack capability | Admit/resolve versioned packs and return typed governed contracts | YAML parsing stages, indices, caches, diagnostics assembly |
| `utterance-engine` gameboard facade | Construct position, evaluate observation/attempt, explain, preview, decide and replay evidence | board builders, lane producers, fusion, motif/history algorithms, ranker adapters |
| `designer-graph` BPMN adapter facade | Canonical graph position, legal concrete moves and delta preview/apply contracts | positional traversal, operation-specific enumeration and mutation helpers |
| `bpmn-lite-compiler` facade | Parse/compile/admit/verify through supported entry points | lowering, verifier passes, graph algorithms and intermediate forms unless already contractual |
| `bpmn-lite-server-designer` application | HTTP/session DTOs, composition, persistence and rollout | host orchestration, pending state and endpoint helpers |
| `xtask` | Invoke supported commands/facades, schedule tests/fuzz, validate receipts/artifacts | No domain semantics and no dependency on internal modules |

Required visibility rules:

1. Crate roots expose named facade items explicitly. Avoid `pub mod implementation`
   and prohibit `pub use module::*`.
2. Cross-crate types live in the narrowest owning contract/facade crate. Do not make an
   internal type public and then treat its accidental shape as a contract.
3. `pub(crate)` is the maximum default for implementation modules; use private or
   `pub(super)` wherever possible.
4. Traits not intended for consumer implementation are sealed. Constructors enforce
   invariants rather than exposing public fields for convenience.
5. Integration tests exercise the public facade. White-box unit tests remain beside
   private modules and use ordinary Rust child-module visibility.
6. Fuzz projects link the production facade and admission APIs. Reference models and
   generators live in the fuzz project or a dedicated non-release test-support crate;
   no `cfg(fuzzing)` production bypass is permitted.
7. Examples and benches compile against the documented facade only.
8. `xtask` orchestrates binaries/facades and files. If it needs an internal symbol, add
   a real supported capability operation or redesign the task—do not widen visibility.
9. Feature combinations used only by tests/tooling cannot change the non-test public
   API or enable otherwise-unavailable authority.
10. The application composition root may be public at its external API boundary, but
    application DTOs and services do not leak back into capability crates.

## 5. Phase sequence

Dependency spine:

```text
Phase 0 -> Phase 1 -> Phase 2 -> Phase 3 -> Phase 4 -> Phase 5
                                      \-> Phase 6 --------/
Phase 5 + Phase 6 -> Phase 7 -> Phase 8 -> Phase 9
```

No model retraining is required before Phase 6. Phases 0–5 must establish a strong
deterministic and statistical baseline without changing runtime execution semantics.

### 5.1 Fuzz-first engineering contract

Fuzzing is part of each phase's implementation, not deferred to Phase 8. Every new
gameboard type or transition must supply, as appropriate:

- an `Arbitrary`/byte-tape projection that generates valid, boundary and hostile forms;
- a compact reference-model representation independent of the production transition
  implementation;
- a shrinkable operation tape containing explicit clock, identity and fault events;
- invariants checked after every operation, including refused operations;
- canonical serialization suitable for a portable reproducer;
- semantic event counters, not only code coverage;
- a permanent regression location and manifest entry after any real finding;
- a bounded smoke budget for pull requests and an independently receipted nightly
  budget.

Production APIs must not gain fuzz-only authority or bypasses. Fuzz generators construct
through public/admission contracts or test-support builders that cannot enter release
artifacts. Hidden wall clocks, random IDs, global mutable registries and network-bound
core logic are stop conditions; they must be injected or moved behind adapters.

The compact reference model owns only game invariants and abstract state, not a second
copy of compiler implementation details. Differential comparison against this model
must therefore detect production errors rather than reproduce them.

### 5.2 Public API and tooling governance

Before implementation, capture the exported API of every affected library crate and
the dependency graph among applications, capability crates, fuzz projects and `xtask`.
Maintain these as reviewed receipts/allowlists.

Permanent controls:

- enable `unreachable_pub` as deny-by-default in affected capability crates;
- inventory the rustdoc/public API and fail unapproved additions or removals;
- lint `pub mod` and glob `pub use` against a small explicit allowlist;
- run compile-pass consumer fixtures using only facade imports;
- run compile-fail fixtures proving internal module paths and constructors remain
  inaccessible;
- compare public surfaces under default, all production and test/fuzz/tooling feature
  sets; tests and tooling may not widen production API;
- enforce dependency direction from application/tooling to facade to implementation;
  no capability crate depends on an application, fuzz target or `xtask`;
- inspect `xtask`, example, bench and fuzz dependencies and imports as part of the gate;
- use semver/API-diff tooling before shared capability releases.

A request to make an implementation item `pub` must name its external consumer,
capability contract, stability expectation and owning facade. “The test/fuzzer/xtask
needs it” is not sufficient justification.

## 6. Phase 0 — Freeze claims and repair the measurement instrument

### Purpose

The current starter-seed evaluator builds a legacy thin board, records
`semantic_v3: None`, and invokes `TrainedRanker::score`, while production v3 uses a
semantic board and `rank_full_board`. No refactor may use the reported 7/34 result as a
baseline until this is corrected.

### Work

1. Add an explicit bundle/input-generation mode to `TrainedRanker`.
2. Make legacy `score`/`score_list` reject any bundle whose card declares the v3 pair
   serializer.
3. Replace the starter evaluator's `build_board`/`pack.none` path with
   `build_bpmn_semantic_board` using the same semantic snapshot and context projection
   as production.
4. Route evaluation through the production-equivalent full-board scoring,
   `finalize_semantic_evidence`, and `decide_with_action_spans` path.
5. Record candidate-pair hashes, move/candidate list, board hash, exact evidence,
   disposition and producer identities for every test turn.
6. Add a Python-versus-Candle parity packet over fixed v3 pairs and assert ranking
   equality plus numerical tolerance.
7. Re-run the frozen 34 utterances, preserving the old result as invalidated historical
   evidence rather than overwriting it.
8. Amend the 2026-08-07 peer review: withdraw the claimed live-v3 cliff unless the
   corrected instrument reproduces it.
9. Run `cargo xtask fuzz list` and freeze the discovered-target manifest, committed
   regression inventory, CI sharding and receipt completeness as the gameboard fuzz
   baseline.
10. Add a bounded route/admission fuzz target proving that any v3 bundle/input packet
    either reaches the production pair serializer or is typedly refused; it can never
    fall through to legacy text.
11. Capture the affected crates' public API and dependency-direction baseline. Identify
    exported implementation modules, test/fuzz-only visibility and `xtask` imports that
    bypass a capability facade; record dispositions without broad cleanup in Phase 0.

### Primary files

- `utterance-engine/examples/starter_seed_eval.rs`
- `utterance-engine/src/trained_ranker.rs`
- `utterance-engine/src/bpmn_board.rs`
- `utterance-engine/src/exact.rs`
- `utterance-engine/src/policy.rs`
- `bpmn-lite-server-designer/src/rest.rs`
- `utterance-engine/train_py/eval_stored_pairs.py`
- `docs/reviews/utterance-resolver-findings-peer-review-2026-08-07.md`

### Gate 0

- Evaluator and live serving produce identical board and pair hashes for every cement
  packet.
- A v3 bundle is mechanically unable to enter a v2 scoring path.
- Python and Candle agree within the declared tolerance.
- Corrected top-1/top-3/NOTA/disposition baseline is published with sample counts.
- The current fuzz manifest has one independently completable receipt path per target,
  zero successful empty-regression skips and no unaccounted project corpus/artifact
  paths.
- The new route/admission target completes its smoke budget and has a committed seed
  exercising v3/legacy mismatch refusal.
- Public API/dependency receipts identify every existing `pub` item by facade/contract
  or remediation owner, and prove Phase 0 introduced no visibility expansion.
- No corpus or bundle change is included in the phase.

## 7. Phase 1 — Introduce the design-position and move contracts

### Purpose

Represent what the session actually is before changing ranking behaviour.

### Work

1. Extend `semantic-decision-contracts` with the domain-neutral contracts in §4.1.
2. Define canonical encodings and content hashes for domain/board path, focus,
   position, legal move, move set, attempt, feedback, correction, delta preview,
   evidence and belief.
3. Add schema versions and strict deserialization validation.
4. Define `DesignFocus` so absence is explicit; do not default to the most recent or
   first node without recording a policy decision.
5. Define append-only `DesignTurn` events:
   - input observed;
   - focus changed;
   - board constructed;
   - evidence recorded;
   - clarification asked/answered;
   - move proposed/rejected/ratified;
   - attempt incomplete/inapplicable/stale/refused;
   - feedback options presented and selected;
   - compile refused;
   - graph revision committed;
   - move undone/corrected, linked to the original attempt.
6. Release and pin a new shared-contract revision before BPMN-Lite consumes it.
7. Add compatibility adapters from the current `SemanticDecisionBoard`,
   `InferenceEvidence`, `DecisionRecord` and `ProposalWorkbook`.
8. Add the generic rule-explanation, feedback-option and disclosure contracts required
   by §4.4.
9. Ensure board/domain identities support later `ob-poc` subdomain federation without
   importing `ob-poc` vocabulary.
10. Add contract fuzz targets for design positions, legal moves, attempts, rule
    explanations, feedback options and belief records. Decode hostile bytes, then
    canonical round-trip every admitted value.
11. Define the shared crate's explicit facade exports. Keep canonical-preimage,
    validation, migration and builder machinery private/`pub(crate)`; seal extension
    traits not intended for application implementation.
12. Add `unreachable_pub`, public-API snapshot, compile-pass facade consumer and
    compile-fail internal-path fixtures to the shared release gate.

### Tests

- canonical round-trip and golden bytes;
- hash changes for every authority-bearing field;
- permutation invariance where order is semantically irrelevant;
- duplicate move and non-finite evidence refusal;
- unknown focus round-trip;
- every attempt outcome round-trip, including non-transition outcomes;
- correction-link integrity and cycle refusal;
- disclosure-class round-trip and filtering;
- old decision-record compatibility fixtures;
- no BPMN/application vocabulary in the shared crate.

### Gate 1

- Shared crate is independently testable, documented, MIT-licensed and pinned by exact
  revision.
- A Designer request can expose a serialized `DesignPosition` without changing current
  proposal behaviour.
- Every attempted interaction can be represented by a typed receipt even when no graph
  transition occurs.
- Existing board/workbook/audit tests remain green.
- Contract fuzz targets complete smoke budgets, expose semantic counters for every
  attempt outcome/disclosure class and commit any minimized findings under the governed
  regression layout.
- Shared public API contains only reviewed cross-crate contracts/facade operations;
  default/test/fuzz feature builds expose the same production surface.

## 8. Phase 2 — Build the deterministic legal-move engine

### Purpose

Widen the current semantic candidate board into concrete, position-bound graph moves.

### Work

1. Introduce `utterance-engine/src/game_state.rs` and
   `utterance-engine/src/legal_moves.rs`.
2. Refactor `build_bpmn_semantic_board` so it remains the semantic candidate source but
   feeds a concrete move enumerator.
3. For every candidate emitted by `PositionalLegality`, enumerate valid anchors and
   known typed bindings for the current graph/focus.
4. Represent missing arguments as partially bound moves rather than inventing values.
5. Create a non-mutating `AstMutator` preview API that returns a canonical graph delta.
6. Dry-apply each fully bound preview to a clone and pass it through the same production
   parse/compile/admission boundary used after ratification.
7. Exclude or mark refused any move that fails preview admission; never silently offer
   it as legal.
8. Canonically order moves and calculate `MoveSetHash` from graph revision, focus,
   semantic snapshot, policy and move contents.
9. Make direct BPMN/DSL graph editing the semantic-IDE baseline. For every supported
   raw operation or production tape, deterministically attempt exact equivalence against
   the current `DesignPosition`; a resolution must carry the same candidate, typed
   bindings, `LegalMoveId`, preview, compiler admission result and receipt as the
   palette/language path. Permit a lower-level audited edit only when no admitted
   semantic counterpart exists or proof fails, with a typed non-equivalence reason.

   **v0.8 amendment — reverse-materializer contract for single-operation tapes.**
   `bpmn_legal_move_id_for_operation`'s original strategy (search `position.legal_moves()`
   for a `MoveBindingState::Complete` move) only ever worked for `op.delete_subgraph`: the
   board auto-binds only the anchor argument, so `delete_subgraph` — the one candidate
   with no other required argument — is structurally the only candidate that can reach
   `Complete` at the position layer. Generalizing to the other single-`Operation`
   candidates (`append_node`, `insert_before`, `insert_after`, `replace_node`, `connect`,
   `create_branch`, `create_parallel_region`, `create_inclusive_region`,
   `create_multi_instance_region`, `set_guard_trigger`, `set_guard_budget`,
   `set_correlation_source`) requires a different mechanism, not an extended search:

   1. `recover_candidate_shape(operation) -> Option<{candidate_id, anchor, arguments}>` —
      one structural arm per `Operation` variant, pulling typed argument values straight
      out of the operation's own content fields (e.g. `ReplaceNode.node` → the
      `replacement` identifier; `CreateBranch.condition` → the `outcome` identifier,
      refused unless its shape is exactly the `Eq/Bool(true)` form the materializer always
      emits). Workbook-synthesized-only fields (`key`, `edge_id`, `guard_id`, `fork_key`,
      `join_key`, `entry_edge_id`, `in_edge_id`, `out_edge_id`) are never part of the
      recovered shape.
   2. Locate the matching `LegalMove` (candidate id + anchor; `Incomplete` binding is
      expected), build a `ProposalWorkbook` directly against its argument slots (anchor
      slot pre-resolved, everything else `Missing` — no utterance-lexical extraction,
      since a direct edit has no utterance), then drive it through the production
      `apply_explicit_answers` typed-answer validation (`bpmn-lite-server-designer/src/proposal.rs`)
      with the recovered values as the answer batch — the same validation
      `POST .../answers` runs, not a re-implementation.
   3. Materialize the completed workbook through
      `utterance_engine::bpmn_board::materialize_bpmn_workbook` — the single production
      materializer (`proposal::materialize_operations` is test-only and itself delegates
      to this facade; there is no second materializer to diverge from).
   4. Apply both the submitted raw operation(s) and the materialized operation(s) to
      separate clones of the *same* base `DesignerDag`, reconstruct each via `to_ir()`,
      and compare the two resulting `IRGraph`s structurally by BPMN element id — same
      node set, same per-node `IRNode` content, same edges by
      `(from_bpmn_id, to_bpmn_id, condition)`. This is a resulting-*state* comparison, not
      an edit-*representation* comparison — internal `NodeKey` handles and wiring-only
      synthesized ids never enter it, because `to_ir()` never surfaces them as BPMN-visible
      content in the first place.
   5. **Considered and rejected:** comparing `GraphDeltaPreview` values directly (already
      `PartialEq`/`Eq`). Its `payload_hash` is a SHA-256 of the raw serialized `Operation`
      struct (`legal_moves.rs::preview_operations`), including synthesized fields — the
      same problem one layer up, with no partial-match granularity to recover from it.
      Resulting-graph comparison is the only formulation consistent with graph shape (+
      per-node content) being the deterministic source of truth, not the edit command that
      produced it.
   6. Requires adding `PartialEq` to `IRNode`, `TimerSpec`, `ConditionExpr`, `IrLiteral`,
      `Expression`, `FfiInputBinding`, `FfiOutputBinding`, `IREdge`
      (`bpmn-lite-compiler/src/ir.rs`) — mechanical, additive, no behavioural change;
      every field type already supports it (`DataObjectType`/`DataObjectRole` already do).
   7. Multi-operation candidates (`attach_guard`, `attach_rearming_guard`, the 4 `prod.*`
      productions) stay refused by the existing `let [operation] = operations else {...}`
      single-op guard — a separate tranche needing N-op tape comparison, not covered by
      this amendment.

   **v0.9 amendment — multi-operation tranche.** Closes item 7 above.
   `recover_candidate_shape` generalizes from `&Operation` to `&[Operation]`; the
   single-op arms fold into a `[operation] => ...` case unchanged (renamed
   `recover_single_operation_shape`). `resolve_direct_edit`'s `apply_production`/
   `ir_graphs_equivalent` comparison already operated on `&[Operation]` on both sides
   (raw and materialized) — it required *zero* changes, since it compares resulting
   `IRGraph` content, never operation count or identity.

   1. Five candidates are mechanical: `op.attach_guard`, `op.attach_rearming_guard`
      (2-op: `AttachGuard`/`AttachRearmingGuard` chained to an `AppendNode` whose
      `anchor` equals the guard op's minted `key`), `prod.request_and_wait` (2-op:
      chained `InsertAfter`/`InsertAfter` ending in `IRNode::MessageWait`),
      `prod.interrupting_timeout` (3-op: `AttachGuard` chained through two
      `AppendNode`s, the last an `IRNode::End`).
   2. **Fork surfaced and ruled (2026-08-10):** `prod.reminder_then_escalate` and
      `prod.non_interrupting_notification` materialize byte-for-byte identical
      `Vec<Operation>` shapes (`AttachRearmingGuard`/`Cycle` → `AppendNode` → `End`) —
      differing only in which workbook slot *name* supplied the same typed values, never
      reaching the operation content. Ruled: fail closed. A new `ShapeRefusal` enum
      (`NotProducible` | `Ambiguous`) lets `recover_candidate_shape` name this as a
      distinct, real defect class rather than folding it into "no candidate matched";
      `resolve_direct_edit` reports `"ambiguous_candidate_shape"` — never guesses a label
      the content can't prove.
   3. **Test-scope finding, matching the `op.delete_subgraph` precedent:** three of the
      five mechanical candidates' standalone materializations independently fail full
      compiler admission (`DesignerDag::admit`) when applied alone to a plain seeded
      session — `attach_guard`/`attach_rearming_guard` leave the escape task with no
      outgoing edge (unreachable-terminal bytecode lowering failure); `request_and_wait`'s
      `MessageWait.corr_key_source` must reference an `IRNode::DataObject`, and no
      `Operation` variant can create one (`DesignerDag::seed` only, never through
      `/graph-edit`). This is a property of each candidate's own materialized shape,
      orthogonal to this change. Regression coverage for those three is therefore at the
      `recover_candidate_shape` unit level; `prod.interrupting_timeout` (self-contained,
      no external data dependency) and the ambiguous-shape refusal both get full HTTP
      round-trip proof.
10. Add a chain-preview API over an ordered list of hypothetical moves (a template
    invocation or a matched motif line): apply move 1's preview to a clone, derive the
    resulting hypothetical `DesignPosition`, enumerate and preview move 2 against *that*
    position, and so on. The sealed pack DAG (SESE, exact pins) bounds which
    combinations are structurally possible but is not a line-legality oracle; each step
    must independently clear the same production parse/compile/admission boundary as a
    single-move preview. Stop and typedly refuse the line at the first step that fails,
    rather than offering a partially-verified remainder.
11. Add completeness checks between semantic candidates, `OperationKind`, binder
    support and mutation implementations.
12. Retain rule/application facts for both admitted and requested-but-inapplicable
    candidate shapes so `explain_move` can say why an attempt does not fit without
    adding it to the legal move board.
13. Map known `PositionalLegality`, binder and compiler diagnostics to stable rule codes;
    leave unknown diagnostics typed and unmapped rather than fabricating guidance.
14. Generate recovery options only from the current legal move set or an explicit
    governed focus/context transition.
15. Implement a compact abstract design-game reference model covering focus, move
    availability, binding state, graph revision, apply/refuse and correction linkage.
    It must not reuse `AstMutator` or compiler internals.
16. Add structured fuzz targets for legal-move enumeration and preview compilation,
    including chain-preview: byte tapes generate graphs, focus changes, bindings, legal
    and deliberately wrong attempts, and multi-step lines that go legal-legal-illegal at
    an arbitrary depth; compare abstract outcomes after every operation and confirm the
    line refuses at exactly the first illegal step.
17. Keep `game_state`, `legal_moves`, preview and operation-specific implementations
    private or `pub(crate)`. Export only named gameboard/BPMN adapter facade operations
    and stable contracts from crate roots.
18. Place generators/reference models in fuzz or non-release test-support ownership;
    do not expose graph builders or unchecked constructors from production crates.

### Primary files

- `utterance-engine/src/bpmn_board.rs`
- `utterance-engine/src/board.rs`
- `utterance-engine/src/bpmn_pack.rs`
- new `utterance-engine/src/game_state.rs`
- new `utterance-engine/src/legal_moves.rs`
- `bpmn-lite-compiler/src/dsl/refactor.rs`
- `bpmn-lite-compiler/src/dsl/dag.rs`
- `designer-graph` positional and operation modules in the workspace dependency
- `bpmn-lite-server-designer/src/rest.rs`

### Tests

- empty graph positions;
- every supported node/focus kind;
- multiple legal anchors for one semantic candidate;
- partially bound moves;
- stale revision and stale anchor refusal;
- canonical move-set equality across reconstruction;
- preview/apply delta equality;
- every fully bound offered move compiles;
- no hidden or policy-forbidden candidate enters a move;
- inapplicable attempt returns a non-transition receipt and legal recovery options;
- policy-hidden attempt returns disclosure-safe feedback without naming hidden pieces;
- compiler-refused preview preserves graph revision and emits a typed diagnostic;
- fuzz arbitrary graphs/focus against move enumeration and preview;
- chain-preview: an all-legal line previews end to end; a line that goes illegal at step
  N refuses exactly at N and offers nothing beyond it; chain-preview of a single-step
  line is identical to ordinary single-move preview.

### Gate 2

- Palette endpoint and language path observe the same move-set hash.
- Every offered fully bound move dry-compiles.
- A `CompoundPlan`/motif/template line is never offered unless every step has been
  independently chain-previewed against its predecessor's hypothetical resulting
  position; reachable-in-the-DAG is necessary but not sufficient.
- Every existing executable semantic candidate maps to at least one tested move shape.
- No graph mutation or model call is needed to enumerate the legal move board.
- Sage can retrieve a typed explanation and recovery options for every governed
  inapplicability fixture without parsing a Rust error string.
- Model-based fuzzing finds no divergence in legal-move soundness, refusal non-mutation,
  revision updates or correction linkage across the committed smoke corpus.
- Every supported operation kind, anchor shape, binding state and attempt outcome is
  reached by semantic coverage counters.
- Integration tests, examples and fuzz targets compile using only the public adapter
  facade; compile-fail fixtures prove implementation modules remain inaccessible.

## 9. Phase 3 — Make deterministic graph and argument evidence first-class

### Purpose

Replace the exclusive `Candle else embedding else lexical` producer choice with a
complete per-move evidence vector.

### Work

1. Introduce a `MoveEvidenceProducer` interface that returns zero or more typed lane
   scores for every move.
2. Implement governed producers for:
   - exact phrases and explicit candidate/move references;
   - deterministic grammar and negation;
   - typed argument extraction and schema compatibility;
   - graph focus/locality;
   - recent-edit continuity, failed-attempt context and correction evidence;
   - lexical/embedding similarity;
   - current Candle cross-encoder score;
   - abstention.
3. Split extraction from binding: statistical or deterministic parsers may propose
   typed values with provenance; the workbook validates and binds them.
4. Move operation-specific cue definitions, feature declarations, rule explanations,
   feedback/recovery options and clarification wording into the admitted YAML semantic
   pack.
5. Extend pack admission to refuse unknown feature kinds, invalid weights, unresolved
   candidate references and contradictory deterministic gates.
6. Introduce a versioned `EvidenceFusionPolicy` with canonical identity.
7. Start with hand-ratified weights and rule dominance; do not train weights in this
   phase.
8. Materialize the existing shared `CandidateEvidence`/`InferenceEvidence` concept in
   live BPMN serving rather than storing lanes as provenance-only metadata.
9. Record raw lane scores, normalized values, final score and explanation codes.
10. Prevent a rejected or corrected move from becoming positive learning evidence
    merely because it was syntactically legal or briefly applied.
11. Add pack-admission checks that every governed refusal/argument requirement used by
    the BPMN adapter resolves to an explanation and at least one permitted feedback
    disposition; a technical-only fallback remains explicit for genuinely unmapped
    system failures.
12. Add evidence/fusion fuzzing with duplicate, missing, reordered, non-finite and
    extreme lane inputs. Add metamorphic cases for candidate permutation, canonical
    equivalent inputs and irrelevant-history insertion.
13. Fuzz semantic-pack admission for rule explanations, recovery links, feature weights
    and disclosure classes; all dangling or cyclic references must be refused.
14. Keep lane implementations, fusion calculations and ranker adapters crate-private.
    The facade returns stable evidence/decision contracts without exporting producer
    internals.

### Primary files

- `utterance-engine/src/contract.rs`
- `utterance-engine/src/exact.rs`
- `utterance-engine/src/retrieval.rs`
- `utterance-engine/src/trained_ranker.rs`
- new `utterance-engine/src/graph_features.rs`
- new `utterance-engine/src/argument_evidence.rs`
- new `utterance-engine/src/fusion.rs`
- `utterance-engine/src/bpmn_pack.rs`
- BPMN semantic YAML pack and lock
- `bpmn-lite-server-designer/src/rest.rs`

### Gate 3

- Every legal move has a complete evidence vector exactly once.
- Disabling the learned model leaves a useful deterministic resolver and legal palette.
- A duration, count, explicit node reference and governed negative contrast each alter
  the intended move's recorded evidence in cement fixtures.
- A rejection/correction changes subsequent evidence without changing the legal move
  set unless the authoritative graph or focus also changed.
- Policy and evidence hashes reproduce byte-for-byte.
- No application-specific cue is hard-coded in the host server.
- Governed feedback renders from pack resources and stable parameters, not compiler
  message text.
- Evidence fuzzing proves producer order independence, complete move coverage, finite
  scores and inability of evidence to alter the legal move set.
- Pack fuzzing produces no admitted dangling feedback/rule/candidate reference and no
  unbounded explanation expansion.
- Public-API diff contains no new implementation modules, producer types or test hooks.

## 10. Phase 4 — Add design history, motifs and belief state

### Purpose

Represent the user's evolving plan without pretending to know a complete target graph.
Motifs and templates are the two pack-level mechanisms for this: a motif is the
system's *inferred* hypothesis of the runbook a less-explicit user might be mid-way
through; a template (already settled — CLAUDE.md "Template ≠ macro") is a Sage-authored,
compile-validated, hash-frozen runbook the power user *invokes* explicitly. Both are the
pack-level unit Sage and the REPL author and converse in; neither gains apply authority
of its own — every step still resolves through the unchanged atomic preview/admission/
ratification path (I-4, I-12).

### Work

1. Add append-only turn history to graph-backed Designer sessions.
2. Derive a bounded, canonical `HistoryProjection` containing only decision-relevant
   facts; never serialize an unbounded transcript into the ranker.
3. Add a generic semantic-pack section for governed design motifs:
   - motif identity and version;
   - graph preconditions;
   - completion facts;
   - likely legal next candidates;
   - discriminating contrasts;
   - completion/abandonment conditions.
4. Implement a deterministic motif matcher over the current graph.
5. Implement a simple belief updater over likely moves and motifs. Initial form:
   calibrated Bayesian/log-linear update over the Phase 3 evidence vector.
6. Decay or close hypotheses when graph changes contradict them, the user rejects them,
   or their completion condition is met.
7. Record belief snapshots as evidence tied to position and producer hash.
8. Use history for continuity and correction, never as an authorization cache.
9. Bound hypothesis count, history window, serialized size and update time.
10. Represent unsuccessful attempts in the history projection with outcome and rule
    codes; do not collapse them into generic errors or accepted moves.
11. Link a legal-but-unwanted move to its later undo, replacement or corrective move.
12. Track whether a feedback option resolved the attempt, led to another clarification,
    or repeated the same failure; use this only as evidence and product telemetry.
13. Extend the reference-model tape with motif start/advance/abandon, repeated wrong
    attempts, rejection, undo, correction, focus changes and bounded history compaction.
14. Fuzz belief/history update and replay with explicit clocks/IDs; assert deterministic
    hashes, acyclic correction links, bounded memory and unchanged legality.
15. Keep history projection, motif matching and belief algorithms crate-private. Expose
    only stable position/history/belief views required by the application facade.

### Primary files

- new `utterance-engine/src/history.rs`
- new `utterance-engine/src/motifs.rs`
- new `utterance-engine/src/belief.rs`
- `utterance-engine/src/context.rs`
- BPMN semantic YAML pack and lock
- `bpmn-lite-server-designer/src/rest.rs`
- server-side session persistence/audit modules

### Gate 4

- Identical graph/pack/focus/history inputs reproduce identical deterministic features
  and belief updates.
- A multi-step timeout/escalation fixture retains a motif across turns while every
  applied move remains atomic and separately ratified.
- Rejection and undo reduce or close the relevant hypothesis.
- An illegal attempt advances session history while leaving graph revision and move-set
  inputs unchanged.
- A legal-but-unwanted move and its correction remain replayable as two domain
  transitions plus a correction link.
- Repeated failure can change explanation/clarification strategy without weakening the
  governing rule.
- Belief removal has no effect on legality or the ability to use the palette.
- A template invocation and a matched motif expose the same typed pack-level runbook
  view through Sage's API — differing only in provenance (asserted vs. inferred), not
  in shape or in the atomic ratification path underneath.
- Resource bounds are enforced.
- Stateful fuzzing reaches every motif lifecycle and correction outcome, reproduces the
  final state from its minimized tape, and cannot produce unbounded feedback/history
  amplification.
- No history/motif/belief implementation type appears in the public API snapshot.

## 11. Phase 5 — Replace top-one hand-off with game-aware disposition

### Purpose

Choose the safest useful interaction, not merely the highest scalar score.

### Work

1. Replace the current two-score/margin policy with a versioned policy over complete
   move evidence and calibrated probabilities.
2. Support dispositions:
   - `ProposeMove`;
   - `ClarifyMoves` with two or three alternatives;
   - `RequestMoveArguments`;
   - `ExplainAttempt`;
   - `OfferRecoveryMoves`;
   - `OfferCorrection`;
   - `OutOfScope`;
   - `ChangeFocusOrContext`;
   - `Escalate`;
   - `CompoundPlan` containing non-authoritative motif/template steps, each of which
     must already be chain-previewed (Phase 2 item 10) before the disposition is
     offered — a line whose step N fails chain-preview is truncated to N-1 or, if N=1,
     falls back to a single-move disposition; it is never offered with an unverified
     tail.
3. Select clarification questions using expected information gain across unresolved
   move, anchor and argument dimensions.
4. Generate questions only from admitted contrasts and argument schemas.
5. Unify `ProposalWorkbook` with the selected `LegalMove`; the workbook must preserve
   move ID, position ID, graph revision and move-set hash.
6. Preview the completed move and show its graph delta before ratification.
7. On any graph, pack, focus or policy drift, expire the proposal and rebuild the
   position; never rebase it silently.
8. Expose legal alternatives even when language confidence is low.
9. Produce a `MoveAttemptReceipt` for every path, including exact success, ambiguity,
   incomplete binding, inapplicability, user rejection, staleness and compiler refusal.
10. When the user says a previously applied move was wrong, resolve undo/replace/follow-
    up options from the current board and record `correction_of`; never edit history.
11. Give Sage the typed receipt, rule explanations and disclosure-filtered options so it
    can explain and collaborate without inventing legality.
12. Add a disposition/workbook state-machine fuzzer covering score ties, missing
    arguments, hostile clarification answers, wrong moves, feedback selection,
    concurrent revision drift, correction and ratification.
13. Check policy metamorphisms: canonical candidate permutation cannot change the
    semantic outcome; removing evidence cannot create authority; adding a hidden move
    cannot leak it; a stale position can never become applicable through scoring.
14. Expose disposition, attempt, feedback and workbook operations through one named
    capability facade; keep policy machinery and state-machine internals crate-private.

### Primary files

- `utterance-engine/src/policy.rs`
- `utterance-engine/src/disposition.rs`
- new `utterance-engine/src/clarification.rs`
- `bpmn-lite-server-designer/src/proposal.rs`
- `bpmn-lite-server-designer/src/rest.rs`

### Gate 5

- Gold-in-top-three and clarification-success fixtures are independently measured.
- A correct third-ranked move can be surfaced through one governed clarification.
- Confidently wrong mutating proposals remain separately visible and gated.
- No clarification can name a move absent from the recorded move set.
- Every unsuccessful-attempt fixture yields a truthful user response and at least one
  useful next action or an honest terminal explanation.
- No feedback option names a hidden or currently illegal move.
- No `CompoundPlan` step is ever offered without a chain-preview verified against its
  predecessor's actual hypothetical resulting position; reachable-in-the-DAG alone never
  substitutes for that verification.
- A correction is itself previewed, ratified and compiler-admitted.
- Workbook completion, preview, ratification and compiler admission form one stale-safe
  state machine.
- The state-machine target reaches every disposition and attempt outcome, and every
  minimized finding is replayable through the public contract path.
- White-box tests remain colocated with private modules; integration/fuzz tests require
  no visibility widening.

## 12. Phase 6 — Establish the statistical baseline and learning path

### Purpose

Learn only after the correct game state and evidence are observable.

### Work

1. Extend consented capture with the complete game-level turn record:
   position, legal moves, evidence, belief, disposition, answer, chosen move, delta,
   attempt outcome, rule explanations, feedback options, compiler result and later
   correction/undo.
2. Add an adjudication tool for intended move, anchor, arguments, motif and acceptable
   clarification/feedback set. Explicitly distinguish an exploratory human attempt,
   accepted move, accidental move and system misinterpretation.
3. Freeze a real-turn evaluation split by session, time and semantic family.
4. Implement an interpretable conditional-logit/listwise model over the Phase 3 feature
   vectors.
5. Compare four resolvers on identical move boards:
   - deterministic fusion only;
   - deterministic fusion plus existing Candle lane;
   - trained structured-choice weights;
   - bounded prompt ranker as offline evidence.
6. Calibrate by board size and risk class without changing legality.
7. Do not build a graph neural network until the structured baseline and data volume
   demonstrate a material residual graph-representation error.
8. Do not use reinforcement learning unless a later, separately ratified objective and
   offline safety protocol justify it.
9. Add model-boundary fuzzing for token budgets, oversized/empty candidate text,
   Unicode, full-board completeness, non-finite logits, model refusal and bundle/card
   mismatch.
10. Use metamorphic statistical tests only where the relationship is normative—for
    example batch/candidate order independence and canonical serialization equality.
    Do not encode an unproven assumption that every paraphrase must retain rank one.

### Evaluation funnel

- intended move representable;
- intended concrete move on board;
- top-1/top-3;
- correct disposition;
- clarification success and turn cost;
- argument accuracy;
- accepted without correction;
- graph-delta correctness;
- compiler admission;
- correct feedback for wrong/incomplete/stale attempts;
- recovery within one or more turns;
- repeat-failure and abandonment rate;
- eventual target completion and reversals.

### Gate 6

The gate has two explicitly separate lanes.

**Structural-infrastructure lane — permits Phase 7:**

- The complete game-turn capture, adjudication, split, funnel, structured baseline,
  calibration and identical-board comparison mechanisms are implemented and verified
  through focused unit, integration, property and bounded fuzz checks.
- Prompt and local models remain evidence producers with identical move-board
  constraints.
- Rejected, undone and corrected attempts are excluded from positive labels unless
  separately adjudicated; their outcome remains available as negative/correction data.
- Model-boundary fuzzing cannot panic, omit a legal move, emit a non-finite score or
  exceed declared token/memory/time limits without a typed bounded refusal.
- Any unavailable real-turn measurement is recorded as `not measured`; it is never
  estimated from synthetic data or represented as a passing product metric.

**Promotion-evidence lane — mandatory before learned-policy promotion or release:**

- At least 100 adjudicated real turns before any learned-policy promotion decision.
- Confidence intervals and per-risk-class results published.
- Model improvement is incremental over deterministic fusion, not compared only with
  random or lexical baselines.
- Feedback correctness and recovery rate are promotion metrics, not anecdotal UI facts.
- Synthetic data cannot authorize promotion.

The promotion-evidence lane remains pending after a structural green receipt. It does
not block Phase 7, but it does block learned-policy promotion and release.

## 13. Phase 7 — Converge APIs and user surfaces

### Purpose

Make the gameboard contracts the only design path while retaining an explicit legacy
compatibility window.

### Entry condition

Phase 6 structural-infrastructure lane is green. Phase 6 promotion evidence may remain
pending; no Phase 7 work may treat it as collected, measured or promotional authority.

### Work

1. Add or version endpoints for:
   - current `DesignPosition`;
   - board/piece/rule description;
   - legal palette;
   - move applicability explanation;
   - attempt evaluation and feedback options;
   - utterance evidence/disposition;
   - clarification answer;
   - move argument answer;
   - delta preview;
   - ratify/reject/undo;
   - correct/replace with an explicit prior-attempt link;
   - audit/history projection.
2. Make the graph UI and utterance endpoint consume the same position response and
   move IDs.
3. Route explicit palette selections through the same workbook, preview and admission
   path as utterances.
4. Route direct BPMN/DSL graph manipulations through the semantic-IDE equivalence
   resolver. A supported proven edit must be the same typed move as palette/language;
   a lower-level audit is allowed only for a typed no-counterpart/non-equivalence result.
5. Deprecate legacy thin-board serving and K-subset v3 helpers.
6. Remove duplicate candidate/disposition DTOs after compatibility tests pass.
7. Keep legacy sessions isolated and clearly identified until their rollback window
   closes.
8. Provide Sage a policy-filtered typed tool/API surface for board, move, rule, attempt
   and feedback retrieval. Do not expose internal Rust errors as its semantic contract.
9. Replay the same reference-model operation tapes through the in-process API/session
   adapter, injecting lost responses, restart, stale clients, duplicate requests and
   concurrent revision attempts.
10. Keep database/transport fuzzing outside the pure kernel target while comparing its
    durable results with the same abstract model after every recovery cut-point.
11. Audit `xtask`, examples, benches, integration tests and every fuzz crate: imports
    must resolve through designated facades or dedicated non-release test support.
    Remove any production `pub` justified only by those consumers.
12. Add dependency/public-API checks to production CI and path triggers for affected
    crate roots, features, fuzz manifests and `xtask` dependencies.

### Gate 7

- One move ID follows palette, language and direct-edit paths through the same compiler
  result.
- Every supported direct-edit tape either resolves to that same semantic move or emits
  a typed attributable non-equivalence reason; no unclassified raw-edit fallback exists.
- No live v3 bundle can enter legacy `score`, `score_list` or `score_serving` text.
- API compatibility and restart/recovery tests pass.
- Sage-facing responses cite snapshot, rule and attempt identities and reproduce after
  restart.
- A refused attempt changes the session turn but not the graph revision.
- Removed paths have no call sites under all supported feature combinations.
- Fault/schedule fuzzing preserves exactly-once graph revision semantics, idempotent
  receipts and replayable feedback history across restart and ambiguous responses.
- Tooling/test consumers have no privileged production import path, and `xtask`
  contains orchestration rather than duplicated game/pack/compiler semantics.

## 14. Phase 8 — Property, fuzz, differential and performance qualification

This phase composes and qualifies fuzz targets delivered in Phases 0–7. It must not be
used to defer a missing generator, reference model or invariant from the phase that
introduced the relevant state or behaviour.

### Property tests

- legal move set is deterministic and canonically ordered;
- move-set hash changes with graph/focus/pack/policy drift;
- every offered fully bound move previews and compiles;
- previewed delta equals ratified delta;
- no off-board evidence or clarification survives validation;
- evidence fusion is invariant to producer execution order;
- probability and score values are finite;
- stale proposals never apply;
- every attempt reaches exactly one typed outcome;
- non-transition outcomes preserve graph state;
- correction links are acyclic and resolve to an earlier attempt;
- feedback options resolve to legal moves or governed context/focus actions;
- disclosure filtering never leaks a hidden candidate through explanation text;
- history/belief cannot change legality;
- removing statistical producers leaves the palette operational.
- production and reference-model outcomes agree after every operation in a generated
  tape, not only at final state.

### Fuzz targets

- `design_position_decode`;
- `legal_move_enumeration` over generated valid/hostile graphs;
- `move_preview_compile`;
- `evidence_fusion` with missing, duplicate and hostile lanes;
- `belief_update` with hostile histories;
- `clarification_policy` over arbitrary candidate sets;
- `workbook_move_state_machine`;
- `move_attempt_feedback` over every outcome and hostile explanation references;
- `correction_history` over rejection, undo, replacement and repeated failure;
- `rule_explanation_decode` with hostile parameters and disclosure classes;
- `game_turn_replay`.

### Differential tests

- evaluator versus live-serving packet equality;
- preview versus actual apply;
- native versus Wasm compilation/admission where supported — **v0.10: N/A, no `wasm32`
  target exists in this product; see v0.10 amendment**;
- Python versus Candle learned-lane parity — **v0.10: N/A, no `pyo3` binding exists
  between the Python training scripts and the Rust runtime; see v0.10 amendment**.

### Fuzz governance and durable lanes

- Discover targets from the workspace; generate an independently timed CI matrix and
  fail aggregation if any target lacks a completed receipt.
- Commit minimized historical inputs under the consumed regression tree with a
  hash-governed manifest. Zero total regression cases is a gate failure after the first
  finding.
- Use controllable clocks and permanently remove simulated crashed actors from future
  tape operations.
- Replay minimized session/revision tapes against real PostgreSQL with two identities,
  connection loss before/after commit and process restart.
- Convert portable state/move/history packets into native/Wasm differential inputs.
- Persist corpora and crash artifacts for every fuzz project, including compiler and
  server boundaries.
- Run locked dependency resolutions and fail if fuzz lockfiles drift.
- Schedule corpus minimization/merge and report semantic coverage, valid-input/admission
  rates and coverage trends.
- Assert explicit limits for decode allocations, graph/move amplification, history and
  feedback depth, transition time, Wasm fuel and linear memory.
- Run public-API and dependency-direction checks under every fuzz/test/tooling feature
  combination; a harness cannot change the production facade it is meant to test.

### Performance budgets

Measure by graph size and move-board size:

- legal move enumeration;
- preview compilation;
- deterministic feature calculation;
- belief update;
- learned-lane scoring;
- full disposition latency;
- rule/feedback retrieval latency;
- serialized state/evidence size.

### Gate 8

- Every new fuzz target is discovered, independently sharded and receipted.
- No regression directory is empty after a finding is committed.
- P95 interactive latency meets the ratified budget on representative hardware.
- Resource-limit failures are typed and leave the session usable.
- Expected wrong-move traffic cannot cause unbounded history, feedback recursion or
  repeated compiler work.
- Every target has a completed receipt; semantic coverage includes every move kind,
  attempt outcome, disposition, disclosure class and correction lifecycle or records a
  reviewed unreachable justification.
- PostgreSQL fault tapes, native/Wasm differential packets and resource-abuse corpora
  pass their separately receipted lanes.
- Corpus minimization and regression-manifest validation run in CI without silently
  rewriting committed artifacts.
- Public-API snapshots and compile-fail boundary tests are unchanged except for
  separately reviewed facade/contract additions.

## 15. Phase 9 — Shadow rollout, promotion and cleanup

### Rollout

1. `observe`: build and record game positions without changing responses.
2. `shadow`: calculate game dispositions and compare with the current route.
3. `palette`: expose the legal move palette and previews.
4. `feedback`: expose governed explanations and recovery options for unsuccessful
   attempts while proposals remain shadowed.
5. `suggest`: expose a single proposal or clarification under ratified thresholds.
6. `workbook`: enable the complete argument/preview/ratification flow.

There is no `auto_apply` stage.

### User-population gating

The six stages above gate *capability surface*. A second, orthogonal axis gates
*utterance style / user population*, per the v0.7 amendment:

1. `power_user_dictation`: the deterministic power-user tier — session DSL dictated as
   pack-level runbooks (template invocation) or literal atomic verbs, resolved through
   the exact/lexical evidence lane without requiring Candle/SLM interpretation. This is
   the REPL baseline and may progress through the capability-surface stages above on its
   own schedule.
2. `generic_utterance`: non-power-user, underspecified or ambiguous utterances requiring
   Candle/SLM-assisted evidence and motif inference. Exposure to this population at any
   capability-surface stage requires the power-user tier to already be live and stable
   at that same or a later stage — a generic-utterance user is never the first to reach
   a given capability surface.

This does not change Phase 0-6 engineering, which builds the complete deterministic and
statistical evidence stack regardless of which population is currently exposed to it.

### Promotion controls

- real adjudicated evidence only;
- board recall and compiler-safety gates absolute;
- confident-wrong rates split by mutating risk;
- wrong-attempt feedback correctness, recovery and repeated-failure rates;
- top-three/clarification success reported alongside top-one;
- rollback is a configuration change that does not invalidate stored graph revisions;
- old and new decision records remain distinguishable and replayable.
- all affected fuzz targets, regression replays and semantic coverage floors are green
  for the exact promoted revisions and packs.
- capability public-API/dependency receipts are green and test/tooling feature builds
  expose no additional production surface.

### Cleanup

After the rollback window:

- remove thin-board production construction;
- remove exclusive lane-priority serving;
- remove legacy v2 textualisation APIs from v3-capable types;
- remove duplicate BPMN-local evidence contracts superseded by the shared version;
- archive obsolete training claims and mark invalid measurements explicitly;
- update the ratified V&S and operator/deployment documentation.

### Gate 9

- Ratified real-session thresholds pass for the intended rollout surface.
- No `generic_utterance` population is exposed to a capability-surface stage the
  `power_user_dictation` population has not already reached and stabilized at.
- Rollback rehearsal passes.
- No legacy production call path remains unintentionally reachable.
- Documentation, API examples, receipts and deployment configuration describe the
  delivered gameboard architecture.

## 16. Permanent test matrix

| Boundary | Required permanent evidence |
|---|---|
| Shared contracts | golden bytes, canonical hashes, hostile decode, compatibility |
| Pack | schema/admission, references, motif/cue coverage, lock drift |
| Graph position | focus, revision, move enumeration, canonical reconstruction |
| Reference model | per-operation production/model agreement, shrinkable tapes |
| Compiler | preview/apply equivalence, full admission, typed refusals |
| Attempts | every outcome, graph/session state separation, correction links |
| Rules/feedback | governed explanations, useful legal options, disclosure filtering |
| Evidence | full-board coverage, lane order independence, finite scores |
| Belief | bounded update, rejection/undo behavior, no authority leakage |
| Disposition | top-3 clarification, NOTA, missing args, stale state |
| Workbook | atomic answers, preview identity, ratify/reject/restart |
| Serving | evaluator parity, rollout isolation, palette/language equivalence |
| Models | bundle admission, serializer parity, Python/Candle parity |
| Sage API | typed board/piece/rule/attempt retrieval, provenance, restart replay |
| Product | real-turn funnel, feedback recovery, corrections, reversals, turns-to-valid-target |
| Fuzz governance | target receipts, regression manifests, cmin, semantic coverage trend |
| Capability boundary | API snapshot, `unreachable_pub`, dependency direction, compile-fail internals |
| Tests/tooling | facade-only imports, feature-surface equality, no `xtask` domain logic |

## 17. Source-level change ledger

### Add in BPMN-Lite

- one named gameboard capability facade module with explicit re-exports;
- `utterance-engine/src/game_state.rs`
- `utterance-engine/src/legal_moves.rs`
- `utterance-engine/src/graph_features.rs`
- `utterance-engine/src/argument_evidence.rs`
- `utterance-engine/src/fusion.rs`
- `utterance-engine/src/history.rs`
- `utterance-engine/src/motifs.rs`
- `utterance-engine/src/belief.rs`
- `utterance-engine/src/clarification.rs`
- `utterance-engine/src/attempt.rs`
- `utterance-engine/src/feedback.rs`
- `utterance-engine/src/rule_explanation.rs`
- gameboard fuzz support/reference model isolated under the existing fuzz/test-support
  conventions
- game-level evaluation and fuzz targets following existing project conventions

### Extend coherently

- `utterance-engine/src/bpmn_board.rs`
- `utterance-engine/src/bpmn_pack.rs`
- `utterance-engine/src/board.rs`
- `utterance-engine/src/context.rs`
- `utterance-engine/src/contract.rs`
- `utterance-engine/src/exact.rs`
- `utterance-engine/src/policy.rs`
- `utterance-engine/src/disposition.rs`
- `utterance-engine/src/trained_ranker.rs`
- `bpmn-lite-server-designer/src/proposal.rs`
- `bpmn-lite-server-designer/src/rest.rs`
- `bpmn-lite-compiler/src/dsl/refactor.rs`
- semantic pack YAML and its compiled lock

### Shared workspace release

- `/Users/adamtc007/dev/dsl/crates/semantic-decision-contracts`
- semantic-pack schema/compiler only where required for generic motif/evidence config

### Preserve

- production compiler and verifier authority;
- `PositionalLegality` as the semantic legality input until replaced by a proven single
  move-enumeration oracle;
- `AstMutator` graph-edit semantics;
- proposal workbook atomicity and provenance;
- typed attempt and feedback history once introduced;
- graph revision checks and ratification;
- shadow/suggest/workbook safety posture;
- runtime, lease, persistence and Wasm execution semantics;
- private/`pub(crate)` implementation visibility unless a reviewed facade contract
  requires otherwise.

### Delete only after cutover proof

- v3 access to legacy `TrainedRanker::score`/`score_list`/`score_serving` paths;
- production K-subset helpers for gameboard serving;
- exclusive producer-priority evidence selection;
- duplicate local evidence DTOs;
- thin-board graph-session code;
- top-two-score-gap-only ambiguity policy;
- public implementation modules, unchecked constructors and test/fuzz/`xtask` hooks
  superseded by the capability facade or dedicated non-release support.

## 18. Commit strategy

Recommended commit sequence:

1. `docs: ratify semantic gameboard vision and BPMN implementation plan`
2. `test: make v3 evaluator reproduce production serving packets`
3. `feat(contracts): add canonical design position and legal move contracts`
4. `refactor(api): establish sealed gameboard capability facade`
5. `feat(designer): enumerate explain and preview concrete legal graph moves`
6. `feat(resolver): record and fuse complete per-move evidence`
7. `feat(designer): add attempt feedback correction and rule retrieval`
8. `feat(designer): add history motifs and bounded belief state`
9. `feat(policy): add information-gaining game dispositions`
10. `feat(evaluation): add real-turn game funnel and structured baseline`
11. `refactor(api): converge palette language direct-edit and Sage rule paths`
12. `test: qualify gameboard properties fuzz parity and performance`
13. `feat(rollout): shadow and promote gameboard designer`
14. `refactor: retire superseded resolver paths after rollback window`

Each behavioural commit requires its own red/green receipt. Model weights, generated
corpora and source refactors are always separate commits. Each commit that introduces a
state field, move, outcome or transition must extend the relevant generator, reference
model, semantic counter and fuzz invariant in the same commit.

## 19. Stop conditions

Stop the active phase and report rather than improvising if:

- compiler and `PositionalLegality` disagree on an offered move;
- a concrete move cannot be given a stable identity;
- shared contracts would require BPMN/application vocabulary;
- preview and ratified application produce different graph deltas;
- the same inputs produce a different move-set hash;
- a statistical producer can change the legal move set;
- a clarification requires an off-board candidate;
- a normal wrong/incomplete attempt can only be represented as an unstructured error;
- Sage would need to infer a rule or recovery option not returned by a governed API;
- a refused attempt mutates authoritative domain state;
- an undo/correction would require rewriting historical events;
- captured wrong attempts would be treated as positive training labels without
  adjudication;
- the core transition path requires ambient wall time, random IDs, process globals,
  network access or a live database to fuzz;
- a new state/move/outcome cannot be generated, shrunk, canonically serialized or
  compared with the reference model;
- a fuzz target can skip because its regression corpus is empty or cannot emit a
  completion receipt;
- a test, fuzz target, example, bench or `xtask` requires an implementation item to
  become `pub` rather than using the facade or non-release test support;
- a capability crate depends on an application, fuzz project or `xtask`;
- a feature used by tooling changes the production public API or authority surface;
- public API growth has no named external consumer, stability contract and owner;
- implementing a phase requires runtime execution-semantics changes;
- pre-existing user work overlaps a required source file and cannot be safely preserved;
- an external model or service becomes necessary for the palette or compiler path.

## 20. Definition of done

The refactor is complete only when:

1. the session exposes a canonical, replayable `DesignPosition`;
2. the compiler-governed move set is the shared basis of every design surface;
3. graph structure, arguments, history, pack semantics and optional models are recorded
   as per-move evidence;
4. the resolver can operate usefully without a learned model;
5. ambiguity is handled through governed, information-gaining clarification;
6. multi-step intentions are retained as non-authoritative motif hypotheses;
7. every attempt has a typed, retrievable outcome and governed feedback path;
8. illegal attempts advance only session/learning state, while legal-but-unwanted moves
   are corrected through linked governed moves;
9. Sage can retrieve board, pieces, rules, explanations and recovery options without
   parsing technical errors or inventing semantics;
10. every accepted move is fully bound, previewed, ratified and production-compiled;
11. real-session evidence meets ratified product, feedback and safety thresholds;
12. shared contracts can support later bounded `ob-poc` subdomain boards;
13. every gameboard contract and transition is covered by model/state-machine fuzzing,
    semantic coverage receipts and governed minimized regressions;
14. durable fault tapes and native/Wasm differential packets qualify the applicable
    boundaries;
15. only designated capability facades and stable contracts are public; implementation
    defaults to `pub(crate)` or narrower;
16. tests, fuzzers, examples, benches and `xtask` use those facades without privileged
    production hooks or visibility expansion;
17. public-API snapshots, compile-fail boundary tests and dependency-direction gates are
    permanent CI evidence;
18. legacy resolver paths are either explicitly isolated or removed;
19. runtime/compiler authority and configuration ownership remain intact.

## 21. Immediate next action after ratification

Execute Phase 0 only. Do not begin another retrain or introduce graph-model machinery.
The first delivery receipt must prove that the evaluator and production generate the
same semantic board, candidate pairs, evidence finalisation and disposition for fixed
turn packets. It must also freeze the discovered fuzz-target/receipt/regression baseline
and add the v3-route refusal target, plus capture the affected crates' public-API and
dependency-direction baseline—including tests, fuzz projects and `xtask`. That
establishes the trustworthy measurement, fuzz and capability-boundary baseline on
which the rest of this programme depends.
