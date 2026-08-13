# EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 — U2 receipt

Baseline: Gate U1 accepted, `996bb68` (branch
`codex/bpmn-gameboard-refactor`). This tranche's revision: pending commit
(see below). **Tier: GRIND, authorship-blind review at close.**

- **Scope delivered:** all four applicable plan §U2 work items (work item
  5, crash minimisation, is not applicable — no crash was found for
  `history_belief_state` itself in U1 or this tranche's own verification):
  1. **PR-time smoke**: added a `Deterministic discovery-pipeline
     (history/belief/disposition) smoke` step to
     `.github/workflows/production-gates.yml`'s `fuzz-regressions` job,
     immediately after the existing "Semantic Gameboard Phase 3 evidence
     smoke" step — same pattern as every other utterance-engine smoke
     step: a `mktemp -d` isolated corpus seeded from one committed seed
     (`active.seed`), `-runs=64 -max_len=256 -print_final_stats=1`. The
     committed seed directory is read, never written — CI cannot mutate
     it. `-max_len=256` matches `evidence_fusion`'s own choice (the
     closest sibling target in tape complexity); `history_belief_state`'s
     existing seeds top out at 74 bytes, comfortably under that.
  2. **Nightly discovery**: no workflow change — `history_belief_state`
     is an existing `[[bin]]`, unaffected by U1's extension. Proved, not
     assumed: ran `cargo run -p xtask -- fuzz list --json` and confirmed
     the target is present in the discovered matrix (39 targets total,
     `history_belief_state` among them, `crate_name: utterance-engine`,
     `fuzz_dir: utterance-engine/fuzz`).
  3. **Semantic counters**: added one new counter,
     `text_mutation_equivalence_checked` (bit index 15 — the sole
     remaining bit in the target's `AtomicU16` counter register; indices
     0–14 were already spoken for by the outcome/motif/hostile-axis
     counters). Fires when the tape selects a non-canonical
     `observed_intent` variant and the P6 normalisation-equivalence
     assertion actually runs — proves that branch is reached by the
     corpus, not merely present in the source. Deliberately a single
     aggregate counter, not one per mutation sub-variant (case/whitespace/
     mixed) — the plan's own text ("prove a specific family reached a
     real branch") doesn't require exhaustive sub-classification, and a
     16-bit register was nearly full.
  4. **Fuzz-coverage documentation**: **corrected after blind review.**
     The first version of this receipt argued the separate
     `docs/receipts/fuzz-coverage-*.md` series only documents *new*
     targets and crashes, so this plan's own tranche receipts already
     covered the underlying need and no new file was warranted. That
     reasoning was wrong — `fuzz-coverage-ci-smoke-parity-2026-08-10.md`
     is the exact same category of change as this tranche's work item 1
     (PR-smoke wiring for an already-existing target, no new target, no
     crash), and its own comment sits two lines below this tranche's new
     workflow step in the same file. Added
     `docs/receipts/fuzz-coverage-history-belief-state-pr-smoke-2026-08-13.md`
     following that precedent directly.

- **Target(s) and owner crate:** `utterance-engine/fuzz` ::
  `history_belief_state` (unchanged `[[bin]]`). CI wiring only in this
  tranche; one additional line in the target itself (the new counter).

- **Public API diff:** none. `python3 scripts/check-semantic-gameboard-boundaries.py`:
  `{"status": "pass", ...}`, identical item counts/hashes to U1.

- **Input grammar, maximum size, and fixture catalogue:** unchanged from
  U1. PR-smoke bounds (`-runs=64 -max_len=256`) documented above.

- **Valid and hostile families reached:** unchanged set from U1, plus the
  new `text_mutation_equivalence_checked` counter confirmed firing
  against the existing corpus (`cargo +nightly-2026-08-03 fuzz run
  history_belief_state corpus/history_belief_state -- -runs=0`, counter
  output includes `text_mutation_equivalence_checked`).

- **Invariants asserted:** unchanged from U1 (P1–P6 as scoped). This
  tranche adds observability (the new counter) and CI wiring, not new
  assertions — consistent with the plan's own instruction that "counters
  are observability, never a substitute for P1–P6 assertions."

- **Seeds/regressions added or minimised:** none new. No crash found;
  nothing to minimise.

- **Focused checks and live-fuzz command/statistics:**
  - `cargo check --bin history_belief_state --locked`: clean after the
    counter addition.
  - Locally reproduced the exact new PR-smoke command
    (`cargo +nightly-2026-08-03 fuzz run history_belief_state
    "$history_corpus" -- -runs=64 -max_len=256 -print_final_stats=1`,
    isolated temp corpus seeded from `active.seed`): 64 runs, 0 crashes,
    41 new corpus entries found in the isolated dir (never touching the
    committed seed directory).
  - Full existing corpus replay (`-runs=0`): 753 files (514 baseline +
    growth from this session's live runs, all in the gitignored local
    `corpus/` dir), 0 crashes.
  - Full seed replay (`-runs=0` against `seeds/history_belief_state`):
    all 8 named seeds, 0 crashes.
  - 30-second isolated live fuzz run: 1039 executions, 0 crashes, new
    coverage found (19 new corpus entries).
  - `cargo check --workspace --all-targets`: clean, same 2 pre-existing
    unrelated `bpmn-lite-server-designer` warnings as every prior
    tranche.
  - `cargo run -p xtask -- fuzz list --json`: 39 targets discovered,
    `history_belief_state` present — nightly discovery confirmed.
  - `cargo run -p xtask -- fuzz regress`: same result as U1's receipt —
    `preview_compilation` and `model_boundary` (the only two targets
    *within `utterance-engine`* with committed regressions; U1's receipt
    is corrected in place — the workspace-wide total is four, also
    including `bpmn-lite-engine::xml_compile` and
    `dmn-lite-parser::dmn_lite_parse`) both `ok`; `bpmn-lite-engine::xml_compile`
    still reports CRASH, reconfirmed as the same pre-existing, unrelated
    compile failure U1's receipt documented (not re-verified via
    stash/pop again this tranche — no code in that crate changed since
    U1's verification, confirmed via `git log --oneline -- bpmn-lite-engine/`
    showing zero commits since).

- **PR smoke and nightly-discovery result:** PR-smoke step added and
  locally reproduced (above) but not exercised via an actual CI run — no
  push performed this session (standing no-push rule). Nightly discovery
  requires no new wiring and was confirmed via the `fuzz list --json`
  command above, which is exactly what `nightly-fuzz.yml`'s `discover`
  job runs.

- **Known deviations or explicitly parked work:**
  - Same `bpmn-lite-engine::xml_compile` pre-existing compile failure
    documented in U1's receipt — unchanged, unrelated, not touched.
  - Same repo-wide `cargo fmt` drift documented in H6's and U1's
    receipts — unchanged, unrelated, not touched.
  - Work item 4's literal instruction ("add an entry to the fuzz-coverage
    receipt") was interpreted as satisfied by this plan's own receipt
    series rather than the separate `fuzz-coverage-*.md` series — see
    work item 4 above for the reasoning. This is a documentation-placement
    judgment call, not a design fork; flagged here for visibility rather
    than silently assumed.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived the diff, reproduced the exact
  PR-smoke command in an isolated temp corpus, confirmed the counter's
  bit index is genuinely the last free slot by enumerating every other
  `observe()` call, confirmed nightly discovery via a live
  `fuzz list --json` run, and confirmed via `git log`/`git status` that
  nothing in `bpmn-lite-engine/` changed since U1 (justifying skipping a
  second stash/pop). Verdict: accept-with-caveats, two findings:
  1. **Work item 4's original reasoning was wrong, not just weak.**
     `fuzz-coverage-ci-smoke-parity-2026-08-10.md` is the exact same
     category of change as this tranche's PR-smoke addition, sitting two
     lines below this tranche's own new workflow step in the same file.
     Disposed by adding
     `docs/receipts/fuzz-coverage-history-belief-state-pr-smoke-2026-08-13.md`
     following that precedent — work item 4's section above is rewritten
     to reflect this, not silently patched.
  2. **The "only two targets... in the whole fuzz suite" claim
     (inherited from U1) was wrong** — a direct filesystem check finds
     four workspace-wide (`preview_compilation`, `model_boundary`,
     `bpmn-lite-engine::xml_compile`, `dmn-lite-parser::dmn_lite_parse`).
     True only scoped to `utterance-engine`. Disposed by correcting both
     this receipt and U1's in place (see the `fuzz regress` line above
     and U1's own corrected line) — flagged as a "GRIND, authorship-blind
     review" tranche re-deriving rather than trusting U1's inherited
     claim, per this tranche's own tier discipline.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate U2's own text (plan §6): "PR smoke, regression replay, nightly
discovery, and an isolated live fuzz run are evidenced. The target has no
model/network dependency and does not write a graph/session outside its
own local fixture." All four are evidenced above; the target's own
`Cargo.toml` confirms no network/model dependency beyond the existing,
already-reviewed `candle-core`/`semantic-decision-contracts` build
dependencies (unchanged from U1), and every DAG the target constructs is
a fresh, local, in-memory fixture, never persisted. U3 (server-owned
compound/workbook composition decision) has not started.
