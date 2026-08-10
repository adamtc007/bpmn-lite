# Semantic gameboard Phase 7 — concurrent-revision-drift fault tape

Date: 2026-08-10

Phase: 7 — converge APIs and user surfaces

Closes: red-receipt item 6 partial —
`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md` ("The broader
restart/lost-response/duplicate-request and concurrent-revision tape replay
through the in-process API adapter is not yet a Phase 7-qualified suite.").
Stale-client, restart/lost-ratify-response, expired-workbook-retry and
duplicate-rejection-receipt tapes were already landed
(`68a511c`, `f38230d`, `396da6d`, `5fc087c`). This closes the remaining
concurrent-revision-drift case.

## Scenario

Two independently staged workbooks are built against the same base graph
revision (two separate utterances against the same session, before either
ratifies). Their ratify requests race through the real in-process HTTP
adapter (`futures::future::join_all`, same harness pattern as
`test_concurrent_ratify_applies_one_graph_revision`, applied here to two
*different* proposals rather than a duplicate of one).

## Required red assertions (all hold)

- Exactly one of the two racing proposals applies (`200 OK`, real graph
  revision appended); the other observes graph drift and is refused
  (`409 CONFLICT`) on its first attempt — not silently dropped, not
  double-applied.
- The session's `GraphEdit` event count advances by exactly one, never two,
  regardless of which proposal wins.
- The loser's drift refusal is not a transient race artifact: after a full
  `DesignerState` restart, retrying the loser's ratify returns
  `idempotent: true` with `terminal_receipt.outcome ==
  "expired_graph_drift"` and `proposal_status == "expired"` — the same typed
  outcome as the pre-existing sequential case
  (`test_ratify_refuses_on_graph_drift`), durably recorded, not
  re-derived from in-memory race timing.
- The winner's own terminal receipt also replays idempotently after restart
  (`outcome == "ratified"`, `proposal_status == "ratified"`).
- A second restart and a second round of idempotent replay of both
  proposals still shows exactly one net `GraphEdit` — restart and replay
  never append additional graph revisions.

## Test

`bpmn-lite-server-designer/src/rest.rs::rest::tests::test_api_fault_tape_concurrent_revision_drift`

Green under both `--all-features` (63/63 designer tests) and default
features (61/61) — feature-gated `q9-capture` code paths are not exercised
by this test and do not change its outcome.

## Scope retained

This closes the concurrent-revision-drift gap named in the Phase 7 red
receipt's item 6. Still open from that receipt, unaffected by this change:
item 2 (broader direct-edit operation-to-move equivalence beyond single
deletion), item 4 (Sage audit/history compatibility boundary), and item 8
(libFuzzer smoke — host-blocked, no nightly sanitizer toolchain in this
environment). No Phase 7 gate receipt is written by this change; Phase 7
remains RED pending those items.
