# EOP-REPORT-SLM-BAKEOFF-001 — DIR-002 Phase E: the tier-1 SLM bake-off

**Status:** DELIVERED 2026-07-28. Nothing in this report promotes anything. The promotion ladder is untouched: shadow → suggest-only happens only at G3, against criteria whose threshold values are Adam's to set. Base-model ratification is likewise Adam's — this report carries a recommendation, not a decision.
**Authority:** EOP-DIR-BPMN-DESIGN-003-002 (Phase E deliverable list); EOP-SPEC-SLM-TRAIN-001 v0.3; EOP-PLAN-BPMN-DESIGN-003 v0.2 (WS-C / Phase D "compare against the tier-0 matcher alone").
**Executor:** Fable (Claude Code session, 2026-07-28), branch `feat/dir-002-phase-c-slm-training`.

---

## 1. The headline

The thesis survived contact with measurement. Tier-0 (the pinned Candle BGE embed matcher, phrase-vs-description cosine) puts the gold candidate *somewhere* in its top-8 96% of the time but picks it **first** only **44.9%** of the time — it is structurally blind to graph position (`EmbedTier0::retrieve()` never sees the context projection). The fine-tuned cross-encoders, which read utterance + the A1 context serialization together, roughly double that.

Statistical honesty at n=98: the ~±6pp band around 88.8% and ~±10pp band around 44.9% do not come near each other — the *directional* conclusion is solid. The *numbers* are "high eighties, synthetic distribution" and must not seed G3 threshold values, which get set against real-session data.

## 2. Bake-off table

Baseline: `tier0_top1_accuracy = 0.4490` (44/98), eval = 98-entry held-out slice (disjoint board-families, includes a regime never seen in training banks), scored through Candle behind the real `SlmResult` contract.

| base | params | top1 e2e | uplift | best epoch | cal. T | val-NLL cal. | lat. mean K=9 | lat. mean K=13 | ambiguity margin (vs ordinary) | conf-split pairs |
|---|---|---|---|---|---|---|---|---|---|---|
| **modernbert-base** ★ | 149M | **0.8878** | **+43.9pp** | 1/4 | 1.43 | 0.565 | 1423ms | 1979ms | 0.62 vs 0.79 | 9/37 |
| gte-modernbert | 149M | **0.8878** | **+43.9pp** | 0/4 | 1.18 | 0.375 | 1406ms | 1953ms | 0.71 vs 0.85 | 7/37 |
| bge-reranker | 278M | 0.7959 | +34.7pp | 1/4 | 1.03 | 0.497 | 1181ms | 1774ms | 0.60 vs 0.70 | 11/37 |
| ms-marco | 22M | 0.7449 | +29.6pp | 3/4 | 1.75 | 0.858 | **224ms** | **312ms** | 0.68 vs 0.78 | 15/37 |

★ = **recommended: `modernbert-base` — RECOMMENDED-PENDING-ADAM, not ratified.** At n=98 the two ModernBERT-family bases are statistically indistinguishable; the scores cannot separate them, so the tiebreak is provenance: plain Apache-2.0 MLM base, no inherited reranker head in the bundle lineage, and the "ModernBERT-in-Candle verified not assumed" flag closed by receipt. `gte-modernbert` is the recorded runner-up. Latency is per-utterance on M4 Pro CPU, fp32, unbatched, whole served list in one forward — the 149M tier needs batching/quantization receipts before interactive serving; `ms-marco` is the only comfortably sub-second base and is the designated fallback if latency ever dominates accuracy.

Bundle identities (blake2b-256 of `model.safetensors`; full recipe in each bundle's `training_card.json`):

- modernbert-base `73bf9ca14c9df247097bbe26ec83460c6ca2caafca000398af9b459662d787e4`
- gte-modernbert `ace90c769593bded6d043ad7fc52881732981aa712c5ca09af05446bfece4df4`
- bge-reranker `1a8e04563fbc889313b6692740ecd3522dab0fabad94e3bde337e42298baf1da`
- ms-marco `c59131a187e11e8abb82218d186338251180b034390b3d0bafcd6a0140d39b25`

Recipe (identical across bases, per A4): pretrained encoder + freshly initialized scalar head (uniform — two checkpoints cannot structurally reuse their own heads, so none get theirs), listwise softmax-CE over the real `tier1_list` (tier-0 top-8 + NOTA, finding-5 ruling), input = utterance + ctxproj.v1 serialization ‖ candidate description, max_length 256, AdamW lr 2e-5, grad-accum 16, 5 epochs, seed 20260728, family-level 80/10/10 split (manifest committed), best-checkpoint-by-val-loss export.

## 3. Two findings that corroborate the closed-domain thesis independently

1. **The reranker-pretrained and plain-MLM ModernBERT variants tie exactly (87/98 both).** General reranking priors added *nothing* — the fine-tune distribution dominates entirely. This is the "world knowledge is an adverse prior; capacity matched to the task's entropy is what matters" argument as an empirical result.
2. **The largest model underperforms.** bge-reranker (278M, nearly 2× the winner) lands 9pp lower. Same conclusion from the other side: capacity isn't fit. The task's information content is bounded by the closed vocabulary and the corpus; past sufficiency, more parameters buy overfitting surface, not accuracy.

## 4. The recall@K ceiling — measured, with an open ruling for Adam

Tier-0 recall@8 = 95.9% is a hard ceiling tier-1 can never recover: 4 of 98 gold labels never reach the served list. The curve: K=1: 62.2% · K=2: 75.5% · K=4: 85.7% · **K=8: 95.9% · K=12: 100%** · K=16: 100%. All four K=8 misses sit at gold ranks 9–11 — the embedding is not lost, the cutoff is one notch too tight.

Latency cost of widening K=8→12 (mean per utterance): ms-marco +88ms (224→312), modernbert-base +556ms (1423→1979), bge-reranker +593ms, gte-modernbert +547ms. A cross-encoder happily absorbs the noisier retrieval set; on this eval set widening is pure recall gain.

**OPEN RULING (Adam): K=8 is a ruled value (spec S5). Recommendation: widen to K=12.** On this evidence it converts a permanent 4% error floor into ~40% more cross-encoder compute per utterance — nearly free for ms-marco, material-but-not-structural for the 149M tier (whose serving latency needs batching/quantization work regardless). Not implemented; awaiting the word.

## 5. A3 mitigations — measured results

- **A3.1 label leakage / string-matching collapse:** Jaccard ≤0.5 cap enforced at generation against the gold description (NOTA: against every boarded description); 2 breaches dropped and counted across 5,394 authored entries. No utterance quotes its candidate.
- **A3.2 teacher distribution collapse:** five authoring rounds by independent agent fan-outs under different style instructions; five regimes (terse/telegraphic/full/spoken/dsl_shorthand); cross-bank normalized-token and Jaccard>0.6 near-duplicate sweeps each round (agents self-caught and rewrote up to 154/320 of a draft). *Honest gap:* formal n-gram diversity statistics were not computed — regime mix and dedup enforcement are the receipt that exists; a corpus-card diversity table remains open work.
- **A3.3 shared blind spots:** eval slice authored under disjoint personas and includes a regime (`rushed_ops`) present in zero training banks. The A2.5 ambiguity set (37 constructed pairs) was never trained on and never force-labelled. **The human-authored eval utterances the spec calls for are STILL OUTSTANDING — request repeated here explicitly: a small set from Adam and colleagues would be the first non-synthetic signal the pipeline has ever seen.**
- **A3.4 split leakage:** family-level split (`class::label` connected components), seed 20260728, manifest committed before any base trained; all four bases trained under the identical split. Val-NLL of each reloaded bundle exactly matches its training-time best val loss — the export/reload path is bit-faithful.

## 6. Ambiguity-set behaviour (A2.5)

Direction correct on every base: constructed-ambiguous pairs score closer than ordinary examples (e.g. winner-margin median 0.69 vs 0.96 on the recommended base). But raw confidence on unlabelable pairs remains substantial, and 7–15 of 37 pairs (per base) split *more* confidently than the median ordinary example. Stated plainly: **uncalibrated margins cannot drive the clarification path.** The fitted temperatures (T=1.03–1.75, all over-confident in the expected direction) are the mechanism; the threshold values that decide when "close" means "ask" are E5/G3 material — Adam's.

## 6a. Position invariance (per-class accuracy, recommended base)

11 of 13 board positions score ≥5/6; `message_wait`, `send_task`, `parallel_branch_interior`, `start_anchor`, `end_anchor`, `empty_graph`, `data_object` are perfect. Two positions dip: `guard_node` 6/8 and — the one real finding — **`xor_gateway` 4/8**. The gateway board's near-synonym cluster (`create_branch` vs `insert_after` vs `connect` at a routing node) is the hardest discrimination in the vocabulary, and single-digit cells mean this is directional, but it is where targeted corpus reinforcement (more xor-anchored context pairs) should go before any retrain. Recorded in `eval_scores.json` per base.

## 7. Process events

- **Caught-by-process: overfit checkpoint export.** The first training pass exported final-epoch weights unconditionally; auditing the per-epoch val curves after reporting preliminary numbers found 3 of 4 bases past their val-loss minimum. Fixed (best-checkpoint selection), all four retrained, conclusion held and strengthened (+2 to +4pp; every base chose a non-final epoch — including ms-marco, whose earlier "still improving" look was an artifact of not training long enough to see the turn). Second self-caught near-miss of this build after the `ExecutableWorkflow` store; the discipline is transferring to the executor.
- **Prompt-injection attempt refused** by an authoring subagent mid-corpus-build (fake tool-result claiming its scratch file was externally modified, instructing concealment); refused, flagged, output independently verified before acceptance.

## 8. Open risks — led by the one the spec says not to soften

1. **Synthetic-only evaluation overstates real-world performance until session data exists.** Both sides of every comparison here are same-pipeline synthetic. This is not softened: the 88.8% is an upper bound on what real designers' utterances will show, not an estimate of it.
2. n=98 eval; per-class cells are single-digit; treat all sub-metrics as directional.
3. The 4% recall ceiling until the K ruling lands.
4. 149M-tier CPU latency (1.4–2.0s/utterance unbatched fp32) needs batching/quantization receipts before interactive use; ms-marco is the latency fallback.
5. Calibration is fitted but thresholds are unset; the clarification path is not yet armed.

## 9. What this report does not do

No promotion. No wiring beyond shadow. No threshold values. No K change. No base ratification — `modernbert-base` is recommended; one word from Adam ratifies it. The retraining loop (corpus → train → calibrate → score → bundle) exists as committed, re-runnable code; when the Q9 charter lands and real session records accrue, retraining with mixed provenance is a routine run, not a research project. **The charter remains the sole blocker between this measured shadow pipeline and evidence that counts.**
