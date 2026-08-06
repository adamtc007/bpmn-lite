# BPMN proposal workbook — Phase 7 receipt

**Date:** 4 August 2026  
**Scope:** terminal binding replacement, typed answers, dry staging and
ratification boundary

## Contract reconciliation

The Phase 3 semantic profile described operation applicability but omitted the
resolved positional argument from several candidate argument lists. That made
the Phase 7 rules mutually impossible: the server needed the anchor to
materialize operations, while a workbook was forbidden from inventing a slot
absent from the candidate contract.

The BPMN profile now declares the candidate-specific positional reference
(`anchor`, `target`, `gateway`, `host`, `guard`, `node` or `from`) for every
position-dependent candidate. `op.create_branch` was also corrected to match
the operation the Designer actually exposes: a conditional route to an
existing target, not an unimplemented new branch-body node.

## Landed behaviour

- `proposal.rs` now starts a shared `ProposalWorkbook` from exactly the selected
  candidate's board-carried `ArgumentSpec`s.
- Explicit anchor, bounded quoted identifiers, durations, counts and existing
  node/data references carry deterministic provenance. Unresolved values stay
  typed `Missing`; they are never defaulted.
- `POST /api/dsl/sessions/:id/proposals/:pid/answers` validates typed batches
  atomically. Wrong kinds, unknown slots, duplicate answers, invalid
  identifiers, missing graph/data references, invalid conditions and resource
  bounds leave the previous workbook unchanged.
- Partial answer batches remain `NeedsArguments`. Complete workbooks
  materialize operations, dry-stage through the real production mutator and
  full admission, then transition to `ReadyForRatification` or
  `DryRunRefused`.
- Graph `NodeKey`s are minted only in `materialize_operations`; asking a
  clarification creates only the required workbook identity.
- Pending state holds the workbook plus an optional bound plan and remains
  deliberately ephemeral. Restart loses it without mutation.
- Ratification requires `ReadyForRatification`, rechecks graph identity,
  restages, admits, appends one linked `GraphEdit`, transitions the response to
  `Ratified` and consumes the pending entry.
- Responses expose inference disposition, decision-record hash, workbook,
  proposal status, proposal id and dry-run diagnostics separately. Binding no
  longer overwrites inference with a `MissingArguments` shape.

## Red/green cements

- direct `NeedsArguments -> Ratified` is refused and consumes the one-shot
  proposal;
- undeclared, wrong-kind and duplicate answers return 422 and retain the prior
  workbook;
- graph drift before answers or ratification returns 409 and consumes the stale
  workbook;
- restart drops the ephemeral workbook and later answers return 404;
- rejection preserves the graph and second ratification remains 404;
- complete single-turn extraction still reaches ready-for-ratification;
- a missing insert identifier becomes a workbook, a typed answer completes it,
  dry admission succeeds and the graph stays unchanged before ratification;
- the normative request-and-wait example starts without correlation data,
  accepts an existing typed data reference, materializes and dry-admits.

## Verification

- `bpmn-lite-server-designer`: 46 tests passed.
- `utterance-engine`: 47 unit tests, candidate inventory and documentation test
  passed.
- changed-package Clippy (`--no-deps --all-targets -D warnings`) passed.
- workspace all-feature check passed after closing five stale
  `ExecutionNode::MessageWait` server-runner projections; the runner now renders
  the wait and its correlation edge explicitly, with 3/3 library tests green.
- no `q9-capture` feature was enabled.

## Boundary retained

The v3 training bundle remains externally blocked as recorded in the Phase 6
receipt. Phase 7 operates correctly on the deterministic tier-0 producer and
does not relabel or promote the incompatible v2 bundle.
