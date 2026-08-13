# EOP-PLAN-CRATE-HYGIENE-001 — H6 receipt (final tranche)

Baseline revision: `89ae3e6` (H0). Prior tranches: H1 (`496ddb5` +
addendum), H2 (`9a65092`), H3 (`30831f4`), H4 (`3c6a357`, `20bf005`,
`9175549`, `c871faa`), H5 (`ffbe438`, `8754831`, `f0af7b0`, `a81213f`).
This tranche's revisions: pending commit (see below).

- **Scope delivered:**
  1. **Extended `cargo-public-api` gate to every library package**
     (work item 1). The existing narrow gate
     (`scripts/check-semantic-gameboard-boundaries.py`, already installed
     and run in `production-gates.yml`, previously scoped to
     `utterance-engine` and `bpmn-lite-server-designer` only) gained a
     second check function, `check_workspace_public_api_baselines`, that
     diffs every one of the 34 committed per-crate baselines in
     `docs/generated/public-api-baselines/*.txt` (H0's own artifact,
     kept current through H1–H5) against a live `cargo public-api -p
     <crate> -sss` run, plus `check_baseline_coverage`, which fails
     closed if any workspace library package (by `cargo metadata`'s own
     `kind` field) has no committed baseline. Same gate mechanism, wider
     baseline set — not a second, conflicting gate, per the work item's
     own constraint.
     - **Found and fixed while wiring this in**: the pre-existing narrow
       gate's own committed baseline
       (`scripts/baselines/semantic-gameboard-public-api-v1.json`) had
       silently drifted stale during H2 and H5 — it still listed
       `metrics` as an approved `pub mod` for `utterance-engine` (made
       private in H2) and `runbook` as one for `designer-graph` (moved
       out entirely in H5). Root cause: this gate only runs in CI, and
       this branch has never been pushed, so H1–H5 never actually
       exercised it locally. Corrected both stale entries; the
       `surfaces` (item-count/hash) entries needed no change — `metrics`
       was already `#[cfg(test)]`-gated before H2 (so its removal from
       `pub` never touched the *default*-build public API cargo
       public-api reports), and `designer-graph` was never one of this
       narrow gate's tracked `surfaces` packages to begin with.
  2. **`#[cfg(test)] pub` exception-file gate** (work item 2). New
     `scripts/check-test-only-pub.py`: scans every `*.rs` file for a
     bare `pub` item (not `pub(crate)`/`pub(super)`/`pub(in ...)`)
     immediately following a `#[cfg(...test...)]` attribute, and fails
     closed on anything not listed in the new, currently-empty
     `scripts/baselines/cfg-test-pub-exceptions.txt`. One genuine
     candidate was found (`bpmn-lite-engine/fuzz/src/lib.rs`'s
     `#[cfg(test)] pub mod covering;`) — traced its only consumer
     (`covering::ALL_ARCHETYPES` at the same file's own
     `#[cfg(test)] mod tests::write_xml_seeds`) to inside the same
     crate, confirmed no `fuzz_targets/*.rs` binary references it, and
     tightened it to `pub(crate)` instead of adding an exception — same
     H2 discipline (`test_lock`, `utterance_engine::metrics`), applied
     to a crate that had never opted into the workspace
     `unreachable_pub` lint ratchet. Exception file is empty, as the
     work item's own text expects "after H2".
  3. **Test-topology manifest/check** (work item 3). New
     `docs/generated/test-topology-manifest.txt` (R3-classifies all 45
     current Cargo integration-test targets — a direct child of some
     crate's `tests/` dir; helper files pulled in via `mod`, e.g.
     `tests/common/mod.rs`, are not separate targets and aren't listed)
     and `scripts/check-test-topology.py`, which fails closed on: any
     target missing from the manifest, any stale manifest entry with no
     matching file, any `multi`-classified target outside
     `xtask/tests/`, and any `xtask/tests/` target not classified
     `multi`. Enforces plan §3 decision 1 ("xtask/tests is the sole
     multi-crate application-test home") both directions, not just the
     literal instruction's one direction.
  4. **Final inventory and dead-code ruling** (work item 4). Surfaced
     the two items flagged-not-deleted across H3/H4 to Adam
     (`AskUserQuestion`, since production-code deletion is a design
     decision, not a mechanical hygiene step) — ruling: delete both.
     - `bpmn_lite_store::store::TransactionContext` (struct + `new`/
       `add_op`/`get_join_count`, ~30 lines): re-confirmed zero callers
       anywhere in the workspace, deleted outright.
     - `bpmn_lite_compiler::dsl::{WorkflowFrontend, DslFrontend}`: NOT
       simply dead — H4's "confirmed dead_code even internally" claim
       was only true for the *default* (non-test) build; the crate's
       own `#[cfg(test)] mod tests` calls `DslFrontend::lower(...)` at
       14 call sites (the `cargo public-api`/plain-`cargo check` build
       that produced H4's dead_code warning doesn't compile
       `#[cfg(test)]` code at all, so it never saw those uses). The
       trait added no behavior beyond delegating to `lower_plan`, so
       deleting it cleanly required first rewriting all 14 call sites
       from `DslFrontend::lower(&plan)` to `lower_plan(&plan)` (a
       mechanical, signature-preserving substitution), then removing
       the trait/struct/impl block and fixing one stale doc-comment
       cross-reference. This is a materially different finding than
       what was surfaced in the ruling question (which described it as
       "confirmed dead even internally," true only in one build mode)
       — recorded here so the receipt reflects what was actually true,
       not what was assumed when asking.
     - **Not touched**: `bpmn-lite-store/src/store.rs`'s
       `transition_from_tick_ops`, doc-commented "T4 compatibility
       bridge. T7 deletes `TickOperation` and this conversion" — this
       is a different, unrelated initiative's tranche numbering (not
       this plan's H-series), so its removal is out of this plan's
       scope; noted for whoever owns T4/T7, not acted on here.
     - Also found and fixed in passing: `bpmn-lite-engine/fuzz/Cargo.lock`
       had never been regenerated after H2 removed the `bpmn-lite-vm`
       dependency (a separate `[workspace]`-rooted lockfile from the
       main one, so H2's own `cargo check --workspace` never touched
       it) — regenerated via `cargo check` in that sub-workspace;
       10-line diff, `bpmn-lite-vm` package entry and its dependency
       edge removed, nothing else.

- **Files/packages changed:**
  `.github/workflows/production-gates.yml`,
  `scripts/check-semantic-gameboard-boundaries.py`,
  `scripts/baselines/semantic-gameboard-public-api-v1.json`,
  `scripts/check-test-only-pub.py` (new),
  `scripts/baselines/cfg-test-pub-exceptions.txt` (new, empty),
  `scripts/check-test-topology.py` (new),
  `docs/generated/test-topology-manifest.txt` (new),
  `bpmn-lite-engine/fuzz/src/lib.rs`, `bpmn-lite-engine/fuzz/Cargo.lock`,
  `bpmn-lite-store/src/store.rs`, `bpmn-lite-compiler/src/dsl/frontend.rs`,
  `docs/generated/public-api-baselines/bpmn-lite-store.txt`.

- **Public API before/after (`cargo public-api -p <package> -sss`):**
  - `bpmn-lite-store`: **7 removals** (`TransactionContext` struct + 2
    fields + `impl` block's 3 methods — appears twice in the diff,
    module-path and crate-root-re-exported forms, 14 lines total).
  - `bpmn-lite-compiler`: **no diff** — `WorkflowFrontend`/`DslFrontend`
    were `pub(super)`, already outside the public surface; their
    removal is an internal-only simplification.
  - `bpmn-lite-engine-fuzz` (the fuzz sub-workspace, not `cargo
    public-api`-tracked — it's a `[[bin]]`-only package, not a
    library): `covering` module narrowed `pub` → `pub(crate)`; no
    external package depends on this crate at all, so this has no
    downstream effect.
  - `utterance-engine`, `bpmn-lite-server-designer` (the two packages
    the pre-existing narrow gate tracks): **no diff** — only the stale
    `approved_pub_modules` list entries changed, not the packages'
    actual surfaces.
  - All diffs matched what was planned; the workspace-wide baseline
    diff (new `check_workspace_public_api_baselines`) was re-run after
    each source change and the single expected `bpmn-lite-store` delta
    captured in its baseline file.

- **Removed public items and migrated consumers:** `TransactionContext`
  had zero callers to migrate (confirmed by grep before deletion, then
  by a clean `cargo check --workspace --all-targets`).
  `WorkflowFrontend`/`DslFrontend` were `pub(super)` (not public); their
  14 real internal callers (this crate's own unit tests) were migrated
  to call `lower_plan` directly.

- **Added public items and capability justification:** none.

- **Test classification changes:** none — H6 only added the
  *enforcement* of R3 classification (item 3 above) over the
  classification H0/H1/H1-followup/H2 already produced; no test moved
  or was reclassified in this tranche.

- **Focused tests:**
  - `cargo test -p bpmn-lite-compiler --lib dsl::frontend`: 12 passed,
    0 failed (all `DslFrontend::lower` call sites, now `lower_plan`,
    still pass).
  - `cargo test -p bpmn-lite-store --lib`: 46 passed, 0 failed.
  - `cargo test -p bpmn-lite-store-postgres --lib -- --test-threads=1`:
    94 passed, 0 failed (a `--test-threads` default-parallel run showed
    one `ffi_template_store` failure from shared-DB row contention
    between concurrently-running tests in the same database — confirmed
    pre-existing test-isolation flakiness, not a regression, by
    re-running that single test in isolation and the whole suite
    single-threaded, both clean).
  - `cargo test -p xtask --tests -- --test-threads=1`: all 11
    `xtask/tests/*` targets green, 44 passed / 1 pre-existing ignored /
    0 failed.

- **Workspace checks:**
  - `cargo check --workspace --all-targets`: clean, exit 0. Same 2
    pre-existing unrelated `bpmn-lite-server-designer` warnings as
    every prior tranche (an unused variable and a dead enum variant in
    `rest.rs`, both predating this plan).
  - `cargo test --workspace --lib --bins`: 47/47 binaries green
    (unchanged count from H1–H5).
  - `python3 scripts/check-semantic-gameboard-boundaries.py`: clean —
    exercises the extended gate (all 34 per-crate baselines,
    `check_baseline_coverage`), the corrected narrow-gate baseline, the
    dependency-direction checks, and the compile-fixture checks
    together.
  - `python3 scripts/check-test-only-pub.py`: `ok: 0 #[cfg(test)] pub
    item(s), all reviewed`.
  - `python3 scripts/check-test-topology.py`: `ok: 45 integration-test
    target(s) classified and placed correctly`.
  - `cargo fmt --all -- --check`: **not clean workspace-wide** —
    dozens of pre-existing diffs across files this plan never touched
    (e.g. `bpmn-lite-authoring/src/*.rs`), consistent with a local
    `rustfmt` version drift from whatever version CI's pinned toolchain
    uses, not anything introduced by H1–H6. The specific files this
    tranche edited (`frontend.rs`, `store.rs`, the three new/edited
    `scripts/*.py`) show the *same* pre-existing drift pattern as
    every other file in the repo, not a new violation localized to my
    edits — noted here rather than silently worked around, since this
    plan's own rules don't authorize touching unrelated files to chase
    a repo-wide formatting drift.

- **Known deviations or explicitly parked work:** none new. The two
  backlog items ruled by Adam in H4/H5 (kernel-field accessor API for
  `ProcessInstance`/`Fiber`/`ConcurrencyRecord`; the `utterance-engine`
  fuzz/examples crate-split) remain deferred, unchanged by this
  tranche. The `transition_from_tick_ops` "T4/T7" compatibility bridge
  noted above belongs to a different, untracked-by-this-plan
  initiative — flagged, not touched.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context on this session's work) verified every
  claim above directly against the live repo — read and re-derived the
  gate logic in all three new/extended scripts rather than trusting
  the receipt's prose, grepped for both deletions workspace-wide,
  spot-checked 5 of the 45 manifest entries against the filesystem,
  confirmed the CI step placement, re-ran every focused test and gate
  command independently and reproduced the exact reported numbers
  (12/12, 46/46, 45 targets, clean `cargo check`), diffed commit
  `717d3ef`'s file list against the receipt's own list (exact match,
  modulo the receipt reasonably omitting itself), and independently
  re-derived the H4→H6 dead-code correction from the pre-H6 file
  rather than accepting it asserted.
  - **One finding, disposed:** the receipt originally stated "16 call
    sites" for the `DslFrontend::lower` → `lower_plan` migration (three
    places: the narrative, the deletion description, and the
    consumer-migration summary). The reviewer recounted directly
    against `git show a81213f:.../frontend.rs` and the commit diff:
    the real count is **14** call sites (the migration itself,
    `cargo test -p bpmn-lite-compiler --lib dsl::frontend` at 12/12,
    was and remains correct — this was a receipt-text miscount, not a
    code defect). Corrected in place in this document; all three
    occurrences now read 14.
  - No other discrepancy found. Overall reviewer verdict: accept after
    this correction, no code or gate changes required.

- **STOP-gate decision: blocked — awaiting peer review of this
  receipt.**

Per Gate H6's own text, peer review must accept this receipt — which
must enumerate every retained public module, every intentional
public-API removal, every approved exception (there are none), test
moves, and all commands/results — before EOP-PLAN-CRATE-HYGIENE-001 is
closed. This is the plan's final tranche; acceptance of this receipt
closes the plan.
