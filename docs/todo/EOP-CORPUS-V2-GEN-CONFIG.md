# EOP-CORPUS-V2-GEN-CONFIG — prepared, not run

**Status:** PREPARED ONLY (EOP-DIR-BPMN-DESIGN-003-003, AFTER-item 1). This is a
generation-config proposal, not a change to `corpus_gen.rs`. No corpus has been
regenerated; no base has been retrained. This file exists so the next retrain,
when Adam calls it, is a one-command run against a config that has already been
thought through — not a repeat of this session's discovery work.

**Authority for "when":** Adam's, gated by the Q9 charter (real-usage data
timing) and his adjudication of the open items below. Nothing here schedules
the retrain.

---

## 1. K: 8 → 12

`corpus_gen.rs`'s `const K: usize = 8` moves to `12`, matching `eval_enrich.rs`'s
already-widened value (EOP-DIR-BPMN-DESIGN-003-003 Phase 1). This removes the
deliberate K divergence documented in `eval_enrich.rs`'s header comment — the
four currently-canonical bundles were trained on K=8-sized served lists; a
corpus-v2 retrain at K=12 makes training and serving consistent again.

## 2. Candidate descriptions

Bake in whichever description text Adam ratifies from the Phase 2 audit
(`EOP-PLAN-BPMN-DESIGN-003.md` §Phase 2 receipt, `EOP-REPORT-SLM-BAKEOFF-001.md`
§10.2). Two live options, both already implemented in
`designer-graph/src/board_candidate.rs` (`CANDIDATE_SCHEMA_VERSION` = 2):

- **Keep the audited wording** (`create_branch`/`insert_after`/`connect`, current
  head-of-branch state) — the audit was inconclusive on whether the wording
  itself helps (skew-contaminated read), not negative on the wording.
- **Revert to the pre-audit wording** — a one-line revert plus
  `CANDIDATE_SCHEMA_VERSION` back to a fresh bump (never re-use `1`, per the
  cement rule), if Adam judges the audit's flat-to-negative xor_gateway result
  disqualifying on its own.

Whichever text is live at generation time is what corpus-v2 trains against —
descriptions are board-hash input; there is no "generate once, decide the
wording later" option.

## 3. xor-anchored context-sensitivity reinforcement (new)

The one repeated real finding across both the original bake-off (§6a,
`xor_gateway` 4/8) and the description audit (§10.2, flat-to-negative on every
base) is that the three-way `create_branch`/`insert_after`/`connect` cluster is
the hardest discrimination in the vocabulary, and it has not moved under either
intervention tried so far (more training data was never tried — only
architecture and wording). Corpus-v2 should add a **dedicated paraphrase-pair
regime** at the `xor_gateway` position, specifically constructed so the same
board differs only in which of the three near-synonyms is gold, with utterance
phrasing that leans on the routing consequence (matching the audited
description text) rather than surface keyword overlap with any one
description (A3.1's Jaccard cap already guards against literal quoting, but a
dedicated regime raises the SAMPLE COUNT for this specific three-way
discrimination, which the current 5,018-record corpus under-samples — 8
xor_gateway eval items is not enough signal to have moved either intervention).
Target: at minimum triple `xor_gateway`'s current per-family paraphrase count
relative to other classes.

**Seed phrase set — ratified (Adam, "your suggestions seem sensible - lets go
with them"):** starter examples for the paraphrase generator to expand from,
contrastive by construction (same surface vocabulary, different gold target),
leaning on the routing consequence per the audited descriptions:

| candidate | routing consequence | example utterances |
|---|---|---|
| `create_branch` | new outgoing route, own outcome key | "if it's rejected, branch off to a different path"; "add another route out of here for the escalated case"; "split this so a declined application goes its own way"; "give this a new outcome for 'high risk'" |
| `insert_after` | extends the current route | "after this step, add a review before it continues"; "put a validation check right after the current one, same path"; "insert a step here, staying on this route"; "add one more task before it reaches the end" |
| `connect` | wires two existing nodes | "wire the review step straight to the approval step"; "connect this back to the earlier check"; "join these two steps directly, skip what's between them"; "route this node's output into that existing task" |

Anchoring triples (same anchor, adjacent phrasing, forces the three-way split):
at a gateway anchor — "branch to a new outcome here" (`create_branch`) vs.
"add a step here before it moves on" (`insert_after`) vs. "hook this up to the
existing approval step" (`connect`). These are prose seeds for the generator,
not literal corpus records — `corpus_gen.rs`'s paraphrase expansion (and
A3.1's Jaccard cap) still governs the actual generated set.

## 4. starter-seed-v1 lessons folded in (open items, not yet decided)

From `starter-seed-v1`'s 34-item slice (§10.3), three patterns recurred enough
to be worth deliberate corpus coverage, each still needs Adam's read before
becoming a generation rule:

1. **MI-without-a-stated-ceiling** (seq 18, "do this for each director"): the
   corpus currently has no examples teaching "for each X" WITHOUT an explicit
   cap to prefer a clarification/NOTA reading over direct
   `create_multi_instance_region` construction. If Adam rules that an unstated
   ceiling should NOT auto-construct, corpus-v2 needs a new sub-regime pairing
   capped vs uncapped MI phrasing with different gold labels.
2. **Wait-vs-production ambiguity** (seq 12, seq 22): utterances describing a
   correlated wait in isolation ("park this until X shows up", "when their
   answer lands, wake this up") are currently only trained against the full
   `request_and_wait` production gold, with no contrasting bare-append
   examples. If Adam judges these SHOULD be two distinguishable golds, corpus-v2
   needs paired examples at both readings; if he judges the production reading
   correct either way, no corpus change is needed here — just a plan-doc note
   closing the dispute.
3. **Workflow-level declarations with no node-scoped candidate** (seq 25, "make
   the default budget three for the whole flow"): this is not a corpus problem
   at all — it is a possible missing candidate class (a whole-graph-anchored
   declaration op). Corpus-v2 cannot fix this by adding examples; it needs a
   `designer-graph`/`board_candidate.rs` decision first (new `OperationKind`
   variant or an explicit ruling that this stays NOTA/escalation-only).
   Flagged, not decided, here.

## 5. What corpus-v2 does NOT change

Everything else about `corpus_gen.rs`'s current recipe stays as-is: the A3.1
Jaccard≤0.5 anti-leakage cap, the five-regime authoring-round diversity
mechanism (A3.2), the family-level 80/10/10 split methodology (A3.4), the
NONE_OF_THE_ABOVE-always-appended `tier1_list` construction, the S3 floor
(≥5,000 records). This config only touches K, descriptions, and the two new
items above (§3 xor reinforcement, §4 open patterns).

## 6. Retrain sequence when Adam calls it

1. Apply the ratified description choice (§2) if not already head-of-branch.
2. Bump `corpus_gen.rs`'s `K` to 12 (§1).
3. Add the xor-anchored reinforcement regime (§3) and any starter-seed-derived
   sub-regimes Adam ratifies from §4.
4. Regenerate the corpus (`corpus_gen.rs`), confirm S3 floors still clear.
5. Re-run `eval_enrich.rs` (K=12 already standing) — `corpus_gen.rs` and
   `eval_enrich.rs` are now at the same K, closing the divergence.
6. Retrain all four bases (`train_slm.py`) — uniform recipe, same seed
   discipline, best-checkpoint-by-val-loss export (the overfit-checkpoint fix
   from this session's Phase C is permanent, not a one-off).
7. Calibrate (`calibrate.py`), score (`score_trained_bundle.rs`), re-run
   `starter_seed_eval.rs` against the new bundles (permanent suite, §10.3) —
   this is the number that tells you whether corpus-v2 closed any of the gap
   §10.3 measured, not the synthetic eval number alone.
8. New bake-off report addendum; ratification is Adam's again, not automatic
   carry-forward from this cycle's ruling.
