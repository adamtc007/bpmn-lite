# Semantic Gameboard Phase 4 RED Receipt — 2026-08-07

## Baseline

Phase 3 is committed at
`a3b4784340906d754d43ffd77f79eade67b1cb16`. Gate 3 is green. The
shared contracts are pinned to published `dsl` commit
`f3f781cc42c61066dfb2728c441389f4c34a595d`.

No model training or corpus generation is authorized or required for Phase 4.

## Commands inspected

```text
git status --short --branch
sed -n '681,780p' docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md
rg -n 'HistoryProjection|Belief|Motif|MoveAttemptReceipt|CorrectionKind' \
  /Users/adamtc007/dev/dsl/crates/semantic-decision-contracts \
  /Users/adamtc007/dev/dsl/crates/semantic-pack \
  bpmn-lite-server-designer utterance-engine designer-graph
rg -n 'DesignSessionEventKind|append_design_session_event|ProposalAudit' \
  bpmn-lite-store bpmn-lite-store-postgres bpmn-lite-server-designer
```

## RED findings

1. Graph-backed sessions have a durable append-only event log, but no bounded typed
   `HistoryProjection`. The current position history hash includes the entire event
   serialization and therefore does not provide the decision-relevant, size-bounded
   projection required by Phase 4.
2. Shared `MoveAttemptReceipt`, correction validation, `MotifHypothesis` and
   `DesignBelief` contracts exist, but the BPMN capability does not materialize them
   from live session history.
3. The admitted semantic-pack schema has governed evidence/rule/recovery resources but
   no generic governed motif section. BPMN motif identities, fact patterns, likely next
   candidates, contrasts, completion and abandonment conditions therefore cannot be
   pack-owned or admission-checked.
4. No private deterministic motif matcher or belief updater exists in
   `utterance-engine`.
5. Phase 3 history/correction evidence accepts explicit receipts at the facade, but the
   live server supplies an empty receipt slice because no typed attempt projection is
   reconstructed from the session.
6. `Utterance`, `ProposalAudit` and `GraphEdit` events preserve useful historical
   bytes, but normal wrong attempts are not projected into one typed outcome with rule
   and recovery links. Rejection is an outcome string in an opaque proposal audit, not
   yet a replayed correction-aware attempt history.
7. No durable belief snapshot is tied to position and producer identity. There are no
   motif lifecycle, bounded compaction, repeated-failure or correction state-machine
   fuzz targets in BPMN-Lite.
8. There is no product-level bound for retained attempt receipts, motif hypotheses,
   history projection bytes or belief update work.

## Required implementation boundary

Phase 4 must extend the generic shared pack schema without BPMN vocabulary, then pin a
reviewed immutable shared commit. BPMN graph fact derivation and motif declarations
remain in the BPMN capability/YAML adapter. History, motif matching and belief
algorithms remain crate-private; the server consumes only stable shared contracts via
the existing BPMN facade. History and belief remain evidence only and may not change
compiler legality, preview, ratification or graph mutation.

The concurrent workspace-wide formatting delta and all pre-existing corpus, bundle,
training-log, `.DS_Store`, runner and normative-document changes remain protected and
unstaged.
