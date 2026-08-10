# Semantic gameboard Phase 7 — restart/lost-response/duplicate-request/concurrent-revision fault-tape suite

Date: 2026-08-10

Phase: 7 — converge APIs and user surfaces

Closes red-receipt item 6 (`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`):
"The broader restart/lost-response/duplicate-request and concurrent-revision tape
replay through the in-process API adapter is not yet a Phase 7-qualified suite."

## Coverage matrix

All four named categories, against the real in-process HTTP adapter (axum
`Router::oneshot` / `futures::future::join_all`, not a mock), in
`bpmn-lite-server-designer/src/rest.rs`:

| Category | Tests |
|---|---|
| restart | `test_ratify_applies_proposal_and_appends_graph_edit`, `test_api_fault_tape_restart_and_lost_ratify_response`, `test_api_fault_tape_stale_client_preserves_new_revision`, `test_api_fault_tape_concurrent_revision_drift`, `test_reject_drops_proposal_graph_unchanged`, `test_restart_drops_ephemeral_workbook` |
| lost-response | `test_ratify_applies_proposal_and_appends_graph_edit` (ratify), `test_api_fault_tape_stale_client_preserves_new_revision` (ratify after drift), `test_api_fault_tape_concurrent_revision_drift` (both winner and loser replay after restart) |
| duplicate-request | `test_concurrent_ratify_applies_one_graph_revision` (same-proposal concurrent ratify), `test_reject_drops_proposal_graph_unchanged` (duplicate reject), `test_graph_drift_before_answers_expires_and_consumes_workbook` (duplicate answer under drift), `test_duplicate_valid_answer_after_workbook_complete_refuses_cleanly` (new — duplicate answer *not* under drift) |
| concurrent-revision | `test_concurrent_ratify_applies_one_graph_revision` (same proposal raced), `test_api_fault_tape_concurrent_revision_drift` (two *different* proposals raced — closed 2026-08-10, the harder case) |

All four categories already had at least one passing directed test before this receipt;
this closes the one genuinely untested corner found during the audit (see below) and
documents the coverage as a whole, since no prior artifact stated it explicitly — the
concurrent-revision-drift receipt (`semantic-gameboard-phase7-concurrent-revision-drift-2026-08-10.md`)
closed its own case but never amended the red receipt's item 6 text, leaving it
formally RED on paper despite being substantively done.

## Definition-of-done interpretation

The plan doc (`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 7 Work item 9) describes
this as replaying "reference-model operation tapes through the in-process API/session
adapter." No shared generative reference-model/tape-replay abstraction exists in the
repo (checked: none in the fuzz crates either) — the tests above instead use what their
own doc comments call a "compact model": hand-written invariant assertions (graph-edit
count, idempotent-receipt outcome) per directed scenario. That is the interpretation
this suite is qualified against. Full generative/property-based tape replay across many
random operation sequences is Phase 8 scope (plan doc §14, "Property tests"/"Fuzz
targets"), not Phase 7's.

## New test closing the one real gap found

`test_duplicate_valid_answer_after_workbook_complete_refuses_cleanly`: resubmitting the
same valid, workbook-completing answer as a second, independent request (not a
same-request duplicate slot name — `test_invalid_unknown_and_duplicate_answers_leave_workbook_intact`
already covers that) must refuse cleanly once the workbook is already complete, leave it
untouched, and ratification must still succeed afterward. Confirmed: `apply_explicit_answers`
(`proposal.rs`) requires `workbook.status() == NeedsArguments`; a second POST after
completion hits that guard and returns `422` without mutating the workbook —
by-construction correct behavior that simply wasn't previously asserted.

## Accepted, named residual (not closed, not blocking)

No test exercises restart/duplicate-request against the *initial proposal creation*
endpoints (`POST /utterance`, `POST /palette/select`) in isolation. Explicitly accepted
as out of scope: these endpoints have no idempotency-key mechanism and are not meant to
be idempotent — a retried creation POST legitimately creates a second, independent
proposal, which is ordinary REST creation semantics, not a fault-tolerance gap. The
scenario that actually matters — two independently created proposals racing to ratify —
is already covered by `test_api_fault_tape_concurrent_revision_drift`.

`test_restart_drops_ephemeral_workbook` documents a real, tested architectural choice
(non-terminal workbooks do not survive restart; only terminal outcomes are durable) —
named here explicitly so it is never later mistaken for an oversight.

## Results

- `cargo test -p bpmn-lite-server-designer --all-features`: 76/76 (was 75/75; net +1,
  0 regressions).
- `cargo check --workspace --all-targets --all-features`: clean.
