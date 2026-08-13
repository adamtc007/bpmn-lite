# Fuzz coverage — PR-time smoke for history_belief_state

Date: 2026-08-13

Scope: EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001, Gate U2 work item 1.
Same category of change as `fuzz-coverage-ci-smoke-parity-2026-08-10.md`
(PR-time smoke parity for an existing target that previously relied on
nightly-only live fuzzing) — this file follows that precedent directly
rather than treating this plan's own tranche receipts as a substitute for
it, per a correction raised in Gate U2's own blind peer review.

## What changed

`.github/workflows/production-gates.yml`'s `fuzz-regressions` job gained
one new step, `Deterministic discovery-pipeline
(history/belief/disposition) smoke`, immediately after the existing
"Semantic Gameboard Phase 3 evidence smoke" step, matching the job's
established pattern exactly: `mktemp -d` corpus seeded from the committed
`active.seed`, `cd` into `utterance-engine/fuzz`, `cargo fuzz run
history_belief_state <corpus> -- -runs=64 -max_len=256
-print_final_stats=1`. `-max_len=256` matches `evidence_fusion`'s own
choice — the closest sibling target in tape complexity; this target's
existing seeds top out at 74 bytes.

`history_belief_state` itself was substantially extended in the same
plan's U1 tranche (see `docs/receipts/EOP-UTTERANCE-DETERMINISTIC-FUZZ-001-U1-receipt.md`)
to compose board → position → evidence → belief → disposition as one
deterministic run — this file documents only the PR-smoke wiring; U1's
receipt documents the target's own content, seeds, and invariants in
full.

## Verification

- Ran the exact command the new workflow step invokes, locally, with an
  isolated `mktemp -d` corpus (never the committed seed directory):
  `Done 64 runs in 1 second(s)`, 0 crashes.
- `git diff --stat` (this tranche): touches only
  `.github/workflows/production-gates.yml` (+12 lines) and
  `utterance-engine/fuzz/fuzz_targets/history_belief_state.rs` (+1 line,
  a new observability counter — see the U2 tranche receipt).

## What this does not do

- Does not add PR-time smoke for any other currently nightly-only target
  — this closes the gap for exactly the one target this plan's U1 tranche
  extended, not a broader sweep.
- Does not change `-runs=64`'s depth for any step, old or new — same
  "did we just break something obvious" scope as every other PR-smoke
  step in this job; the nightly 20-minute live-fuzz run and the full
  regression-corpus replay remain the deeper checks.
