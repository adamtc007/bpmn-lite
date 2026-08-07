# EOP-VS-BPMN-GAMEBOARD-001 — Semantic Gameboard architecture, proven through BPMN

**Version:** v0.5
**Status:** DRAFT FOR RATIFICATION
**Date:** 2026-08-07
**Owner:** Adam
**Repository:** `/Users/adamtc007/dev/bpmn-lite`
**Baseline:** `feat/dir-002-phase-c-slm-training` at `22ba055`
**Companion delivery plan:** `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md`

## Changelog

**v0.4 → v0.5 — owner-authorized Phase 6 gate split.** Phase 6 now has a
structural-infrastructure gate and a separate promotion-evidence lane. Passing the
structural gate permits the API/user-surface refactor to continue when the capture
programme is not yet available. It does not authorize a learned-policy promotion,
release claim, or substitution of synthetic fixtures for real design-session evidence.
The real-turn threshold, confidence intervals, per-risk results and feedback/recovery
metrics remain mandatory before any such promotion or release.

**v0.3 → v0.4 — capability facade and visibility discipline.** Public Rust scope is
now treated as an architectural boundary. Each capability exposes one small, deliberate
facade and the minimum stable contract types required by consumers; implementation
modules default to `pub(crate)` or narrower. Applications compose capability facades.
Tests, fuzz projects, examples, benches, generators and `xtask` receive no privileged
route through internals and may not cause production visibility to widen.

**v0.2 → v0.3 — fuzzable by construction.** Fuzzability is now a design law, not a
late test activity. Every gameboard must expose a pure deterministic transition kernel,
canonical state/move/outcome contracts, controllable sources of time/identity/randomness
and a compact reference model suitable for generated operation tapes and shrinking.
Fuzzing begins with each contract/phase; final qualification composes the already-fuzzed
layers. Semantic coverage, corpus governance and permanent minimized regressions are
release evidence.

**v0.1 → v0.2 — human iteration and cross-domain scope.** The gameboard is now a
general DSL/SemOS/Sage/Repl architecture with BPMN-Lite as its first proof domain and
`ob-poc` as the intended federation of later subdomain boards. Wrong, incomplete,
stale, rejected and later-corrected moves are first-class expected session outcomes.
Rules, move pieces, applicability explanations, feedback and recovery options become
typed, pack-governed resources retrievable by Sage; they are not left as technical
Rust/compiler errors. Authoritative domain state remains separate from the session's
attempt/learning state.

## 1. Executive decision

DSL/SemOS applications are to be treated as **compiler- or rule-governed, iterative
semantic games**. Each bounded domain exposes a board, pieces, rules, legal moves,
observable outcomes and governed feedback. Sage and the human collaborate over that
board; Repl adjudicates and applies moves. BPMN-Lite Designer is the first proof domain,
where the constructed result is a valid executable workflow.

At every turn in the BPMN proof:

1. the current workflow graph is the board;
2. the admitted semantic pack and compiler determine the legal moves;
3. the user has a partially hidden target design in mind;
4. an utterance, selection, form answer, or direct manipulation provides evidence about
   the next intended move;
5. deterministic policy either proposes a legal graph transformation, asks the most
   useful clarification, abstains, or exposes the legal palette;
6. the user ratifies a previewed transformation;
7. the production compiler admits the resulting graph before it becomes the next
   authoritative board state.
8. a wrong, incomplete, stale or rejected attempt returns a governed feedback receipt
   and recovery options, becoming part of session history without being disguised as
   an infrastructure failure.

The statistical task is therefore not:

> classify an utterance into one of approximately 27 verbs.

It is:

> rank and complete the legal graph transformations available from this exact design
> position, using the utterance, graph structure, design history, typed arguments and
> governed pack semantics as evidence about the user's partially observed target.

This document resets the resolver framing and establishes the reusable architecture
for DSL, SemOS, Sage and Repl. It preserves the existing deterministic authority
boundaries, compiler, BPMN execution model, semantic-pack ownership, proposal workbook
and ratification controls.

## 2. Why the game metaphor is technically useful

The metaphor is not decorative. It identifies the correct computational structure.

| Design-game concept | BPMN-Lite meaning |
|---|---|
| Board position | Current canonical Designer graph plus focus and revision |
| Rules | BPMN-Lite profile, semantic pack, policy and production compiler |
| Legal move | A typed graph transformation that can be previewed and admitted |
| Move history | Prior graph deltas, utterances, answers, refusals and reversals |
| Player's plan | The user's unobserved intended workflow or design motif |
| Visible puzzle edges | Structurally incomplete or recently edited graph regions |
| Move suggestion | Non-authoritative ranked evidence over legal transformations |
| Question | An information-gaining choice between materially different moves |
| Move acceptance | Explicit ratification of a previewed graph delta |
| Wrong or incomplete attempt | Expected session outcome with governed feedback |
| Illegal move | Refused transition plus rule-grounded legal alternatives |
| Legal but unwanted move | Applied revision followed by an explicit correction or undo |
| Feedback | A typed response grounded in the same rule and piece definitions |
| End condition | A saved, compiler-admitted and optionally published template |

Unlike chess or Go, this is not normally adversarial and there is no opponent. The
closest formal descriptions are:

- sequential decision-making under partial observation;
- constrained structured prediction;
- active disambiguation over a compiler-generated action space;
- incremental programme synthesis with a human in the loop.

This distinction matters. The first implementation should not introduce reinforcement
learning, self-play or a general game-search engine. A calibrated structured choice
model over legal moves is simpler, more inspectable and better matched to available
data.

A wrong move is not an exceptional edge case. Humans form understanding through
attempt, feedback, correction and repetition. The architecture must therefore optimize
not only for first-attempt accuracy, but for safe and comprehensible recovery. It must
not assume that a user's terminology, choices or goals remain perfectly consistent
across turns.

## 3. Product vision

A user can begin with an empty workflow or an admitted existing template. The Designer
always shows the current compilable position and the legal ways it can change. The user
may work through natural language, direct graph manipulation, a governed action palette,
typed questions, or any mixture of those surfaces. All surfaces resolve to the same
typed move contracts and the same compiler path.

Natural language is a high-bandwidth accelerator over the correct-by-construction
Designer. It is not the only way to operate the product and is never an authority
boundary.

The successful experience is:

- the system understands where the user is working;
- it narrows the next moves using graph structure before interpreting language;
- it recognizes common multi-step workflow motifs without pretending they are atomic;
- it asks a small, discriminating question when intent remains ambiguous;
- it previews exactly what will change;
- it treats a mistaken attempt as a normal turn and explains what can be done next;
- Sage can retrieve the exact board, piece and rule facts behind that explanation;
- it never silently invents, expands or applies a move;
- every accepted turn leaves a valid, attributable design state.

### 3.1 Reusable lifecycle

Every conforming domain supplies the same abstract lifecycle:

```text
authoritative state
    -> governed board construction
    -> observation or attempted move
    -> evidence and rule evaluation
    -> proposal | clarification | feedback | abstention
    -> validation and ratification where mutation is requested
    -> transition receipt or non-transition attempt receipt
    -> next session position
```

The reusable pieces are generic contracts and mechanisms. Domain meaning remains in
admitted configuration.

### 3.2 BPMN proof domain

For BPMN-Lite, the board is the Designer graph, the pieces are typed graph operations,
and the production compiler/verifier is the final rules authority. The target is an
admitted executable template.

### 3.3 `ob-poc` federation

`ob-poc` is not one giant gameboard. It is a federation of bounded subdomain boards,
each intended to be roughly BPMN-board scale after state, subject, role and policy
collapse. A subdomain declares its own:

- state projection and identities;
- pieces/action schemas;
- applicability and transition rules;
- outcome and feedback vocabulary;
- motifs and completion conditions;
- bridges to other subdomain boards.

Changing subject, subdomain or board is itself an explicit governed context move. A
cross-domain route is a sequence of board transitions with provenance, not a silent
union of every verb in the estate.

## 4. Normative state model

The word `state` must not refer to one undifferentiated prompt string. A design session
has distinct authoritative, derived, historical, statistical and learning-state
projections with different authority.

### 4.1 Authoritative board state

The authoritative state at turn `t` is:

\[
G_t = \text{the canonical current Designer graph}
\]

It includes the workflow topology and all authoring data needed to compile it. It is
bound to:

- graph revision;
- canonical graph hash;
- semantic-pack snapshot identity;
- compiler/profile identity;
- policy identity;
- current proposal, when one exists.

Only a ratified transformation that survives production compilation can produce
`G_(t+1)`.

### 4.2 Deterministically derived position

The system derives a `DesignPosition` from the authoritative state:

- current focus or selected subgraph;
- legal move set;
- required and optional typed arguments for each move;
- previewable graph delta for sufficiently bound moves;
- local structural facts and incomplete motifs;
- compiler diagnostics and applicability explanations.

The legal move set is a pure, canonical function of its recorded inputs:

\[
A_t = LegalMoves(G_t, focus_t, pack_t, policy_t, compiler_t)
\]

Reconstruction with the same inputs must produce the same ordered moves and move-set
hash.

### 4.3 Observed design history

`H_t` records how the position was reached:

- utterances and explicit UI actions;
- selected anchors and focus changes;
- proposed, clarified, rejected, ratified and undone moves;
- slot answers and their provenance;
- compiler refusals;
- graph deltas and revision changes;
- user corrections and terminology preferences.

History is evidence and audit truth. It does not weaken current legality checks.

### 4.4 Statistical belief state

The system may maintain a non-authoritative belief `B_t` over:

- likely next legal moves;
- likely typed argument values;
- likely target motifs;
- unresolved alternatives;
- whether the current board lacks the intended operation;
- whether a clarification is more valuable than a proposal.

The system does **not** need to enumerate every possible completed workflow. It should
represent only decision-relevant hypotheses, such as `timeout-and-escalate`,
`parallel-approval`, or `repeat-for-each-item`, and the legal next steps that would
advance them.

Belief is recorded as evidence with producer and policy identities. It is never
replayed as authority and never changes the graph directly.

### 4.5 Attempt, feedback and learning state

The session also records `L_t`, the human/system learning state created by attempts and
feedback. This is first-class session state but is not automatically authoritative
domain state.

The architecture distinguishes at least these outcomes:

| Attempt outcome | Domain state | Session/learning state | Required response |
|---|---|---|---|
| Legal and intended | Advances after ratification | Records success and evidence | Transition receipt |
| Incomplete | Unchanged | Records unresolved arguments/focus | Typed completion options |
| Ambiguous | Unchanged | Records live alternatives | Discriminating clarification |
| Inapplicable/illegal | Unchanged | Records attempted intent and applicable rules | Explanation plus legal recovery moves |
| Policy-hidden/forbidden | Unchanged | Records disclosure-safe refusal | Generic governed route forward |
| Stale | Unchanged | Records superseded position | Rebuild board and invite retry |
| Compiler refused | Unchanged | Records typed diagnostic | Rule-grounded repair options where known |
| Legal but later judged wrong | Already advanced | Records correction link | Governed undo, replace or follow-up move |
| System/infrastructure failure | Unknown or unchanged, recovered explicitly | Records incident | Technical recovery; never blame the user |

An illegal attempt does not produce a new workflow/business state, but it **does**
produce a new session turn. A legal move later judged wrong is not erased from history:
the correction is another attributable transition linked to the original move.

Wrong attempts must not be silently used as positive training labels. They become
learning evidence only through their outcome, later correction and, where required,
human adjudication.

Calling these outcomes expected does not make them legal, successful or rewarded. It
means the product models them deliberately, preserves authority correctly and provides
a useful route forward.

## 5. The unit of inference is a typed move

A bare canonical verb is insufficient. The unit placed on the board is a typed,
position-bound `LegalMove`, conceptually:

```rust
pub struct LegalMove {
    move_id: LegalMoveId,
    candidate_id: CanonicalCandidateId,
    graph_revision: GraphRevision,
    anchor: Option<GraphElementRef>,
    arguments: Vec<MoveArgument>,
    binding_state: MoveBindingState,
    applicability: ApplicabilityEvidence,
    preview: Option<GraphDeltaPreview>,
    semantic_hash: String,
}
```

One semantic candidate can yield several legal moves at a position—for example, attach
a timer to task A or task B. Conversely, a move may remain partially bound until the
user supplies a duration or chooses an anchor.

Move identity must be stable for the recorded position and must change when any
authority-bearing input changes.

### 5.1 Attempts and feedback are typed resources

Every attempted move or instruction produces a `MoveAttemptReceipt`, whether or not a
domain transition occurs. Conceptually:

```rust
pub struct MoveAttemptReceipt {
    attempt_id: MoveAttemptId,
    position_id: DesignStateId,
    attempted_move: Option<LegalMoveId>,
    observed_intent_hash: String,
    outcome: MoveAttemptOutcome,
    rule_explanations: Vec<RuleExplanationRef>,
    feedback_options: Vec<FeedbackOption>,
    correction_of: Option<MoveAttemptId>,
    receipt_hash: String,
}
```

The type is public because it is a cross-crate contract; its representation need not
be. Fields should remain private with invariant-preserving constructors and read-only
accessors unless an explicitly versioned wire requirement justifies otherwise.

`RuleExplanation` and `FeedbackOption` are public semantic resources, not formatted
compiler strings. They carry stable codes, governed user-facing text or message keys,
provenance, disclosure classification and links to currently legal moves. This permits
the UI, Sage and audit tooling to render the same truthful response at different levels
of detail.

Compiler and Repl diagnostics remain authoritative technical evidence. The adapter
maps known diagnostics to governed explanations and recovery options; an unmapped
diagnostic remains an honest typed technical failure rather than being improvised by
Sage.

## 6. Evidence model

Every legal move receives a typed evidence vector. Evidence lanes are combined by a
versioned deterministic policy rather than selected by an exclusive fallback chain.

Required lanes are:

1. **Governed exact evidence** — exact pack phrases and explicit identifiers.
2. **Deterministic grammar evidence** — governed action cues, negation, conjunction
   and construction patterns.
3. **Typed argument evidence** — durations, counts, names, data references, node
   references and other schema-compatible values.
4. **Graph-local evidence** — compatibility with the focus, selected node and adjacent
   topology.
5. **Structural-completion evidence** — whether the move completes or advances a
   governed workflow motif.
6. **History evidence** — continuity with recent edits, clarifications, corrections and
   established terminology.
7. **Lexical/embedding evidence** — semantic similarity derived from governed candidate
   text.
8. **Learned ranker evidence** — a candidate-conditioned model score.
9. **Abstention evidence** — evidence that no legal move represents the request.
10. **Correction evidence** — rejection, undo and explicit replacement links from
    earlier attempts.

The candidate-conditioned model is one lane. It does not replace the other lanes and
does not issue dispositions.

An initial score can be expressed as a calibrated log-linear choice model:

\[
score(m_i) = w^T \phi(u_t, G_t, H_t, m_i)
\]

\[
P(m_i) = \frac{e^{score(m_i)}}{\sum_{m_j \in A_t} e^{score(m_j)}}
\]

The feature schema, weights, calibration and policy identity must be versioned and
recorded. Exact rules and safety constraints may gate or dominate learned evidence.

## 7. Declarative ownership

The standing shared-crate boundary remains mandatory:

- Rust owns generic mechanisms, typed contracts, canonical encoding, validation,
  feature calculation machinery and public APIs.
- YAML packs own domain/application semantics: verbs, phrases, arguments,
  applicability, contrasts, motifs, structural cues, rule explanations, feedback
  options and clarification wording.
- BPMN-Lite owns the BPMN adapter from the generic contracts to Designer graph edits
  and compiler admission.
- The host owns session persistence, UI policy, identity, access and deployment.

No BPMN operation vocabulary, motif catalogue or application policy is to be hidden in
a learned bundle or hard-coded as host-specific Rust branching. A model artifact may
learn weights from evidence, but every feature meaning and candidate remains traceable
to governed inputs.

### 7.1 Rules must be retrievable, not merely executable

SemOS and DSL pack APIs must allow an application or Sage to ask, subject to policy:

- what board is active and why;
- which pieces/actions exist on this board;
- which moves are currently legal;
- why a named move is legal, incomplete, inapplicable or unavailable;
- which rule facts caused that result;
- which arguments or state changes would make it applicable;
- what legal recovery or neighbouring moves are available;
- which governed motif a move advances;
- what changed after the last attempt.

Responses are typed, content-addressed and provenance-bearing. Sage may translate or
explain them, but must ground the answer in returned rule/explanation identifiers. It
must not reverse-engineer Rust errors, invent rules or expose policy-hidden actions.

The public capability boundary therefore includes board discovery, move enumeration,
attempt evaluation, rule explanation, feedback retrieval, preview and transition—not
only a boolean legality gate or compiler error.

### 7.2 Capability and crate boundary

The re-engineering must create a strong Rust capability boundary, not spread public
scope across implementation modules.

The visibility default is:

- private item where one module owns it;
- `pub(super)` where only the parent module coordinates it;
- `pub(crate)` for implementation shared inside one crate;
- `pub` only for an item deliberately included in a designated capability facade or
  stable cross-crate contract.

The application is the composition root. It depends on capability facades; it does not
reach into their internal modules. A library necessarily needs some `pub` items for an
application or another capability crate to call, but those items are an audited API,
not permission for implementation-wide public visibility.

The target shape is:

```text
application / host
    -> gameboard capability facade
        -> domain adapter facade
            -> private/pub(crate) implementation modules
        -> shared stable contract crate
        -> compiler/SemOS/DSL capability facades
```

Rules:

1. Implementation modules are private by default; no public module tree is exposed for
   consumer convenience.
2. Facades re-export named stable items explicitly; glob re-exports are prohibited.
3. A type is public only when it crosses a real crate/application boundary or belongs
   to a versioned persistence/wire contract.
4. Internal structs must not be made public merely to support integration tests,
   examples, benchmarks, fuzzing, corpus generation or `xtask`.
5. External trait implementation is allowed only when it is part of the capability
   design; otherwise traits are sealed or constructors constrain implementation.
6. Applications translate facade contracts into host DTOs. Capability crates do not
   depend on application/server crates.
7. Test-support builders, when unavoidable, live behind `cfg(test)` or a dedicated
   non-production test-support package. They cannot add authority, bypass admission or
   enter release artifacts.
8. Fuzz targets use public admission/facade APIs for production behaviour. They may use
   local byte-tape generators and reference models, but no fuzz-only production entry
   point.
9. `xtask` is orchestration only. It may invoke public commands/facades, inspect
   receipts and manage artifacts; it must not become a back door into internal
   implementation modules.
10. Examples and benches demonstrate and measure the supported facade. If an example
    cannot be written without exposing internals, the facade or the example is wrong.

The public API is maintained as a reviewed artifact. CI inventories exported items,
fails unapproved growth, enforces dependency direction and checks that test/fuzz/tooling
features do not change the production public surface.

## 8. Interaction policy

The Designer supports four equivalent input modes over one legal move set:

- direct graph manipulation;
- legal-move palette;
- natural-language instruction;
- typed clarification/workbook answers.

The disposition policy chooses among:

- propose one sufficiently supported move;
- present two or three materially distinct legal moves;
- ask one typed, information-gaining clarification;
- request missing arguments for an otherwise established move;
- abstain because the intended move is not on the board;
- propose a governed context or focus change;
- explain an unsuccessful attempt and offer legal recovery moves;
- offer correction or undo for a legal move the user now rejects;
- escalate for collaborative analysis without silently expanding the board.

Clarification is selected by expected information gain, constrained by governed
contrasts and argument schemas. A top-two score gap alone is not a semantic reason to
ask a question.

Every proposal displays:

- the affected graph elements;
- a human-readable description;
- required arguments still missing;
- the previewed graph delta;
- relevant compiler diagnostics;
- the evidence/disposition reason at an appropriate user-facing level.

Every unsuccessful attempt displays, subject to disclosure policy:

- what the system understood the attempted move to be;
- whether it was incomplete, ambiguous, inapplicable, stale or compiler-refused;
- the relevant governed rule explanation;
- one or more valid next options, including changing focus or context;
- a simple way to correct the interpretation and try again.

## 9. Multi-step intentions and motifs

Users frequently describe a motif that requires several atomic graph moves. The system
may recognize that motif and retain it as a session hypothesis, but it must decompose
it into individually previewed and ratified legal moves unless a governed macro has
explicit atomic semantics.

Example:

```text
Utterance: "If approval takes more than two days, escalate it and stop waiting."

Belief:
  motif: timeout-and-escalate
  focus: approval_task
  likely steps:
    1. attach interrupting timer to approval_task
    2. bind duration P2D
    3. append escalation task on timer path
    4. connect or configure escalation target
```

The motif is a plan hypothesis, not an executable artifact. The graph and compiler
remain the truth at every intermediate step.

## 10. Success measures

Raw utterance top-1 accuracy is necessary but not sufficient. The product is successful
when users reach their intended valid workflow efficiently and safely.

The permanent measurement funnel is:

1. intended move representable by the pack;
2. intended concrete move present on the legal board;
3. intended move in top three;
4. correct proposal, clarification or abstention disposition;
5. intended arguments correctly extracted or requested;
6. user accepts without correction;
7. graph delta matches the adjudicated intention;
8. resulting workflow compiles;
9. wrong/incomplete attempts receive correct and useful feedback;
10. design reaches its target with bounded turns and reversals.

Report at least:

- legal-move board recall;
- top-1 and top-3 move accuracy;
- calibration and confident-wrong rate by risk class;
- NOTA precision and recall;
- clarification coverage, success and extra-turn cost;
- argument completion accuracy;
- proposal rejection, correction and undo rates;
- wrong-attempt recovery rate and turns-to-recovery;
- feedback-option usefulness and rule-explanation correctness;
- repeated-failure rate for the same rule or missing argument;
- compiler-refusal rate after proposal;
- turns and elapsed time to an admitted target template;
- latency and memory by board-size bucket.

Promotion evidence comes from real, adjudicated design turns. Synthetic examples are
development aids and invariant fixtures, not promotion or release evidence. A green
structural Phase 6 gate may precede that evidence under the v0.5 amendment; it is not
a promotion decision.

## 11. Safety and authority invariants

1. **The graph is authority.** Statistical state cannot mutate it.
2. **The compiler owns legality.** No model output can create an off-board move.
3. **One pack snapshot.** Board, language evidence, motifs and binder schemas derive
   from the same admitted semantic snapshot.
4. **One move set across surfaces.** Palette, language, clarification and direct edit
   use the same typed move contracts.
5. **Ratification is mandatory.** No natural-language auto-apply path exists.
6. **Compile before commit.** A ratified delta is admitted before becoming the next
   board revision.
7. **Belief is evidence.** It is hash-bound, producer-attributed and replayed only as
   historical evidence.
8. **Unknown remains unknown.** Missing focus, context or arguments produce a question,
   not an invented default.
9. **History cannot authorize.** Prior acceptance does not bypass current policy or
   compiler checks.
10. **Configuration is semantic truth.** Domain meaning remains in admitted YAML,
    not application-specific Rust or opaque weights.
11. **Wrong moves are expected.** Every attempt reaches a typed outcome; an expected
    human mistake is not represented as an opaque system exception.
12. **State separation is explicit.** A refused attempt advances session history but
    cannot masquerade as an authoritative domain transition.
13. **Corrections preserve history.** Undo and replacement link to the move corrected;
    historical evidence is never rewritten.
14. **Feedback is governed.** Sage and UI responses resolve from typed rule and feedback
    resources, respecting disclosure policy.
15. **Public scope is capability scope.** Only designated facades and stable contracts
    are public across crates; implementation and test-support code remains crate-private
    or narrower.

## 12. Fuzzability as an architectural quality

The Semantic Gameboard must be **fuzzable by construction**. Its explicit board,
pieces, legal moves, outcomes and transition rules make it unusually well suited to
model-based and stateful fuzz testing. If the implementation cannot be exercised as a
small deterministic transition system without booting an application server or
database, the boundary is incorrectly drawn.

### 12.1 Pure game kernel

Every domain adapter must expose an equivalent of:

```rust
fn position(state: &AuthoritativeState, context: &GovernedContext)
    -> Result<DesignPosition, GameError>;

fn evaluate_attempt(position: &DesignPosition, attempt: &MoveAttempt)
    -> Result<MoveAttemptReceipt, GameError>;

fn preview(position: &DesignPosition, bound_move: &BoundMove)
    -> Result<TransitionPreview, GameError>;

fn step(state: &AuthoritativeState, ratified: &RatifiedMove)
    -> Result<TransitionOutcome, GameError>;
```

These functions may call deterministic compiler/rule mechanisms but must not depend on
ambient wall time, process-global identity, network state or an implicit database.
Clocks, identifiers, randomness and external observations are explicit inputs. Durable
adapters are tested separately against the same operation tapes.

### 12.2 Required fuzz oracles

Generated boards, packs, histories and attempt tapes must continuously assert:

- legal-move **soundness**: every fully bound offered move previews and admits;
- legal-move **completeness** against the compact reference model;
- refused/incomplete/stale attempts never mutate authoritative state;
- accepted moves change only the previewed fields and produce the predicted revision;
- canonical hashes reproduce and change for every authority-bearing difference;
- replay of the same tape produces the same states, receipts and hashes;
- correction/undo links are valid, acyclic and preserve history;
- feedback options resolve to legal moves or governed context/focus transitions;
- disclosure filtering cannot reveal a hidden piece through text, parameters or IDs;
- model/evidence output cannot add, delete or rename a legal move;
- every attempt reaches exactly one typed outcome;
- resource and amplification limits are respected;
- a panic, hang, non-finite score, silent skip or untyped failure is a finding.

### 12.3 Complementary fuzz lanes

The permanent assurance system contains:

1. **Contract/admission fuzzing** — hostile bytes, YAML, canonical values, snapshots,
   histories, evidence and receipts.
2. **Pure state-machine fuzzing** — generated legal and wrong attempts over a compact
   reference game, including corrections, undo and stale moves.
3. **Graph/compiler fuzzing** — move enumeration, preview, apply, compile and replay
   over structured generated graphs.
4. **Metamorphic resolver fuzzing** — candidate order changes, irrelevant history,
   equivalent canonical forms, lane omission and controlled utterance transformations;
   assert only normatively valid relationships, never an assumed semantic label.
5. **Fault/schedule fuzzing** — persistence, lost responses, restart, concurrent
   revision, lease and transaction cut-points through controllable clocks and stores.
6. **Native/Wasm differential fuzzing** — portable packets produce equivalent canonical
   outcomes or equivalent bounded refusal.
7. **Resource-abuse fuzzing** — maximum boards, histories, motifs, feedback chains,
   metadata, graph amplification, model fuel and memory.
8. **Federation fuzzing** — board/context transitions preserve domain identity,
   permissions and disclosure across later `ob-poc` subdomain bridges.

### 12.4 Reproduction and semantic coverage

Every run is identified by source revisions, pack/compiler/policy identities, fuzz seed
and minimized operation tape. Every confirmed finding becomes a permanent committed
regression with a manifest containing finding ID, target, fixed revision, input hash
and expected current outcome.

Receipts report more than line coverage:

- move kinds and concrete binding shapes reached;
- every attempt outcome reached;
- rules and feedback options exercised;
- board-size, graph-shape and history-depth buckets;
- motif starts, completions, abandonment and corrections;
- policy/disclosure classes;
- accepted/refused/clarified/abstained proportions;
- compiler diagnostic and recovery mappings;
- state-transition and outcome-transition pairs;
- corpus minimization and coverage trend.

Every declared target must complete and emit a receipt. Empty regression gates,
unreceipted targets and fuzz workloads that cannot finish inside CI time are failures,
not successful skips.

## 13. Scope

### In scope

- empty-workflow and existing-template design sessions;
- canonical representation of the complete design position;
- deterministic enumeration of concrete legal graph transformations;
- session history and non-authoritative motif belief;
- attempt, feedback, correction and recovery state;
- public rule, piece and feedback retrieval for Sage and application surfaces;
- multi-lane evidence fusion;
- typed arguments and graph anchoring as selection evidence;
- information-gaining clarification;
- palette, utterance and direct-edit convergence;
- preview, ratification and compiler admission;
- real-turn capture, adjudication and game-level evaluation;
- fuzz-first contracts, reference models, semantic coverage and durable fault testing;
- capability-aligned crate facades and public-API governance across application, tests,
  fuzzing and tooling;
- bounded offline prompt-ranker and learned-ranker experiments;
- shadow rollout and replayable decision evidence.

### Out of scope for this programme

- changing BPMN-Lite runtime execution semantics;
- removing compiler, verifier, workbook or ratification boundaries;
- autonomous workflow completion without per-move approval;
- general open-domain workflow generation;
- self-play or reinforcement learning as the first resolver;
- enumerating every possible finished workflow;
- moving BPMN/application vocabulary into shared Rust crates;
- making a remote LLM an execution dependency;
- implementing `ob-poc` subdomain adapters in the BPMN proof programme; the generic
  contracts must nevertheless support their later federation;
- automatic publication or deployment of designed templates.

## 14. Relationship to existing decisions

This document:

- preserves the authority thesis and implementation substrate of
  `EOP-VS-BPMN-DESIGN-003`;
- preserves the semantic decision board, shared contracts, candidate-conditioned
  serializer and proposal workbook already delivered;
- corrects the narrow resolver thesis in `EOP-PLAN-SEM-RESOLVER-001`, where one
  candidate-conditioned SLM was described as the central ranker;
- incorporates the 2026-08-07 finding that the starter evaluator did not exercise the
  production v3 serving path;
- widens the board from semantic verbs to position-bound typed graph moves;
- makes graph structure, history, arguments and clarification first-class evidence;
- treats wrong moves, corrections and feedback as ordinary lifecycle outcomes;
- generalizes the contracts for a federation of bounded `ob-poc` boards without
  pulling `ob-poc` semantics into BPMN or shared Rust;
- adopts the 2026-08-04 fuzz review's operational lessons: independently receipted
  targets, non-empty governed regressions, controlled authority schedules,
  PostgreSQL crash cuts, native/Wasm differential execution, resource budgets and
  corpus/coverage governance.

Until ratified, this is a proposed versioned amendment. It does not silently rewrite
the earlier ratified documents.

## 15. Acceptance definition

This vision is delivered when a user can start from an empty or admitted workflow,
operate through language or the legal palette, and reach an adjudicated target graph
through a sequence where:

- every offered move was legal for the exact recorded position;
- the same move board powered every interaction mode;
- evidence from graph, language, history and typed arguments was recorded per move;
- ambiguity produced a useful governed choice rather than an unsafe guess;
- wrong, incomplete and stale attempts produced truthful rule-grounded feedback and a
  usable recovery path;
- legal-but-unwanted moves remained visible and were corrected through linked moves;
- every accepted move was previewed, ratified and compiled;
- the resulting graph and decision history are deterministically replayable;
- release thresholds are met on real design sessions;
- every gameboard layer has active fuzz targets, semantic coverage receipts and
  permanent minimized regressions for discovered faults;
- application, tests, fuzz targets, examples, benches and `xtask` all exercise the same
  intended capability facades without production visibility expansion;
- no learned component held execution authority.

## 16. Owner rulings proposed

The companion plan proceeds with these defaults unless Adam amends them:

1. **R1 — framing:** adopt the compiler-governed design-game model.
2. **R2 — primary surface:** legal palette and graph manipulation are the dependable
   base; utterance is an accelerator over the same board.
3. **R3 — first statistical model:** use an interpretable calibrated structured-choice
   model before any GNN or reinforcement-learning work.
4. **R4 — belief granularity:** model target motifs and likely next moves, not complete
   target graphs.
5. **R5 — move granularity:** infer position-bound typed moves, allowing partially
   bound arguments; apply only fully bound, previewed moves.
6. **R6 — corpus:** pause further synthetic SLM expansion until the production-equivalent
   evaluator and structural real-turn game funnel are green. Real-turn promotion
   evidence remains required before any learned-policy or release decision.
7. **R7 — human iteration:** wrong, incomplete, stale and corrected moves are expected
   first-class session outcomes, not exceptional error paths.
8. **R8 — feedback:** rules, pieces, explanations and recovery options are typed,
   pack-governed resources retrievable by Sage through SemOS/Repl APIs.
9. **R9 — domain scale:** `ob-poc` is a federation of bounded subdomain boards, with
   governed bridges rather than one global action board.
10. **R10 — fuzzability:** a domain is not gameboard-complete unless its pure transition
    kernel, contracts, wrong-move lifecycle and durable adapters are model/state-machine
    fuzzed with reproducible receipts and regression governance.
11. **R11 — Rust boundary:** production implementation defaults to `pub(crate)` or
    narrower; only explicitly reviewed capability facades and stable contract types are
    `pub`, and no test/fuzz/example/bench/`xtask` need may widen that surface.
