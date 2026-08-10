# Gate 8 bullet 7 (resource-abuse corpora slice) — decode fuzz harnesses can now reach the new caps

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8" bullet 7,
"resource-abuse corpora pass their separately receipted lanes"), carried forward by
`docs/receipts/semantic-gameboard-phase8-gate-2026-08-10.md` as "OPEN, not
addressed this session," and by
`docs/receipts/semantic-gameboard-phase8-resource-limits-2026-08-10.md`'s scope
note: the new caps added for Gate 8 bullet 4 give the fuzz suite real limits to
target, but every existing decode fuzzer self-caps its input size below where any
of them would bite.

## Correction to how this project actually persists fuzz coverage

Before adding anything, checked how "persist corpora" is actually implemented
here rather than assuming a generic convention. `utterance-engine/fuzz/.gitignore`
excludes `corpus`, `artifacts`, `coverage` and `target` wholesale — `git ls-files`
confirms **zero** corpus files are tracked anywhere in this crate today, for any
target. The only git-persisted fuzz artifact mechanism is
`fuzz/regressions/<target>/` plus the hash-governed `fuzz-regressions.json`
manifest `scripts/check_fuzz_regressions.py` validates — and that manifest is
specifically for **confirmed, fixed crash findings** (`fixed_commit`,
`expected_current_outcome` are required fields), not proactive coverage seeding.
Corpus accumulation is CI-driven (`nightly-fuzz.yml`), not git-committed. This
receipt does not fight that convention by force-adding corpus files against the
`.gitignore` — doing so would misrepresent ad hoc local seeds as the project's
governed regression mechanism, which they are not.

## What was actually the gap, and what closes it

The real defect bullet 7 points at is narrower than "missing corpus files": the
survey behind
`docs/receipts/semantic-gameboard-phase8-resource-limits-2026-08-10.md` found
every decode fuzz target imposes its own `MAX_INPUT_BYTES` harness-tractability
guard that **skips (`return`) any input larger than the cap**, and several of
those caps sit below where a real product resource limit would even fire — so
coverage-guided fuzzing, however much CI runs it, can structurally never
discover the resource-limit refusal path. Raising the corpus size limit alone
(if corpus were committed) would not have fixed this; the harness itself has to
be able to construct an oversized field in the first place.

Fixed for `rule_explanation_decode.rs`, the one target where a `ContractText`
field it directly controls (`RuleExplanation`'s `provenance`, validated against
the new `MAX_CONTRACT_TEXT_BYTES = 64 KiB`) is entirely determined by fuzzer
input length: raised `MAX_INPUT_BYTES` from 8 KiB to 96 KiB — enough for one
split chunk to exceed 64 KiB with headroom left for the other chunks and
separators — and corrected the harness's own stale comment ("Only the
empty/control-character provenance case can still fail here"), which was true
at 8 KiB and is no longer true at 96 KiB.

Verified, not just changed:
- `cargo +nightly fuzz run rule_explanation_decode <handcrafted 65596-byte input
  with a 64 KiB+1 provenance chunk> -- -runs=1`: executes cleanly in 2ms, no
  crash — proves the harness survives hitting `ResourceLimitExceeded` on its own
  `let Ok(explanation) = ... else { return; }` path rather than panicking.
- `cargo +nightly fuzz run rule_explanation_decode -- -max_total_time=8`: 26,935
  runs in 9 seconds, 0 crashes — the raised input ceiling does not destabilize
  general fuzzing throughput or discover any new fault.

## Scope — what this does not close

This is one target, not the full bullet. Named explicitly rather than folded
into "done":

- `legal_move_enumeration.rs` still self-caps its generated graph to 0-4 tasks
  (line 173), far below `MAX_ENUMERATION_CANDIDATES` (4096) or `MAX_LEGAL_MOVES`
  (512) — it cannot reach either new amplification cap. Left tiny deliberately
  for tractable differential testing against its reference model; widening it
  without also widening the reference model is separate work, not done here.
- `correction_history.rs` caps its tape at `MAX_STEPS = 24` (line 21), well under
  `MAX_HISTORY_ATTEMPTS = 64`, so it cannot reach the history-count resource
  limit either. Raising it is not a one-line change: the harness's independent
  `reference_valid` model checks acyclic correction-chain correctness only, not
  size — at a tape length past 64, `project_bpmn_attempt_history` would
  legitimately refuse via the new size-based `ResourceLimitExceeded` even when
  `reference_valid` says the chain is still acyclic-correct, which is a real gap
  in the harness's own assertion logic, not a product bug. Fixing it requires
  teaching the harness's correctness model about the size dimension too, which
  was judged out of scope for this pass rather than risking a rushed, subtly
  wrong assertion under time pressure.
- `semantic_board_decode.rs` decodes `SemanticDecisionBoard`
  (`DecisionBoardError`), a different, pre-gameboard contract type this
  session's `GameboardContractError::ResourceLimitExceeded` work does not touch
  at all — raising its cap would not reach any of the new limits, so it was
  left alone.
- No corpus files are committed (see above) — CI's own scheduled fuzzing
  (`nightly-fuzz.yml`) is what accumulates coverage against the now-reachable
  code path going forward; there is nothing to commit under the project's
  existing `.gitignore`-driven convention.

## Results

- `cargo +nightly check --all-targets` (utterance-engine/fuzz): clean.
- `cargo +nightly fuzz run rule_explanation_decode` (single-input smoke + 8s
  burst): 0 crashes.
- `cargo test -p utterance-engine --all-features`: all passing, 0 failed.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, unchanged (fuzz
  target files are not part of the tracked public-API surface).
- `python3 scripts/check_fuzz_regressions.py`: pass, unaffected (3 governed
  regression cases, unchanged — no crash was found here to regress against).
