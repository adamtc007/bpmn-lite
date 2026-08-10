# Phase 9 — MapperRollout expanded to the plan's six named stages

Date: 2026-08-10

Phase: 9 — shadow rollout, promotion and cleanup.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §15 ("Rollout"),
naming six capability-surface stages: `observe -> shadow -> palette ->
feedback -> suggest -> workbook`, with no `auto_apply` stage.

## Why this, and why now

Investigated whether any genuinely safe, bounded code task exists in Phase 9
before touching anything. Conclusion: promotion and cleanup both require
either real production/adjudicated evidence or a human decision to open a
rollback window that doesn't exist yet (already ruled explicitly in
`docs/receipts/semantic-gameboard-phase7-legacy-rollback-audit-2026-08-10.md` —
"the rollback window itself... is a product/rollout decision (Phase 9), not
an engineering gap"). The one candidate that wasn't a production/business
decision — `MapperRollout` (`bpmn-lite-server-designer/src/rest.rs`)
implementing only 3 of the plan's 6 stages — still touches a live,
production-facing config switch (`BPMN_MAPPER_ROLLOUT`), so it was surfaced
rather than done unilaterally. Adam chose to proceed.

## What changed

`MapperRollout` (`rest.rs:39-` area) widened from `{Shadow, Suggest,
Workbook}` to `{Observe, Shadow, Palette, Feedback, Suggest, Workbook}`,
matching the plan's ordering exactly.

Deliberately **not** done: inventing separate `palette_enabled()`/
`feedback_enabled()` gates. This codebase has exactly two real capability
gates today (`suggestions_enabled`, `workbooks_enabled`), consulted at 7 call
sites (`rest.rs:713-715`, `4743`, `4811`, `5455-5460`, `5486-5489`) — nothing
anywhere distinguishes "expose the legal move palette" or "expose governed
recovery options" as a separate switch. Adding a gate that doesn't correspond
to a real code path would be inventing functionality, not scaffolding for it —
the same category of trap door the working contract forbids elsewhere
(swallowed `Result`s, `#[allow]`s to pass a gate). Instead, `Observe`,
`Shadow`, `Palette` and `Feedback` all evaluate identically under the two
existing gates (both `false`) — pure vocabulary/ordering scaffolding, zero
behavior change for any of them. `Suggest` and `Workbook` are byte-for-byte
unchanged from the prior three-stage design.

`parse()` accepts the three new stage names (`"observe"`, `"palette"`,
`"feedback"`); unrecognized/absent input still defaults to `Shadow`,
unchanged. `configured()`'s test-mode override (`Workbook`, so endpoint tests
exercise the full surface) is untouched.

## Tests

- Extended the existing cement-locked test
  (`mapper_rollout_defaults_conservatively_and_has_no_auto_apply_stage`) not
  at all — left every one of its assertions exactly as written, only the enum
  it references grew.
- Added
  `mapper_rollout_names_all_six_plan_stages_with_no_early_gate_invented`:
  proves all three new names parse and label correctly, and explicitly
  asserts all four pre-`Suggest` stages return `false`/`false` for both real
  gates — so a future PR that wires a genuine palette or feedback gate has to
  touch this test deliberately, not silently drift the stage semantics out
  from under it.

## Results

- `cargo test -p bpmn-lite-server-designer --lib`: 78 passed, 0 failed (was
  77; +1 new test, 0 existing assertions changed).
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, unchanged —
  `MapperRollout` is a private enum (`enum`, not `pub enum`), so this carries
  no public-API surface at all.

## What this does not do

This is scaffolding, not a rollout decision. `BPMN_MAPPER_ROLLOUT` still
defaults to `Shadow` in production with nothing configured; nothing about
this change moves any live deployment toward a later stage, exposes any new
capability, or authorizes progression. Actual stage progression, user-
population gating (`power_user_dictation`/`generic_utterance` — still has
zero code presence anywhere), and promotion all remain exactly as blocked as
before this change: on real adjudicated evidence and an explicit human
rollout decision, neither of which this receipt supplies.
