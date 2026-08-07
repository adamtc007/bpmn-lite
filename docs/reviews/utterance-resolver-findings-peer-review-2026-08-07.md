# Peer review: the BPMN utterance resolver — findings, architecture, and the balance question

**Date:** 2026-08-07 · **Status:** FOR PEER REVIEW — findings verified by receipt; the §6 questions are open
**Reviewed artifacts:** `docs/receipts/fk-e-retrain-step1-protocol-2026-08-06.md` (full experiment trail), `EOP-PLAN-SEM-RESOLVER-001` (programme), `EOP-REPORT-SLM-BAKEOFF-001` (history)
**Framing under review (Adam, 2026-08-07):** *"This is the right solution approach and all the ingredients are there — but the weighting / hand-off / concerns are not balanced correctly. This is a simple, defined task domain: build (code) a valid workflow, using ~27 verbs."*

---

## 1. Architecture background

### 1.1 The thesis

Authority, not intelligence, is the organising primitive. The pipeline quarantines
non-determinism to a proposal phase and keeps execution deterministic:

```
utterance ──► semantic decision board ──► evidence lanes ──► disposition ──► human ratify ──► deterministic apply/compile
                (graph decides what              (exact / lexical /            (candidate |                 (correct-by-construction
                 is LEGAL here —                  embed / trained SLM —         clarify |                     AstMutator edit; compiler
                 1–16 candidates)                 evidence, never authority)    abstain)                      proves or refuses)
```

- **The board** is built from the admitted semantic pack (26 candidates: 19 operations
  + 7 productions, + NOTA) filtered by a legality oracle over the draft workflow graph.
  The model can never rank, and the user can never be offered, an illegal action.
- **The pack** is YAML, compiled and content-addressed (`semantic-pack` crate);
  candidates carry typed semantics: intent, applicability, effect, phrases (52 total,
  zero collisions), argument schemas with clarification prompts, negative contrasts.
- **Serving discipline (A1):** one serializer produces both training text and serving
  text; the bundle loader hash-refuses any mismatch (this check worked all week).
- **Promotion discipline:** shadow → suggest → workbook, gated by G3 (ratified values)
  measured by a decomposed funnel over charter-captured, operator-adjudicated real
  turns. Synthetic evidence can never promote (I-4).

Everything in §1 is implemented, gated, and receipted. **None of it is in question here.**

### 1.2 The learned component as currently weighted

Tier-1 is a fine-tuned 150M cross-encoder (ModernBERT-base + scalar head, listwise CE)
scoring `[utterance + context projection] × [candidate semantic slice]` over the legal
board, trained on a synthetic corpus (~2.6–2.7k listwise records; template + authored
bank phrasings), served CPU/Candle in shadow.

---

## 2. Findings (2026-08-06/07 campaign, all receipted)

### 2.1 Infrastructure defects found and fixed — the instruments now hold

| # | Defect | Consequence | Fix |
|---|---|---|---|
| F1 | Split-group leakage: single-pass family collapse broke on chained paraphrase groups (97/165 families overlap; one 95-family component = 79% of records) | All prior splits potentially leaked paraphrases across train/test | Union-find component-level split; validator (fail-closed) now green; floor receipts |
| F2 | `score_trained_bundle` scored v3-trained bundles through the v2 textualization | Historical eval numbers skew-invalid: control bundle read 0.33 when its true stored-pair score was 0.76 | Stored-pair scorer (`eval_stored_pairs.py`) is the interim instrument; Rust scorer rebuild carried over with a serializer-hash gate |
| F3 | No starter-seed measurement was ever run across the v2→v3 corpus switch | A ~2× real-language regression sat unmeasured inside green template-eval receipts for four days | Starter-seed now mandatory in every cycle |

### 2.2 Experiments run and refused (the gates worked)

| Experiment | Design | Result | Disposition |
|---|---|---|---|
| Gateway wording (FK-E) | control vs treatment, identical split/recipe/seed | target class 11/18→10/18; overall 0.7588→0.7382 | **Refused by pre-registered rule; reverted** (pack + ob-poc pin) |
| Claude natural-utterance bank (125 entries, train-only, frozen instruments) | same discipline | template test −5.3pp; starter-seed 8/34 vs 7/34 (noise) | **Refused; bank parked for retest** |

### 2.3 The headline finding: the real-language cliff

| Bundle era | Template eval | Starter-seed (Adam's 34 free utterances) |
|---|---|---|
| v2 corpus, 07-28 bake-off | 0.888 (n=98) | **15/34 = 44.1%** |
| v3 corpus, all 2026-08-06/07 bundles | 0.71–0.76 (n=340, valid instrument) | **7–8/34 ≈ 21%** |
| tier-0 lexical alone | 0.288 | 6/34 = 17.6% |

Three mechanistic hypotheses were tested and **all refuted**: embed-vs-lexical corpus
generation (corpora materially identical), serving-path skew (excluded by green
hash-admission), full-board negative dilution (v3 already trains on K-subset lists;
verified 3,301/3,301). The regression attributes to the v2→v3 corpus regime itself
(content/volume/list-shape), variable not yet isolated. **Open possibility, stated
plainly: the 44.1% may not reproduce** — it was one measurement, n=34, in a different
serving era; if it fails to reproduce, ~21% is the honest baseline and there was no
regression, only an uncorrected early number.

### 2.4 The uncomfortable absolute

Read §2.3 the way a reviewer should: **a fine-tuned 150M model, ranking at most 16
legal, richly-described candidates, gets roughly 1 in 5 of its owner's natural
instructions right.** Tier-0 lexical gets ~1 in 6. Random over a 10–16 board is ~8%.
This is the fact that motivates §6.

---

## 3. What is NOT in question

- The board/legality/authority architecture (§1.1) — every experiment this week
  *strengthened* the case that deterministic contracts + fail-closed gates are right;
  five defects were caught by the system's own refusals.
- The capture → adjudication → funnel → G3 evidence discipline. Real adjudicated
  turns remain the only promotion evidence regardless of §6's outcome.
- The pack as single source of truth (glossary, corpus, serving all project from it).

---

## 4. Implementation strategy as currently sequenced

1. **Bisect the cliff:** reproduce the 07-29 measurement from git history (era
   trainer/corpus/serving). Reproduces → walk v2→v3 deltas single-variable.
   Doesn't → ~21% is the honest baseline; strategy shifts to §6.
2. **Real data:** Adam's captured, adjudicated design sessions (~100 turns) → funnel
   → G3. Unchanged, still the critical path to any visible impact.
3. Carried over: Rust scorer rebuild (serializer-hash gate), corpus producer-identity
   gate, training speedups (hard-negative subsetting, fp16, GPU decision),
   WS-2.C stage-1 candidates, ContextProjection v2 (approved, lands with the
   contract retrain).

---

## 5. The balance critique, stated fairly

The task domain is **small, closed, and fully described**: ~27 actions, each with
typed semantics, distinguishing contrasts, and clarification prompts; boards of 1–16
legal options; a single pack; an authoring context where a wrong suggestion costs one
human "no". The architecture spends its rigor budget well everywhere **except the
resolver choice itself**, which is currently weighted as if the problem were
open-domain language modelling:

- **Weighting:** a trained-from-synthetic cross-encoder carries the discrimination
  load, while the deterministic lanes (exact phrases, lexical, argument/anchor
  evidence) and the *typed contrasts authored precisely to disambiguate* are used as
  tie-breakers or not at all at rank time.
- **Hand-off:** disposition chases top-1. With ≤16 options and typed contrasts, a
  clarify-first posture ("did you mean an exclusive branch — one route wins — or
  inclusive — every matching branch runs?") converts near-misses into two-turn
  successes. Top-1 accuracy may simply be the wrong objective for this domain size.
- **Concerns:** the heavy machinery (corpora, splits, calibration, bundles) exists to
  make a small closed choice — the maintenance cost is visibly larger than the
  decision space it serves.

## 6. Questions for peer review

**Q1 — Resolver weight.** For a ≤16-option closed domain, is a fine-tuned cross-encoder
the right resolver at all? The entire pack — all 26 candidates with contrasts —
serializes to ~3k tokens. A competent instruct-tuned LLM given the glossary and the
legal board as a prompt (same board contract, same evidence-not-authority role, same
disposition gates) plausibly exceeds 44% zero-shot with zero training pipeline.
**Proposed spike R1 (cheap, decisive):** run starter-seed-v1 through a prompt-based
ranker over the identical board contract; compare against 21%/44%. If it wins
decisively, the SLM's role narrows to a local/cheap/deterministic *fallback* (or is
retired), and the training programme's weight shifts accordingly. The architecture is
resolver-agnostic by construction — this changes a component, not the thesis.

**Q2 — Hand-off objective.** Should G3's serving objective be re-expressed for a small
board: "top-3 containing gold + at most one typed clarification question" rather than
raw top-1? The contrasts and clarification prompts already in the pack are the
question generator. (G3 values are ratified; this proposes a *measured additional
regime*, not a weakening — promotion still requires the confident-wrong rate near
zero.)

**Q3 — Lane weighting at rank time.** Exact/lexical lanes and argument-shaped evidence
(a count in the utterance → MI/branch candidates; a duration → timers/guards) are
currently under-weighted relative to the learned score. For this domain size, should
deterministic evidence dominate and the model only break residual ties?

**Q4 — Effort weighting.** Given §2.4, is further synthetic-corpus investment justified
before (a) the R1 spike answers Q1 and (b) 100 real adjudicated turns exist? The
recommendation embedded in this document is **no** — bisection (§4.1) proceeds because
it is cheap and diagnostic, but corpus expansion pauses pending Q1/Q4.

**Q5 — Task framing.** Is "build a valid workflow with 27 verbs" better served by
leaning further into the correct-by-construction editor (guided palette + typed
prompts, utterances as accelerator) than into utterance-first interaction? The
glossary (2026-08-07) is the first artifact of that posture.

## 7. Reviewer verdict requested

Accept/amend the §6 questions; in particular rule on **R1 (the prompt-ranker spike)**
— it is the single cheapest experiment that can resolve the weighting question Adam
raised, and every gate, contract, and receipt built this week applies to it unchanged.

## 8. Semantic Gameboard Phase 0 correction (2026-08-07)

The live-v3 performance claims in §§2.2–2.4 are withdrawn. The evaluator behind the
7–8/34 result built a legacy thin board, recorded no `semantic_v3` closure and sent a
bundle admitted for `bpmn.candidate-pair.v1` through the old
utterance/context-plus-description scorer. The same defect also prevents the older
15/34 result from being used as a semantic-v3 comparator. Those numbers remain
historical records of the invalid instrument; their files were not overwritten.

The corrected Phase 0 instrument uses the graph-position semantic board, the admitted
snapshot, the production candidate-pair serializer, every current-board candidate,
the Candle full-board ranker, semantic evidence finalisation and deterministic
`shadow_v2` disposition. Without retraining or changing a corpus/bundle, the frozen 34
utterances produced:

- top-1 matching the provisional/adjudicated hypothesis: **22/34**;
- top-3 containing the hypothesis: **28/34**;
- NOTA ranked first: **10/34**;
- dispositions: **16 candidate, 8 escalate-to-Sage, 10 out-of-scope**.

The claimed “~44% → ~21% real-language cliff” therefore did **not** reproduce under
the live-v3 route and is withdrawn, along with the causal attribution to the v2→v3
corpus regime. The broader architectural questions in §§5–7 remain legitimate design
questions, but the invalid cliff is no longer evidence for them. The corrected packet,
per-turn hashes and dispositions are in
`docs/receipts/artifacts/semantic-gameboard-phase0-starter-evaluation.json`.
