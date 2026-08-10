# Semantic gameboard Phase 8 follow-up — two production fixes

Date: 2026-08-10

Origin: the coverage audit (`semantic-gameboard-phase8-coverage-audit-2026-08-10.md`)
flagged two `MoveAttemptOutcome` variants — `SystemFailure` and
`DisclosureSafeRefusal` — as having no producer anywhere in the codebase. The user
asked for both to be investigated and fixed. Investigation found real production gaps
behind both, distinct from each other and from the original coverage-audit framing.
This receipt disposes of both.

## 1. `session_graph_edit_endpoint` bypassed `PolicyFilter` entirely

**Finding.** Tracing `DisclosureSafeRefusal`'s natural producer led to a bigger
question than the outcome tag itself: is `PolicyFilter` (policy-hidden candidates) an
authorization boundary, or only a discovery/UX boundary? Evidence for the concern:

- Palette/inference proposal creation (`proposal.rs::start_workbook` →
  `ProposalWorkbook::new_position_bound`) only ever binds to
  `position.legal_moves()`, and `build_bpmn_semantic_board`'s `map_legal_candidate`
  excludes `policy.denied` candidates from that list entirely — so through the normal
  path, a policy-hidden candidate can never be selected or attempted. Sound.
- `session_graph_edit_endpoint` (the raw direct-edit endpoint, `rest.rs:3123`)
  instead applies submitted `Operation`s straight to the graph via
  `designer_graph::productions::apply_production` + `staged.candidate.admit()` — pure
  compiler-structural admission. It calls `resolve_direct_edit` first, but only to
  compute an audit-note label (`semantic_move_equivalent` vs
  `lower_level_direct_edit`); that resolution never gated whether the write was
  allowed to proceed. `materialize_workbook`/`preview_workbook`
  (`legal_moves.rs`) likewise never take a `PolicyFilter` at all.

Confirmed by disabling the fix and testing directly (see below): a raw operation tape
recognizable as a policy-denied candidate resolved to
`NonEquivalent("no_matching_legal_move")` — which the endpoint's pre-existing match
arms treat as "fine, admit as a lower-level edit." The mutation would go through,
just mislabeled in the audit note. `PolicyFilter::default()` is empty everywhere in
this codebase today (confirmed by grep — no code anywhere constructs a non-empty
`PolicyFilter`), so this was dormant, not exploited — but structurally live, and would
silently defeat policy the moment any tenant/session config populates a deny-list.

User ruling (explicit fork, not decided unilaterally): **PolicyFilter is a hard
authorization boundary.** Fix the bypass.

**Fix** (`bpmn-lite-server-designer/src/rest.rs`):

- `resolve_direct_edit` now takes an explicit `policy: &PolicyFilter` parameter
  (previously hardcoded `PolicyFilter::default()` internally — the exact thing that
  made this invisible and untestable). Immediately after `recover_candidate_shape`
  resolves a tape to a named candidate, it checks `policy.denied.contains(candidate_id)`
  and returns a new `DirectEditResolution::PolicyRefused` variant before anything else
  runs — no board/position construction, no admission, nothing staged.
- `session_graph_edit_endpoint` refuses `PolicyRefused` with
  `StatusCode::FORBIDDEN` and a generic `"operation not permitted"` body — before
  `apply_production`/`admit()` is ever reached, before anything is persisted.
  Disclosure-safe: the response never names the candidate or confirms a policy
  verdict, matching `explain_bpmn_candidate`'s existing convention.
- Tapes that don't recognizably match any named candidate (`recover_candidate_shape`
  returns `Err`) are unaffected — policy hides named candidates, not arbitrary raw
  graph surgery with no semantic-move equivalent.

**Red→green trace**
(`test_direct_edit_refuses_policy_denied_candidate`,
`bpmn-lite-server-designer/src/rest.rs`): builds a real session, an `InsertAfter` tape
that recovers to `op.insert_after`. RED confirmed by disabling the check (`if false &&
...`): the same tape under a policy denying `op.insert_after` resolved to
`NonEquivalent("no_matching_legal_move")`, not a refusal — reproducing the bypass
exactly. GREEN: restored, the identical tape now resolves to `PolicyRefused`. Full
`bpmn-lite-server-designer` suite: 76 passed, 0 failed (was 75; one new test).

## 2. Genuine pipeline failures left no trace in attempt history

**Finding.** `attach_terminal_gameboard_attempt` (`rest.rs`) is the one production
call site that converts a workbook's terminal state into a `MoveAttemptOutcome`
receipt. Its `ProposalStatus → MoveAttemptOutcome` match is exhaustive over all 7
workbook states — and none of them represent "an internal error occurred." Tracing the
dry-run/preview handler (`answer_proposal_endpoint`) found four branches upstream of
that match where an unexpected `BpmnBoardError` (not `CompilerRefused`, which already
maps to `DryRunRefused`) or workbook-transition failure returns an HTTP error with
**no receipt recorded at all** — confirmed by grep, this is a generic
`Err(error) => 500/422` pattern repeated across the file. `StaleWorkbook` (revision
drift) is not one of these — it's already caught earlier by
`validate_pending_position`/`workbook.validate_position`, which does transition to
`Expired` and does call `attach_terminal_gameboard_attempt`. The remaining generic
bucket is genuinely "something unexpected broke."

Consequence: `design_history_projection` (which reconstructs the attempt-history
window purely from `gameboard_attempt_receipt_json` on persisted session events) and
`disposition.rs::decide_game`'s "3 recent failures → escalate" check both only ever
see PERSISTED receipts. A run of genuine system failures — exactly the case the
escalation safety net most needs to catch — left no trace at all, so the safety net
never engaged for them.

**Fix** (`bpmn-lite-server-designer/src/rest.rs`): new helper
`record_and_persist_system_failure(demo, session_id, pending, detail)` — constructs a
`MoveAttemptOutcome::SystemFailure` receipt via the existing `record_bpmn_attempt`,
then persists it through the same `append_proposal_audit` → `ProposalAudit` session
event mechanism every other terminal outcome already uses (not a new, parallel
persistence path). Best-effort by design: every internal step uses `let Ok(x) = ...
else { return }` — if receipt construction or persistence itself fails, the caller's
original error response is still returned untouched; this never turns one failure
into two. Wired into all four previously-silent branches in the dry-run/preview
handler: the `ReadyForRatification`/`DryRunRefused` transition failures, the
`project_bpmn_bound_game_turn` failure, and the generic `preview_bpmn_workbook`
refusal.

**Red→green trace**
(`test_system_failure_persists_a_receipt_history_can_see`,
`bpmn-lite-server-designer/src/rest.rs`): stages a real proposal, calls the helper
directly with an induced failure, and asserts via `design_history_projection` that a
`SystemFailure`-outcome receipt is now retrievable from persisted history. RED
confirmed: with the helper body short-circuited to a no-op, the same assertion failed
— the projected history contained only the earlier `Incomplete` staging receipt, no
`SystemFailure` entry, reproducing the pre-fix blind spot exactly. GREEN: restored,
passes. Full suite: 76 passed (included in the same run as fix 1).

## Scope — what this does NOT close

- **`SystemFailure` is fixed at one handler** (`answer_proposal_endpoint`'s
  dry-run/preview path), the one fully traced and understood this session. The same
  generic-error pattern (`Err(error) => 500, no receipt`) recurs at other
  `attach_terminal_gameboard_attempt` call sites and other `preview_bpmn_workbook`/
  `project_bpmn_bound_game_turn` call sites in `rest.rs` (grep: 10
  `attach_terminal_gameboard_attempt` call sites total; at least two more handlers —
  around the replay-recovery and evidence-serving paths — call
  `preview_bpmn_workbook`/`project_bpmn_bound_game_turn` and were not audited this
  pass). Each needs the same tracing (confirm the generic bucket doesn't already
  overlap a well-typed status like `Expired`) before wiring in the same helper —
  not done here, not silently claimed done.
- **The `FocusAbsenceReason`/`DesignFocus::Subgraph` dead-surface question** (item 2
  as originally raised) is a separate, still-open investigation: whether
  `ClearedByUser`/`UnknownReference`/`PolicyDecision` represent real application-layer
  scenarios that should be wired in, are safe to remove, or are legitimately
  aspirational. Not started this pass — this receipt covers only the `SystemFailure`/
  `DisclosureSafeRefusal` production-code fixes.
- The `PolicyFilter` authorization-boundary ruling only closes the one bypass found
  (`session_graph_edit_endpoint`). Whether `materialize_workbook`/`preview_workbook`
  need the same defense-in-depth check (currently protected only by the fact that
  `position.legal_moves()` already excludes hidden candidates upstream, at every
  production call site traced this session) was not separately audited beyond
  confirming today's two production call sites (`proposal.rs::start_workbook`,
  `rest.rs::resolve_direct_edit`) are both now sound.

## Results

- `cargo test -p bpmn-lite-server-designer --lib`: 76 passed, 0 failed (both new
  tests included; each independently red→green verified before landing together).
- `cargo check --workspace --all-targets`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, surface hashes
  unchanged — both fixes are internal to private functions in `rest.rs`, no new `pub`
  surface.
