# EOP-BRIEF-FK-E-001 — xor/or gateway wording adjudication

**Status: RULED — option (a), Adam, 2026-08-06.** The description audit extends to the `or_gateway` candidate family; Adam adjudicates both families' text together (OR draft in §"OR-family draft" below when authored); then one single-variable retrain — adjudicated wording first on the committed 178-family split, the re-split as a separate scored step. Nothing retrains until then and until the WS-1 funnel has real turns (I-3).

## The question

Three near-synonym `xor_gateway` candidate descriptions (`create_branch` / `insert_after` / `connect`) were rewritten in the 2026-07-29 audit to state their distinguishing routing consequence (before/after text in EOP-PLAN-BPMN-DESIGN-003's Phase 2 receipt). Per the standing rule that any adopted description change obligates corpus regeneration at the next retrain: **is the new wording adopted, reverted, or extended before the next corpus is generated?**

## Evidence on the desk

1. **§10.2 no-retrain re-score (2026-07-29):** serving the new descriptions under bundles trained on the old text cost every base 2–6pp overall with the targeted class flat-or-down. By construction this measured **train/serve skew, not wording quality** — it cannot adjudicate the question, only motivate a controlled retrain.
2. **FK-D receipt (2026-08-06, `docs/receipts/fk-d-retrain-2026-08-04-comparison.md`):** the confounded 2026-08-04 retrain — which regenerated the corpus under the corpus-v3 doctrine — showed `xor_gateway` **improving** (6/8 → 7/8) while `or_gateway_node` **collapsed** (3/4 → 0/4), and the deterministic side genuinely strengthened (retrieval inclusion 114→118/118, tier-0 top-1 +14.4pp). Confounded, so not proof — but the asymmetry is exactly what you'd predict if the XOR family got discriminating routing-consequence wording while the **OR family's descriptions were never audited**: the sharpened XOR text pulls OR-flavoured utterances toward XOR candidates.
3. **starter-seed-v1 (§10.3):** `routing_xor` is the real-utterance category with the most headroom (tier-1 4/7); gateway wording is not a synthetic-only concern.

## Options

**(a) Adjudicate + extend to OR + one controlled retrain — RECOMMENDED.**
Extend the description audit to the `or_gateway` candidate family (named-subset routing consequence wording, same discipline as the XOR trio), Adam adjudicates both families' text together, then a single-variable retrain protocol:
1. regenerate the corpus with the adjudicated wording only, **keeping the committed 178-family split** — score;
2. apply the re-split as a separate, separately-scored step.
This removes the skew (the only clean way to measure the wording, per §10.2's own disposition), addresses the OR asymmetry FK-D exposed, and keeps the tier-0/retrieval gains the doctrine text delivered.

**(b) Revert the XOR rewrite entirely.** Restores symmetry by regression. Abandons the measured deterministic-lane improvement and re-opens the original §10.2 weak spot (xor_gateway 5-6/8 across bases) with nothing learned.

**(c) Keep as-is, regenerate without OR audit.** Ships the asymmetry FK-D flagged; `or_gateway_node` at 0/4 in the confounded run is a plausible preview.

## What a ruling on (a) needs from Adam

- Approve the XOR trio's new wording as-adjudicated (or edit it), review the OR-family wording below.
- Nothing retrains until the WS-1 funnel exists (invariant I-3) — this ruling sequences the corpus work; it does not start it.

## OR-family draft (2026-08-06) — for adjudication

Principle (same as the XOR trio): each description states its distinguishing **routing consequence** relative to its confusable siblings. The OR class's two confusion edges are AND (`op.create_parallel_region`: every branch always runs) and XOR (`op.create_branch`: exactly one route wins). The XOR trio's adjudicated text is NOT touched; `op.create_parallel_region` was never part of that trio and is sharpened here as the other half of the OR/AND edge.

**`op.create_inclusive_region`** (the collapsed `or_gateway_node` class):

| field | current | proposed |
|---|---|---|
| intent_summary | Create conditional branches where one or more named outcomes may run | Create conditional branches where every branch whose condition holds runs — one, some, or all together, never none |
| applicability | The anchor is a non-terminal flow node and each branch has a governed condition | The anchor is a non-terminal flow node and each named branch carries a governed condition |
| effect | Creates a matched inclusive split/join region with non-empty selection | Creates a matched inclusive split/join region whose join waits for exactly the subset of branches selected; at least one branch always runs, unlike an exclusive gateway's single winning route |

**`op.create_parallel_region`** (the AND half of the OR/AND edge):

| field | current | proposed |
|---|---|---|
| intent_summary | Run every declared branch concurrently and join them all | Run every declared branch concurrently — all branches always run, with no conditions — and join them all |
| effect | Creates a matched parallel split/join region | Creates a matched parallel split/join region that always executes every branch, unlike an inclusive region's condition-selected subset |

Rationale anchors: the settled OR-gateway decision (named-subset output types, empty set excluded) is what "never none" / "at least one branch always runs" states in candidate language; the join-subset sentence gives the model the interlocking consequence, which is the fact that separates OR from both siblings.

Per the standing rule, adopting any of this text obligates corpus regeneration at the retrain — which is exactly step 1 of the ruled single-variable protocol.

**Adjudication:** ☑ **adopted as drafted** (Adam, 2026-08-06) / ☐ adopt with edits / ☐ reject. Retrain step 1 (wording-only, committed 178-family split) is unlocked; corpus regeneration obligation applies.
