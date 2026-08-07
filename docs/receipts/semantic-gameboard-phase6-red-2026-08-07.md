# Semantic Gameboard Phase 6 red receipt

Date: 2026-08-07

Phase: 6 — establish the statistical baseline and learning path

Baseline commit: `1bbdf4b7ae670ff179a08b60ff0560491fc0c0f6`

Branch: `codex/bpmn-gameboard-refactor` (no upstream)

Status: RED. Gate 5 remains green. Phase 6 promotion is data-blocked, while its
capture, adjudication, evaluation and model-boundary infrastructure is authorised for
implementation. No synthetic result may turn this receipt green.

## Entry inventory

```text
Q9_CAPTURE_DIR configured:       no
Q9_CHARTER_REF configured:       no
real evaluation turns visible:   0
real adjudication lines visible: 0
required before promotion:       100
```

The repository contains synthetic v2/v3 evaluation JSONL and concurrent training
artifacts. They are protected work and are not real-turn evidence. They cannot satisfy
Gate 6 or authorize promotion.

## Reproduced red conditions

1. `CaptureEvent` still contains only the legacy `DecisionRecord`, raw utterance and
   dataset class. It omits the canonical `DesignPosition`, complete `MoveEvidence`,
   `DesignBelief`, `GameDisposition`, typed answer, chosen move, delta, attempt outcome,
   governed explanation/feedback closure, compiler result and later correction link.
2. `AdjudicationOutcome` is a candidate-level four-way label. It cannot distinguish an
   exploratory attempt, accepted move, accidental move and system misinterpretation,
   nor record intended focus, arguments, motif or acceptable clarification/feedback.
3. The existing funnel measures legacy board inclusion, top one and legacy disposition
   only. It does not measure top three, clarification success/turn cost, arguments,
   delta correctness, admission, feedback correctness, recovery or reversals.
4. There is no deterministic real-turn split contract by session, observation time and
   semantic family, and no frozen split manifest for real turns.
5. There is no interpretable structured-choice baseline over complete Phase 3 feature
   vectors and no identical-board comparison of the four required producers.
6. Existing Candle guards cover parts of bundle admission and finite calibration, but
   no Phase 6 model-boundary operation-tape target jointly exercises token limits,
   hostile candidate text, Unicode, full-board completeness, non-finite logits,
   refusal and bundle/card mismatch with semantic counters and bounded refusal.
7. The accessible governed dataset contains zero adjudicated real turns, so confidence
   intervals and per-risk-class promotion results cannot honestly be published.

## Required red assertions

- a captured game turn is position-bound, content-addressed and canonically round trips;
- every evidence vector belongs exactly once to a legal move in the captured position;
- belief and disposition identities match that position;
- chosen moves, feedback and explanations cannot reference hidden/off-board content;
- compiler admission and graph delta agree, while refused attempts carry no admitted
  delta;
- correction/undo links retain the original attempt and remain acyclic;
- adjudication distinguishes exploratory, accepted, accidental and misinterpreted
  interactions and cannot silently turn rejection/correction into a positive label;
- real-turn split assignment is deterministic by session/time/family and cannot leak a
  session across partitions;
- structured-choice scores are finite, complete, candidate-order invariant and never
  change the legal move set;
- model-boundary limits fail with a typed bounded refusal before host exhaustion.

## Authority and scope retained

- Full Phase 6 implementation and model-work permission was confirmed by the user.
- The graph and compiler remain authoritative; statistical outputs are evidence only.
- No learned-policy promotion can occur before 100 adjudicated real turns.
- Synthetic data may test infrastructure and report non-promotional baselines only.
- Rejected, undone and corrected attempts are never positive labels without explicit
  adjudication.
- Existing corpus, bundle, training-log and formatting changes remain protected.
