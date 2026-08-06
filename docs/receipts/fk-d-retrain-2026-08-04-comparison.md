# FK-D receipt — disposition of the uncommitted 2026-08-04 retrain

**Ruling:** investigate first (Adam, 2026-08-06). **Disposition on evidence: discard from the working tree; the committed 2026-08-03 corpus-v3 receipt (68c7e95) remains the baseline.** Raw artifacts archived (see below) — nothing destroyed.

## What the run was

An uncommitted re-split + retrain + re-score dated 2026-08-04 (files sat dirty until 2026-08-06): split 178→165 families (train 132 / val 16 / test 17), records 2941→3097, same seed 20260728; NOTA-forced-to-train family list changed to `data_object`, `guarded_task`, `parallel_branch_interior`, `timer_wait`; modernbert-base retrained (n_train 2638, n_val 152).

## Evidence (same 118-case eval set both runs — per-class totals identical)

| Metric | committed (08-03) | uncommitted (08-04) |
|---|---|---|
| top-1 end-to-end | **0.8390** (99/118) | **0.8051** (95/118) |
| retrieval inclusion | 114/118 | **118/118** |
| top-1 given inclusion | 0.8684 | 0.8051 |
| tier-0 top-1 baseline | 0.2881 | **0.4322** |
| uplift vs tier-0 | +55.1pp | +37.3pp |
| best val acc / loss | 0.7287 / 0.8102 | 0.6908 / 1.0811 |

Per-class deltas (net −4 correct): down — `or_gateway_node` 3/4→**0/4**, `mi_node` 5/6→3/6, `and_gateway_node` 4/4→3/4, `boundary_error` 4/4→3/4, `human_wait` 6/6→5/6, `timer_wait` 4/4→3/4 (−9); up — `parallel_branch_interior` 4/7→6/7, `send_task` 5/7→7/7, `xor_gateway` 6/8→7/8 (+5).

## Diagnosis

1. **The run confounds three changes at once** — family re-split, NOTA-forced-family change, and retrain — so no per-class delta is attributable. This is exactly the churn invariant I-3 of EOP-PLAN-SEM-RESOLVER-001 prohibits before the funnel exists.
2. **The `or_gateway_node` collapse (0/4) alongside `xor_gateway` improving (7/8) is the signature of the open §10.2 xor/or gateway wording question** (EOP-REPORT-SLM-BAKEOFF-001): the corpus doctrine text moved in a way that helps XOR discrimination and destroys OR discrimination. FK-E (wording adjudication) must be ruled before the next corpus regeneration.
3. **Not all bad — the retrieval/tier-0 side genuinely improved**: inclusion went to 118/118 and the model-free tier-0 baseline rose +14.4pp, meaning the candidate-text changes made the deterministic lanes stronger. Half the "uplift regression" is the baseline rising, not the model failing. This is preserved as motivation for the corpus text direction, to be re-applied under single-variable discipline.
4. Training itself degraded (val acc −3.8pp, val loss +33%) on a larger val set — consistent with the harder/regrouped split, not diagnosable further without per-case dumps.

## Disposition

- Working-tree `eval_scores.json`, `modernbert-base/training_card.json`, `split_manifest.json` reverted to the committed 2026-08-03 state.
- Raw 2026-08-04 artifacts archived at `docs/receipts/fk-d-2026-08-04-artifacts/` (the three JSONs; the 596 MB safetensors weights remain local-only in `train_py/bundles/modernbert-base/`, superseding nothing — the committed card no longer matches those weights, which is acceptable because bundles are local artifacts and nothing is promoted).
- **Next retrain happens only after**: FK-E ruled, and (per I-3) the WS-1 funnel exists — then re-apply the corpus-text improvement and the re-split as *separate*, individually-scored steps.
