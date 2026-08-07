# FK-E retrain step 1 — control/treatment protocol (running receipt)

**Design change from the ruled protocol, recorded honestly:** the ruled
baseline ("committed 178-family split") is not a valid control because
the corpus GENERATOR evolved through the lineage merge (bbecabb): the
08-03 baseline corpus was 2,941 records; the current generator emits
3,301 from the same pack. A committed-baseline comparison would measure
generator drift + wording together. The clean single-variable design is
**control vs treatment under the current generator**:

- **Control**: old gateway wording (pre-FK-E pack), current generator →
  3,301 examples. Archived `/tmp/retrain-step1/control/`.
- **Treatment**: FK-E adjudicated wording (inclusive/parallel
  routing-consequence text; artifact 0459649c, source 9c763264),
  current generator → 3,301 examples (identical count — text-only
  delta). Archived `/tmp/retrain-step1/treatment/`.

Both train `modernbert-base` identically (same recipe, seed 20260728,
split derived per-corpus by the trainer's family-level splitter — family
sets are expected identical since wording does not change utterance
templates; the receipt will verify). Scored on the 118-entry held-out
eval slice each corpus emits, with per-class attention on
`or_gateway_node` / `xor_gateway` / `and_gateway_node` (the FK-D
collapse classes), plus starter-seed-v1.

**Decision rule:** treatment is adopted iff or_gateway_node improves
without a net overall regression beyond noise; otherwise the wording
returns to Adam with both runs' numbers.

## State log
- 2026-08-06: wording applied + lock/cement constants updated (60/60
  lib green); ob-poc pin advanced in lockstep (59f35887, gate green);
  both corpora generated and archived; control training started.
- Pending: control train+score → treatment train+score → comparison
  table + adoption call.

## Finding: score_trained_bundle is skew-invalid for v3 bundles (2026-08-06)

Control run scored 0.3305 through `score_trained_bundle` but **0.7588 on
the stored-pair test split** (340 unseen-family records, exact training
text contract, `train_py/eval_stored_pairs.py`). The Rust scorer drives
the corpus_v2 eval textualization against bundles trained on the v3
stored pair sides — a train/score text skew introduced silently by the
lineage merge. All historical eval_scores comparisons through that
example are invalid for v3-contract bundles. Interim ruling: stored-pair
scoring is the valid instrument until the Rust scorer is rebuilt against
the v3 eval closure (carry-over; the missing admission check is
"scorer pair_serializer_hash == bundle card pair_serializer_hash" —
exactly the drift gate shape the estate already uses elsewhere).

Control (old wording), component split, test n=340: top1 0.7588;
gateway classes: or_gateway_node 11/18, xor_gateway 12/13,
and_gateway_node 10/12; weakest: boundary_error 1/7.
Artifacts: /tmp/retrain-step1/control-out/ (weights, card, per-class).

## Result (2026-08-06): treatment NOT adopted per the pre-registered rule

Identical component split, identical recipe/seed, stored-pair test
split (n=340), control (old wording) vs treatment (FK-E adopted
wording):

| | control | treatment |
|---|---|---|
| overall top-1 | **0.7588** | 0.7382 (−2.1pp) |
| or_gateway_node (target) | 11/18 | **10/18** (−1) |
| xor_gateway | 12/13 | 13/13 (+1) |
| guard_node_no_escape | 14/23 | 11/23 (−3) |
| start_anchor | 10/17 | 8/17 (−2) |
| timer_wait / boundary_error | 6/7, 1/7 | 7/7, 2/7 (+1 each) |
| val_loss (trainer) | 0.7561 | 0.8748 |

The targeted class did not improve and overall regressed. Rule:
"adopted iff or_gateway_node improves without net overall regression"
— **both conditions failed**. Honest caveats: single-seed, n=18 for
the target class (±1 is noise); the directional xor↑/or↓ pattern
weakly matches the FK-D asymmetry hypothesis but the OR-wording fix
did not correct it. Disposition returns to Adam: revert the pack
wording (+ ob-poc pin), keep it on clarity grounds despite no
measured gain, or fund a multi-seed replication before deciding.
Artifacts: /tmp/retrain-step1/{control-out,treatment-out}/.

## Augmentation experiment + the embed-generation confound (2026-08-07)

**claude_natural_v1 (125 natural-register utterances, train-only,
frozen instruments): NOT adopted.** Frozen template test 0.7588 →
0.7059 (−5.3pp; saturated classes parallel_branch_interior +6,
start_anchor +3, or_gateway +1 improved, the rest paid for it);
starter-seed 8/34 control vs 7/34 augmented — noise at n=34. Bank
parked (`claude_natural_v1.json.parked`), retest after the fix below.

**The discovery that matters: every corpus regenerated in this series
was built WITHOUT the `embed` feature** — corpus_gen fell back to the
lexical tier-0 for retrieval-list composition, which "structurally
excludes the context-sensitivity pairs tier-1 exists to resolve"
(designer state doc). The historical corpus was embed-generated.
Consequence: all three of tonight's bundles (control / treatment /
augmented) score 21–24% on starter-seed where the 2026-08-03 bundle
scored 44.1% — an internally consistent series on a silently degraded
pipeline configuration. The control-vs-treatment wording verdict
stands (fair internal comparison); every ABSOLUTE number from this
series is non-comparable to history.

**Corrective action (next training act):** regenerate the corpus with
`--features embed`, retrain the control-equivalent, verify starter-seed
recovers to ~44%, and add a fail-closed generation gate: corpus_gen
must refuse to run (or the card must record and the trainer refuse)
a retrieval-producer identity other than the one the standing corpus
declares — the same producer-identity discipline serving already has.
