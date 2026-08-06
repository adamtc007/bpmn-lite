# EOP-PROPOSAL-G3-THRESHOLDS-001 — G3 promotion thresholds (WS-1.5, for FK-B ruling)

**Status: RULED — adopted as proposed (Adam, 2026-08-06; closes FK-B). These are now the G3 values. The ladder stays shadow until the funnel measures them on adjudicated real turns; thresholds ratchet up only.**
**Measured by:** the WS-1.3 funnel (`funnel_report`) over charter-captured, operator-adjudicated turns — real data only. Synthetic evals remain development instruments and can never satisfy a G3 criterion (I-4).
**Ladder (D18, unchanged):** shadow → suggest-only → workbook/staged-patch. `MapperRollout` already implements exactly these stages with no auto-apply.

## 1. Regimes (P6, instantiated for the BPMN Designer)

The review's five regimes, honestly reduced to what the Designer's single-pack, fixed-board world actually distinguishes:

| P6 regime | Designer instantiation |
|---|---|
| Board-size bucket | Degenerate — one board (26 + NOTA). Recorded N/A; re-opens when boards become position-dependent. |
| Active vs inferred pack | Degenerate — one pack, always active. N/A; re-opens at any multi-pack rollout (which is a separate V&S anyway, per the prototype cap). |
| Exact collision vs semantic | **Live**: turns resolved by the governed-exact lane vs model-ranked turns are separate rows. |
| Read-only vs mutating | **Live, by HarmClass**: every authoring op mutates the draft graph, so the split is `Reversible` vs anything higher on the served candidate. |
| In-domain vs NOTA | **Live**: candidate-labelled turns vs abandoned/off-board turns. |

## 2. Proposed G3-suggest (shadow → suggest-only)

All criteria on the adjudicated-turn funnel, all must hold simultaneously:

| # | Criterion | Threshold | Funnel field |
|---|---|---|---|
| S1 | Evidence floor | ≥ 100 adjudicated turns, ≥ 8 distinct eval classes represented, ≥ 5 distinct sessions | `labelled_turns`, `labels` |
| S2 | Board inclusion | ≥ 99% of candidate-labelled turns have gold on the board | `gold_on_board` |
| S3 | Top-1 end-to-end | ≥ 85% on candidate-labelled turns | `top1` |
| S4 | Confident-wrong, Reversible | < 1% | `confident_wrong` / `labelled_turns` |
| S5 | Confident-wrong, above Reversible | 0 occurrences, absolute | per-turn harm join |
| S6 | Abstention behaviour | ≥ 95% of abandoned turns dispose non-committally | `disposition_correct` on abandoned |
| S7 | Exact-lane integrity | 0 exact-collision turns served as a confident single candidate | evidence-trace join |

Honest anchor: starter-seed-v1 measured tier-1 at 44.1% (n=34). S3 at 85% is the review's floor, kept deliberately — the gap is the point. If S2 fails while S3 passes, the defect is board construction, not the model; fix WS-2 coverage before touching ranking (that is what the decomposition is for).

## 3. Proposed G3-workbook (suggest-only → workbook/staged-patch)

Everything in §2 sustained over a SECOND independent window (no overlap with the qualifying window), plus:

| # | Criterion | Threshold |
|---|---|---|
| W1 | Evidence floor | ≥ 100 further adjudicated turns under suggest-only |
| W2 | Top-1 given inclusion | ≥ 90% |
| W3 | Confident-wrong, any harm class | < 0.5%, and still 0 above Reversible |
| W4 | Suggestion acceptance | ≥ 70% of served suggestions accepted or explicitly selected (not corrected) |
| W5 | No regression | S2–S7 all still passing in the second window |

## 4. Measurement gaps, stated

The funnel cannot yet measure retrieval-subset inclusion (records carry the subset hash only), argument binding, or execution outcome — reported as `not_measured`, excluded from every criterion above rather than assumed passing. Binding/execution criteria enter at the workbook stage's own review when those paths produce records.

## 5. Ratchet rule

Thresholds only ever move up. Any threshold change after ruling is a new versioned proposal, not an edit.

## Ruling (FK-B)

- ☐ Adopt §2/§3 as proposed
- ☐ Adopt with edits (record inline)
- ☐ Reject — new values: ____________
