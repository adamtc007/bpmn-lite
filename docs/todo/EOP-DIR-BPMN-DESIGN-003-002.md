# EOP-DIR-BPMN-DESIGN-003-002 — SLM Training Approach: Fable as Teacher over the Pack/DAG

**Executor:** Fable (Zed, repo-visible)
**Task class:** CAREFUL throughout — training methodology is where silent mistakes become confident wrong models. Blind review of the training spec (Phase A) before any corpus generation.
**Authority:** EOP-VS-BPMN-DESIGN-003 v0.6 (RATIFIED) — §10 contracts, §10.7 metrics, §10.8 bundle, D17–D20; EOP-PLAN-BPMN-DESIGN-003 v0.2 (WS-C); EOP-VS-BPMN-ISA-002 v0.19 §30 (the language profile — you will use it as training material, see A2.4).
**Objective:** design and execute the training of the tier-1 SLM — a cross-encoder that ranks boarded candidates *conditioned on Designer context* — using **you, an LLM with full repo knowledge of the BPMN pack, the DSL vocabulary, the graph operations/productions, and the compiler's profile, as the teacher that generates the training corpus.** The trained model must understand what the phrase matcher cannot: that the same phrase means different candidates at different Pack/DAG/data positions.

The core idea, stated once so nothing below drifts from it:

> **Labels are correct by construction, not by judgment.** You never read an utterance and guess its label. You pick a real board state and a real candidate first — both produced by the actual application code — and then write the human language for that pair. The generator's choice IS the label. Your domain knowledge goes into the *language variety and the context sensitivity*, never into labelling.

---

## Phase A — Training specification (CAREFUL; blind-reviewed before Phase B)

### A1 — One serializer, or nothing

The context text the model trains on MUST be produced by the **same board/context serializer the runtime uses at inference** (the WS-C projection behind `context_projection_hash`). Train/serve skew — training on one textualisation of the board and inferring on another — is the classic silent killer of exactly this architecture.

- If the WS-C serializer exists: training consumes it (export, FFI, or a generation binary in the Rust crate that emits JSONL). Golden tests pin training-side and inference-side bytes identical.
- If it does not exist yet: **HALT Phase B** and report the dependency. You may NOT write a provisional Python serializer to unblock yourself — that builds the skew in permanently. The serializer is a C-now WS-C item; sequence it first.

### A2 — Corpus design (what you generate, and from what)

Every training example is: `(utterance_text, serialized_board_context, candidate_set_with_descriptions, correct_candidate_id)` where the board state and candidate set are **enumerated by driving the real board-construction code** across: packs loaded, graph positions (empty DAG, mid-sequence, inside a race arm, inside a guard extent, at a gateway, inside an MI region…), subjects, and declared data. Imagined board states are forbidden — if you can't construct it with the application, it isn't training data.

For each (board, candidate) pair, generate a **paraphrase family**:

- **A2.1 Register variety:** terse typed commands; full sentences; spoken-style fragments; domain shorthand from the DSL vocabulary; telegraphic forms ("remind 3x weekly then escalate"). Style-vary deliberately — a corpus where every utterance sounds like you is a corpus that only recognises you (see A3.2).
- **A2.2 Context-sensitivity pairs — the thesis made into data:** the same utterance text attached to *different* board states with *different* correct candidates ("chase them again" inside an armed reminder cycle → modify the cycle; at a fresh request node → REMINDER_THEN_ESCALATE production). These pairs are the highest-value examples in the corpus; the spec must set a minimum proportion.
- **A2.3 Hard negatives by construction:** boards containing confusable candidate sets — near-identical descriptions, InsertBefore/InsertAfter, the three guard kinds, race-vs-guard timeout shapes — with utterances whose correct answer is one of them. Mine confusability from the candidate descriptions themselves (lexical overlap, shared schema shapes).
- **A2.4 `NONE_OF_THE_ABOVE`, taught from the profile:** off-board utterances with `NONE_OF_THE_ABOVE` as the correct label. ISA-002 §30's deviations and deferrals table is a ready-made syllabus — requests for compensation, message start events, unbounded `R/PT` cycles, backward loops, completion conditions are all things a user *will* plausibly ask for and the board *will never* contain. Add wrong-pack requests and out-of-scope chatter. This is also exactly the adverse-prior inoculation: the things general BPMN knowledge suggests are the things this model must learn to abstain on.
- **A2.5 Genuine-ambiguity set (held out of training labels):** utterances you construct to be *truly* ambiguous between two boarded candidates. These are NOT force-labelled; they go to the evaluation set to test that scores come out close (feeding the disposition policy's clarification path) rather than confidently split.

**Data hygiene rules, absolute:** no real client names, UUIDs, account data, or live-session content anywhere in the synthetic corpus — fixture vocabulary only ("ACME Fund", "J. Smith"). A synthetic corpus generated purely from pack/schema/profile content contains no personal data and is therefore **not Q9-gated** — this is what lets training start before the charter lands. The 30k SemOS corpus and any live-session data remain charter-gated (D17); do not mix them in until Adam confirms the charter position, and keep provenance per example (`synthetic-v1` vs `corpus-30k` vs `session`) so gated data is separable forever.

### A3 — Known synthetic-data failure modes and their mandatory mitigations

- **A3.1 Label leakage / string-matching collapse:** if utterances quote candidate descriptions, the model learns lexical matching — the matcher already does that. Enforce a lexical-overlap cap between utterance and correct-candidate description; report the overlap distribution.
- **A3.2 Teacher distribution collapse:** you have favourite phrasings. Enforce n-gram diversity metrics and near-duplicate removal across the corpus; generate across multiple style instructions; report diversity stats in the corpus card.
- **A3.3 Shared blind spots (the V&S already names this for graphs+tests):** the generator and the evaluator must not be the same mind on the same day. The held-out evaluation set must include: (i) a slice generated under *different* prompting/style regimes than training, (ii) every human-authored utterance available (Adam and colleagues supply a small set — request this explicitly in the Phase E report if not provided sooner), (iii) the A2.5 ambiguity set. When charter-governed real session data arrives, it becomes the canonical eval set and synthetic eval demotes to regression suite.
- **A3.4 Split leakage:** split train/val/test by **board-state family and paraphrase family**, never by individual utterance — sibling paraphrases of one intent on one board must land on the same side of the split. Pin seeds; record split manifests.

### A4 — Fine-tune mechanics (prescriptive, since this is the non-expert zone)

- Objective: **listwise over the board** — softmax cross-entropy across the board's candidate scores (including `NONE_OF_THE_ABOVE`), because that is the inference shape; pointwise/pairwise only as ablation.
- Input encoding: `utterance ‖ serialized context ‖ candidate description` per candidate, per A1's serializer; truncation rules fixed and recorded in the bundle.
- Bases: the T3.4a shortlist, identically trained (same corpus version, same recipe, same seeds) — the bake-off compares bases, not recipes.
- Calibration: per-pack temperature/threshold fitting on validation, recorded in the bundle; outputs are `FiniteScore` end to end.
- Everything versioned: corpus card (composition, diversity stats, overlap stats, provenance mix), training config, seeds, environment — feeding the §10.8 sealed bundle.

→ On blind-review approval of the Phase A spec, IMMEDIATELY proceed to Phase B. (Progress: 25%)

## Phase B — Build generators and corpus v1

Implement the enumeration-and-generation pipeline per A2 (real board code drives states; you write language), run hygiene checks per A3, produce `synthetic-v1` with its corpus card. HALT conditions: serializer missing (A1); board code unable to enumerate a context class the spec requires (report as a WS-C ask). → IMMEDIATELY proceed to Phase C. (50%)

## Phase C — Fine-tune the shortlist

Train each shortlisted base per A4. Export safetensors; verify each loads and scores in Candle behind the `SlmResult` contract; assemble sealed bundles. → IMMEDIATELY proceed to Phase D. (70%)

## Phase D — Evaluate on the T3.5 harness

Full §10.7 decomposition per base: board completeness (should be 1.0 on synthetic — if not, the generator is broken), tier-0 recall@K on the synthetic eval set, ranking-given-inclusion, end-to-end, abstention on oracle-absent, latency vs K, hard-case suites, position invariance, ambiguity-set score-separation behaviour, per-pack breakdown. Compare against the tier-0 matcher alone (C5 baseline). → IMMEDIATELY proceed to Phase E. (90%)

## Phase E — Report and stop

Deliver: the bake-off table with a recommended base and the evidence; corpus card; bundle hashes; every A3 mitigation's measured result; open risks (led by: synthetic-only evaluation overstates real-world performance until session data exists — state this plainly, do not soften it); the request for human-authored eval utterances if still outstanding. **Do not promote anything. Do not wire the model into the live disposition path beyond shadow. G3 and all thresholds are Adam's.** (100%)

---

## AFTER THIS IS DONE — do this

1. **Stand up the retraining loop, dormant:** a repeatable `corpus → train → evaluate → bundle` pipeline (one command), so that when the Q9 charter lands and real session records accrue, retraining with mixed provenance (synthetic + session, weighted, separable) is a routine run — not a second research project. Include drift monitoring: the shadow pipeline's recorded scores versus each new bundle's re-scores on the regression suite.
2. **Write the session-data integration note:** exactly which I28 record fields become training fields, confirming the context projection is stored richly enough to train on (serialised, not hash-only) — if it is not, raise the WS-C amendment now, while it is one field, not a migration.
3. **File the trace receipts:** C5's matcher findings and any substrate asks raised (serializer, enumeration gaps) into the plan's receipts section.
4. Then **stop and report**. The next decisions — charter timing, corpus-mixing policy, G3 threshold values, base-model ratification — are rulings, not tasks.
