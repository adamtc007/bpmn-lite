# BPMN disposition and audit closure — Phase 8 receipt

**Date:** 4 August 2026  
**Scope:** governed ambiguity, abstention, strict compound boundary and durable
utterance/proposal evidence

## Landed behaviour

- Disposition policy v2 renders a candidate clarification only when the top
  evidence passes the configured floor, the pair is insufficiently separated,
  and both live board contracts carry reciprocal negative contrasts. The
  question is assembled from those contracts; no model-generated question is
  admitted.
- Abstention remains a scored candidate. An empty/impossible semantic position
  contains only abstention and deterministically returns `OutOfScope`.
- Policy filtering happens before evidence. A ranking that names a hidden
  candidate is rejected as off-board and the response path has no candidate
  description from which to disclose it.
- `ActionSpanProducer` is a separately identified evidence seam. The V1
  implementation recognizes exactly two non-empty, semicolon-separated spans
  only when each span is a governed exact phrase on the current board.
- Strict compound evidence returns `Compound` and never creates a workbook or
  concatenates two guessed operations. Conjunction without strict evidence
  remains atomic evidence; two score peaks alone never become two actions.
- Decision records now retain a resolvable full semantic board dump, action-span
  producer identity and a content hash over the complete historical closure.
  Consent-gated development capture carries both new fields.
- Proposal creation, answers, dry-run result, expiry and rejection are copied
  into append-only `ProposalAudit` events. Each event contains the complete
  shared workbook, optional bound plan, diagnostics and diagnostics hash,
  decision-record hash and previous-audit linkage. Ratifying `GraphEdit` events
  link the proposal audit, workbook, source utterance and evidence record.

## Permanent cements

- reciprocal interrupting/rearming-guard contracts produce the same
  deterministic clarification and decision hash on repeated decisions;
- a legacy board with no governed contrast escalates rather than inventing a
  clarification;
- the actual empty semantic-board fixture scores abstention for an impossible
  guard request;
- strict governed semicolon syntax preserves two spans and produces no pending
  proposal or graph mutation;
- a denied candidate is absent from the board and off-board evidence fails
  closed;
- decision-record hashes move with the semantic board, retrieval subset, model
  bundle, policy, turn context, finite ranking, candidate serializer/evidence
  trace and action-span producer;
- rejection persists a terminal workbook linked to the creation audit;
- ratification persists a graph edit linked to the proposal audit and evidence;
- development capture remains empty before explicit consent and retains the
  full new closure after consent.

## Verification

- `utterance-engine`: 51 unit tests, candidate inventory and documentation
  test passed; the focused policy suite contains 12 cements.
- `bpmn-lite-server-designer`: 47 passed.
- `bpmn-lite-store`: 37 passed.
- affected-package Clippy (`--all-targets --no-deps -D warnings`) passed. One
  mechanical pre-existing `sort_by` spelling in the newly touched store
  package was updated to the equivalent `sort_by_key` required by Rust 1.95.
- default-feature all-target workspace check passed.
- the structural Q9 capture gate passed: `q9-capture` is not a default feature
  and no build/CI command enables it.

## Boundary retained

Compound **execution** remains deferred. The strict producer recognizes the
boundary only; it never synthesizes or applies a multi-operation workbook.
Phase 6's independently trained v3 model and owner-ratified promotion thresholds
remain externally blocked and were not fabricated or substituted.
