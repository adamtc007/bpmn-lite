# EOP-PLAN-SEM-RESOLVER-001 — Realizing the 2026-08-04 architecture review

**Authored:** 2026-08-06
**Originating review:** `utterance_dsl_semos_architecture_review.md` (2026-08-04, findings F1–F10, improvements P0–P7, delivery Phases 1–4)
**Coordinating repo:** `/Users/adamtc007/dev/bpmn-lite` (this doc)
**Estate:** dsl `refactor/sem-os-pack-policy` @ a38eefe (= v0.2.2) · bpmn-lite board lineage `refactor/bpmn-semantic-pack` @ 44afb93 · bpmn-lite SLM lineage `feat/dir-002-phase-c-slm-training` @ 8b9ef11 · ob-poc `refactor/semantic-policy-consumer` @ d36a1779

## 1. Thesis (verbatim intent of the review)

One pack-conditioned utterance resolver: the SemOS graph produces a **small, versioned, semantically rich decision board**; **one candidate-conditioned SLM** ranks that board; **deterministic code retains all authority** (binding, legality recheck, compile, execution). Not a larger model. The review's three measured loss sources — pack/state exclusion before ranking, ranking among hundreds of weakly described candidates, synthetic/serving mismatch — are each owned by a named workstream below.

## 2. Position at plan authorship (verified 2026-08-06)

| Review item | Status |
|---|---|
| Phase 2 "one board contract" | **Done for BPMN**: `SemanticDecisionBoard`/`CandidateSemanticSlice` in `semantic-decision-contracts` v0.2.2; BPMN pack admitted from YAML via `semantic-pack::admit_pack` + lock; train/serve identity bound (shared `tier1_list`, serializer hashes in evidence trace). **Not adopted in ob-poc serving.** |
| P2 reranker | v2 cross-encoder live in designer (shadow, calibrated, NOTA, K=12). **v3 candidate-conditioned serializer implemented, weightless** (CO-02: no PyTorch env). |
| P3 typed phrases | Done in the contract (`PhraseEvidence` roles/provenance, `NegativeContrast`). ob-poc's 15k flat phrase rows + `phrase_bank` workspace TODO untouched. |
| P6 calibration | Per-bundle temperatures done; per-regime thresholds = **G3, values unset (Adam's)**. |
| P7 convergence | Started: designer's ob-poc fallbacks removed; `dsl-sage` orphan retired; Sage boundary ruled (no shared repl-contracts). V1/V2/ACP/Sage-Coder convergence unstarted. |
| P0 / F6 / F10 evidence | **Open.** 80.5% synthetic vs 44.1% real; no funnel; no adjudicated corpus; **no ratified Q9 charter** (capture module built, feature off, CI-enforced, allowlist empty). |
| P1 / F3 metadata | **Open.** `requires_states` 3.02%; pack-scoped sets 73–349 verbs; no case ≤5 candidates. |
| F5 graph population | **Open.** SemOS `bpmn_dag` is a 26-line stateless menu; zero of the 26 `op.*`/`prod.*` authoring candidates exist in SemOS; runtime verb sets diverge 8-vs-13. |
| P4 / P5 / F9 | **Open.** `group.ownership` default, premature pack/subject gating, post-selection-only slot evidence all unchanged. |
| Boundary work (enabler) | Shared crates v0.2.2: one pack schema/loader for all 16 packs (15 ob-poc + 1 BPMN), MIT, rev-pinned everywhere, `SlotKind` now a generic semantic id. **All on three unmerged refactor branches.** |

## 3. Invariants

- **I-1 Authority is deterministic.** Models emit evidence and rankings, never execution authority. SemOS legality recheck immediately precedes execution, always.
- **I-2 One snapshot.** Every board is generated from the same versioned admitted artifact (`semantic-pack` compiled pack / snapshot) used by compiler and policy. No second dialect, no hand mirror.
- **I-3 No ranking churn before the gate.** Per P0: no further ranking/boost/corpus changes on either host until the decomposed funnel over labelled data exists (WS-1.3). The uncommitted 2026-08-04 retrain is disposed under FK-D, not silently absorbed.
- **I-4 Shadow until G3.** Promotion (`shadow→suggest`) happens only against Adam-ratified per-regime thresholds measured on **real, adjudicated** turns. Synthetic evidence never promotes.
- **I-5 Unknown stays unknown.** No implicit context defaults (`group.ownership` dies). Inferred pack = hypothesis; user-selected pack = constraint.
- **I-6 Hash-bound decisions.** Every decision record binds snapshot, board, bundle, and serializer hashes. Cement tests on all of them.
- **I-7 Fail closed.** Missing metadata is a compile/admission reject with a localized diagnostic, never "unrestricted legality" (review risk 9).
- **I-8 Receipts.** Every gate closes on a red→green pair; gates run in CI, not only under `cargo test`.

## 4. Workstreams

Dependency spine: **WS-0 → (WS-1 ∥ WS-2) → WS-3 → WS-4 → WS-5 → WS-6.** WS-1.1 (Q9 charter) is schedule-critical for Gates 1 and 4. WS-2.D (domain metadata) is the long pole. WS-4.2 (BPMN v3 bundle) can run as soon as WS-4.1 lands, independent of the ob-poc legs.

### WS-0 — Convergence and housekeeping (prerequisite)

The estate is currently three unmerged refactor branches plus a decoupled SLM lineage. Nothing in this plan lands cleanly until that is resolved.

- **0.1** Merge disposition for dsl `refactor/sem-os-pack-policy`, bpmn-lite `refactor/bpmn-semantic-pack`, ob-poc `refactor/semantic-policy-consumer` → their `main`s (FK-G, Adam's call: merge now as the v0.2.2 baseline vs hold for external promotion).
- **0.2** Unify the two bpmn-lite lineages: bring `feat/dir-002-phase-c-slm-training` (tier-1 serving, training receipts, WS-D timer work; currently 22 behind main, **zero dsl deps**) together with the board/semantic-pack lineage so the trained ranker and the admitted-pack board live in one tree. Then: repoint ob-poc's stale bpmn-lite pin (`de48b8cf`, pre-v0.2.2) and retire the `ob-semantic-matcher` pin at the stale `ff3f12c7` clone in favour of `semantic-embedder` (the crate exists precisely for this; shared-crates Gate 4 promised it).
- **0.3** Execute FK-D (retrain disposition) before any further training.
- **Gate 0:** one bpmn-lite lineage; every cross-repo pin current and rev-exact; `check-shared-pin.sh` green; clean worktrees.

### WS-1 — Evidence plane (P0, F6, F10; review Phase 1)

- **1.1 Q9 charter (GOV.1 — hard blocker).** Draft the charter for Adam's ratification: scope (designer live-session capture; retrospective use of the 30k corpus per D17), retention, `DatasetClass` separation (already built: Evaluation/Training/Audit, one class per event), provenance labelling, revocation. On ratification: allowlist entry in `check-q9-capture-gate.sh`, `q9-capture` enabled in the designated deployment only, `on_under_charter(<charter-ref>)`.
- **1.2 Adjudication loop.** Extend the capture record with outcome labels (accepted / corrected / explicitly-selected / abandoned); replay-safe operator adjudication CLI over captured `DecisionRecord`s. These are the review's "durable adjudication signal".
- **1.3 Seven-stage funnel.** Per labelled turn: pack∈top-N → verb∈board → verb∈retrieved → top-1 → accept/clarify/abstain correct → subject/args correct → compile/execute result. Split by session/time and phrase family; CI lint that no paraphrase family crosses train/test.
- **1.4 Freeze real eval set v1**: 34-item starter seed + adjudicated turns as they accrue; publish the non-promotion baseline against it.
- **1.5 G3 proposal pack** (FK-B): per-regime thresholds — board-size bucket, active vs inferred pack, exact-collision vs semantic, read-only vs mutating, in-domain vs NOTA — proposed with the review's Phase 3 gates as the floor (board recall ≥99%, retrieval@K ≥99%, top-1|inclusion ≥90%, e2e ≥85%, confident-wrong mutating <1%). Adam sets values.
- **Gate 1** (= review Phase 1 exit): correct canonical verb present on the legal board for ≥99% of in-scope labelled turns — *measured*, with the funnel attributing every failure to a stage.

### WS-2 — Graph population and metadata (P1, F3, F5)

- **2.A BPMN authoring plane into SemOS — RULED + LANDED (2026-08-06): reference-by-pin, not verb registration.** Full registration of the 26 `op.*`/`prod.*` candidates as ob-poc verbs was rejected as a breach of the AST-mutation-isolation settled decision. Instead the SemOS graph pins the Designer pack `bpmn.designer@1.0.0` by exact content hash in two agreeing seed declarations (bpmn_dag `workspace_root` slot annotation + ob-poc.bpmn-ops `typed_extension_points`), gated by `bpmn_authoring_plane_pin_is_consistent_and_well_formed` (ob-poc `a7bccc02`, branch `feat/ws-2a-authoring-plane-pin`). Candidates stay Designer-side. Honest limit, carried over: the pin's truth-link to the compiled artifact is compiler-verified only bpmn-lite-side (`bpmn-semantic-pack.lock`); full cross-repo pin *resolution* (the G4 discipline) lands when SemOS gains pack-pin resolution. Map-root `SlotKind` fix and dmn-lite candidate exclusion remain open under FK-F/F3 follow-through.
- **2.B Runtime verb reconciliation.** Close the 8-vs-13 divergence: `correlate-message`, `list-templates`, `get-template-version` absent in ob-poc (the latter two absent even from bpmn-lite's own manifest); `bpmn-controller.list-instances`, `workflow.start-process`, 4× `loader.*` absent in bpmn-lite. Rule the `bpmn.compile`(XML) vs `define-template`(DSL plan body) bridge contract (FK-C).
- **2.C DSL construct coverage.** Candidates (and, where required, new designer-graph operations — scope ruled under FK-F) for: loop, start/end events, join-mode selection, DMN decision/business-rule-task, task plug/args/delivery-mode, standalone message-wait and timer nodes, flow conditions, edge disconnect/reroute; resolve the multi-instance reverse gap. Each addition: red (coverage gate fails) → green (pack + `OperationKind::ALL` extended in lockstep).
- **2.D ob-poc domain metadata (the long pole).** Raise `requires_states` from 3.02% to near-complete for stateful verbs, highest-volume packs first (kyc-case, cbu-maintenance, onboarding-request). CI admission check, generated from the DAG source (DAG is normative): every executable transition maps to canonical verb + source/target state + subject kind + required args + ≥1 distinctive example + harm/authority class. Phrase-collision linting (`list`/`read`/`create` are never global certainty).
- **Gate 2:** admission gates enforce completeness (reject, not warn); measured board-size distribution shows legal boards ≤~30 for covered packs, whole-board scoring engaged below the cutoff.

### WS-3 — One board contract in ob-poc serving (review Phase 2; P3, F7)

- **3.1** ob-poc's REPL/serving path constructs `SemanticDecisionBoard` from the admitted pack snapshot (crates already pinned at v0.2.2) instead of the hybrid-searcher candidate pool.
- **3.2 One versioned language index.** The compiler emits a single typed phrase index (`PhraseEvidence`: role, locale, workspace/pack applicability, subject/state cues, provenance, status) consumed by exact match, pgvector population, corpus generation, and serving. Retires the `dsl_verbs` flat arrays as a serving source and closes the `phrase_bank` workspace TODO. No independent sync/rebuild paths.
- **3.3** Remove implicit defaults (`group.ownership` — I-5); unknown context flows to clarification, not fabricated context.
- **3.4** ob-poc decision records bind snapshot/board hashes (parity with the BPMN designer).
- **Gate 3** (= review Phase 2 exit): replayed turns produce byte-identical train/serve board hashes and candidate text; provenance on every candidate.

### WS-4 — Retrieval and candidate-conditioned reranking (review Phase 3; P2, F1, F8; CO-02/03/04)

- **4.1 Training environment.** Pinned Python/PyTorch environment receipt (CO-02 unblock) — reproducible, recorded in the bundle card.
- **4.2 BPMN v3 bundle.** Train the candidate-conditioned v3 (serializer/validator/admission already exist): corpus regenerated per bake-off §10.2 (FK-E wording adjudication first), hard negatives drawn from same state/pack, NOTA and explicit-ambiguity examples preserved, pair-survival-under-tokenization assertion enforced (exists). Admit only through the committed validator; immutable by content hash.
- **4.3 Shared domain reranker.** Extend the same cross-encoder pattern over ob-poc boards. Input contract: `[utterance + subject/state/dialogue] × [candidate + typed phrases + source→target transition + args + taxonomy neighbourhood]` — the local pack slice, inside the 256-token budget with truncation-survival checks. Not a giant whole-pack serialization.
- **4.4 Hybrid union retrieval.** For large boards: collision-aware exact ∪ lexical ∪ pgvector phrases ∪ description/contract embeddings ∪ current-state transition candidates ∪ recently-expected actions. Retrieve generously; **one** rerank; delete the lane-priority sort and the additive pack boosts (+0.10/−0.05/+0.15/+0.03) once replay proves parity (F2, F8). Whole-board scoring when ≤~30.
- **4.5** Latency/memory qualification for the production model (CO-04).
- **Gate 4:** review Phase 3 gates, at Adam's G3 values, on the held-out **real** corpus from WS-1. Independent evaluation satisfies CO-03 (pipeline evidence ≠ promotion evidence).

### WS-5 — Uncertainty and slot evidence (P4, P5, F9)

- **5.1 Hypothesis sets.** Carry top 2–3 pack/subject hypotheses; construct the union of their legal candidates with provenance; rank `(pack, subject/state, verb)` jointly. Active user-selected pack = hard constraint; inferred = hypothesis.
- **5.2 Typed slot evidence in selection.** Structured parser emits typed evidence (percentage → ownership/capital; document type → requirement verbs; LEI → GLEIF; status pair → specific transition; plurality → list/one/many) consumed as reranker features. Binding and authority stay deterministic post-selection.
- **Gate 5:** review risks 1–3 (subject-hypothesis exclusion, stale active pack, cross-pack generic action) pass as cement fixtures.

### WS-6 — Route convergence (review Phase 4; P7, F4)

- **6.1** The shared resolver becomes the **only** NL→verb path: V1 chat, REPL V2 matcher, and ACP DAG router cut over, each behind a replay-parity receipt; special-case boosts removed only after replay proves no regression.
- **6.2** Sage demoted to disposition, auxiliary features, and clarification. It never selects a verb through the lossy outcome→Coder handoff (F4: 43.28% → 8.21% is the standing receipt for why).
- **6.3** ACP lexical resolver retained as deterministic fallback and diagnostics, not a second source of truth.
- **Gate 6:** one resolver path in production; replay parity receipts archived; residual special cases enumerated with owners or deleted.

## 5. Cross-cutting cement fixtures

The review's ten risks become permanent tests, distributed: 1–3 → WS-5; 4 (exact collision across legal verbs) → WS-3; 5 (correct phrase below K) → WS-4.4; 6 (candidate text drift without corpus regen) → WS-3.2/4.2 drift gate; 7 (context truncates candidate side) → WS-4.3; 8 (template-step boost overrules explicit deviation) → WS-4.4 boost removal; 9 (absent state read as unrestricted legality) → WS-2.D admission reject; 10 (NOTA on fluent out-of-domain) → WS-4.2 corpus + Gate 4.

## 6. Fork register (Adam rules; surfaced, not decided)

| Fork | Question | Blocks | Status |
|---|---|---|---|
| FK-A | Q9 capture charter | Gates 1 & 4 | **Ratified 2026-08-06** (EOP-GOV-Q9-CHARTER-001 v1.0; live in f8fb444; §9 30k timing = after lineage pass) |
| FK-B | G3 per-regime threshold values (proposal pack from WS-1.5) | Gate 4 / any promotion | Open |
| FK-C | `bpmn.compile`(XML) vs `define-template`(DSL plan body): one bridge contract | WS-2.B | Open |
| FK-D | Uncommitted 2026-08-04 retrain disposition | WS-0.3, I-3 | **Disposed 2026-08-06** — investigated, discarded with receipt `docs/receipts/fk-d-retrain-2026-08-04-comparison.md` |
| FK-E | Gateway wording adjudication (bake-off §10.2) before corpus regeneration | WS-4.2 | **Ruled 2026-08-06: option (a)** — extend audit to OR family + single-variable retrain; OR-family text drafted in the brief, awaiting Adam's adjudication |
| FK-F | DSL-coverage scope: which constructs get new designer-graph operations vs pack-only candidates vs ruled out of authoring scope | WS-2.C | Open |
| FK-G | Merge the three refactor branches to main now vs hold | WS-0.1 | **Ruled + executed 2026-08-06** — all three mains at the v0.2.2 baseline; ob-poc pin-alignment merged (9daf876c) |
| FK-H | External promotion prerequisites (P8-01: registry, non-prod env, traffic corpus, tolerance policy, dashboards) — also the sole blocker on shared-crates Gates 8/9 and the v0.3.0 shim deletion | Deployment legs only | Open — infra decision |

## 7. Verification

- Every gate has a named receipt doc in `docs/todo/` with commands and red→green pairs.
- CI carries: pack admission completeness (WS-2), pin gate (9 packages), Q9 gate (until charter, then allowlisted form), funnel regression on the frozen eval set (WS-1), drift gates (board/candidate text vs corpus), phrase-collision lint.
- Promotion evidence = Gate 4 on real data only. Synthetic evals remain development instruments.

## 8. Non-goals

Per the review and standing rulings: no larger model as a strategy; no BPMN execution-semantics redesign (snapshot/lease/idempotency untouched); no monorepo; v0.3.0 breaking shim deletion stays blocked on the rollback window (FK-H); no crates.io publication.
