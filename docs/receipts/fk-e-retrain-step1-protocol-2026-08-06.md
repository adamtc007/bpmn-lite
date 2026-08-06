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
