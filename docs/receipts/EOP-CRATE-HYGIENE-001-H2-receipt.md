# EOP-PLAN-CRATE-HYGIENE-001 — H2 receipt

Baseline revision: `89ae3e6` (H0). Prior tranche: H1 (`6052546`) + H1 addendum
(`496ddb5`). This tranche's revision: see `git log -1` on branch
`codex/bpmn-gameboard-refactor` at commit time.

- **Scope delivered:** H2 work items 1–4 in full.
  1. `bpmn-lite-store-postgres::test_lock` made fully private: `pub mod
     test_lock` → `mod test_lock`, `pub fn get_mutex` → `pub(crate) fn
     get_mutex`. It was already `#[cfg(test)]`-gated (so never appeared in
     the normal public-API surface); this closes the *intra-crate* leak —
     the module and its function are now visible only within
     `bpmn-lite-store-postgres`, matching its only 2 real callers
     (`pending_store.rs:274`, `store_postgres.rs:5101`).
  2. `utterance-engine::metrics` made private: `#[cfg(test)] pub mod
     metrics` → `#[cfg(test)] mod metrics`. Confirmed via workspace-wide
     grep that every item inside was already `pub(crate)` or private, and
     the module has zero callers anywhere — not even within
     utterance-engine's own other test files — so no test-local access
     needed preserving beyond the module's own inline `#[cfg(test)] mod
     tests`/`mod seed_corpus_baseline` submodules.
  3. `bpmn_lite_vm::compute_hash` retired. Confirmed via workspace-wide
     grep that `bpmn_lite_types::EffectId::content_hash(bytes: &[u8]) ->
     [u8; 32]` (`bpmn-lite-types/src/transition.rs:390-392`) is the
     pre-existing true domain contract — byte-for-byte the same blake3
     call already used throughout `bpmn-lite-types`/`bpmn-lite-store` to
     populate `domain_payload_hash`/`payload_hash` fields.
     `bpmn_lite_vm::compute_hash` was a separate, parallel
     reimplementation of the same one-line hash, not the source of truth.
     All 100 call sites across the workspace (grpc.rs's 2 real production
     callers — `start_process`/`signal` — 5 proof binaries, 3 engine
     integration tests, `bpmn-lite-engine/src/tests.rs`'s 68 sites, and 6
     xtask vertical test files' 22 sites) migrated to call
     `bpmn_lite_types::EffectId::content_hash` directly (`&str` args
     converted to `.as_bytes()`). `compute_hash` deleted from
     `bpmn-lite-vm/src/lib.rs`.
  4. `build_demo_plan`/`demo_initial_vars` moved from `bpmn-lite-engine`
     (`src/demo.rs`) to `bpmn-lite-server-runner` (`src/demo.rs`, `git mv`
     preserving history) — `bpmn-lite-server-runner/src/rest.rs` is
     confirmed the real, wired-in runtime consumer
     (`RunnerState::try_new()`, the `POST /bpmn/instances/start` handler);
     it now imports from `crate::demo` instead of `bpmn_lite_engine`.
     `bpmn-lite-server-runner/src/lib.rs` gains `pub mod demo;` — pub
     because `xtask/tests/demo_corpus_vertical.rs` is a second, legitimate
     cross-crate consumer (see next point), not a test-only escape hatch.

- **Files/packages changed:**
  - `bpmn-lite-store-postgres/src/lib.rs` — `test_lock` visibility
    tightened (no behaviour change).
  - `utterance-engine/src/lib.rs` — `metrics` module declaration
    de-pub'd.
  - `bpmn-lite-vm/src/lib.rs` — `compute_hash` deleted; doc comment
    updated to record why. `bpmn-lite-vm/Cargo.toml` — `blake3` dependency
    removed (now unused; the crate's only other module, `json_path`, never
    used it).
  - `bpmn-lite-server-runner/src/grpc.rs` (2 sites),
    `bpmn-lite-server-runner/src/bin/{ffi_proof,http_proof,load_harness,
    grpc_proof,heterogeneous_proof}.rs` (1 site each), `bpmn-lite-engine/
    tests/{send_task,differential_bpmn,correlation_content}.rs` (1 site
    each), `bpmn-lite-engine/src/tests.rs` (68 sites),
    `xtask/tests/{authoring_engine_vertical,runner_array_limits_vertical,
    ffi_vertical,store_postgres_engine_kernel_vertical,
    store_postgres_engine_fault_injection_vertical,
    runner_application}.rs` (22 sites total) — `compute_hash` calls
    migrated to `bpmn_lite_types::EffectId::content_hash`, dead
    `use bpmn_lite_vm::compute_hash;` imports removed.
  - `bpmn-lite-engine/Cargo.toml`, `bpmn-lite-server-runner/Cargo.toml`,
    `xtask/Cargo.toml` — `bpmn-lite-vm` dependency removed from all 3;
    confirmed via grep zero remaining `bpmn_lite_vm::` references in any
    of their real code after the migration above (`bpmn-lite-engine`'s
    entry was a real, non-dev `[dependencies]` line used only by its own
    `#[cfg(test)] mod tests` — itself a boundary smell the migration
    exposed, now fully gone).
  - `bpmn-lite-engine/src/lib.rs` — `mod demo;` / `pub use
    demo::{build_demo_plan, demo_initial_vars};` removed; stale doc
    comment ("wires ... `bpmn-lite-vm` (execute)") corrected to
    `bpmn-lite-kernel` (the crate's real executor dependency — confirmed
    via `engine.rs`'s own `use bpmn_lite_kernel::{apply as apply_kernel,
    ...}`; `bpmn-lite-vm` was never actually the interpreter this crate
    wires, only ever a test-only `compute_hash` caller).
  - `bpmn-lite-engine/src/demo.rs` → `bpmn-lite-server-runner/src/demo.rs`
    (`git mv`, content unchanged including its own 6 tests).
  - `bpmn-lite-server-runner/src/lib.rs` — `pub mod demo;` added.
    `bpmn-lite-server-runner/src/rest.rs` — import switched to
    `crate::demo::{build_demo_plan, demo_initial_vars}`.
  - `bpmn-lite-engine/src/tests.rs` — `corpus_sweep_demo_source_lowers_
    and_verifies` (the §10-demo half of the V5.5 corpus-sweep regression
    gate) removed; it depended on `build_demo_plan`, which no longer
    lives in this crate.
  - New: `xtask/tests/demo_corpus_vertical.rs` — the same test, moved
    verbatim (assertions unchanged), now calling
    `bpmn_lite_server_runner::demo::build_demo_plan()` +
    `bpmn_lite_compiler::Compiler::lower_dsl`. A genuine multi-crate
    application scenario per R3 (drives one capability crate's real
    output through another's verifier), not a test-only relocation of
    convenience.
  - `docs/generated/public-api-baselines/{bpmn-lite-engine,
    bpmn-lite-server-runner,bpmn-lite-vm}.txt` — updated to the new
    approved state (see diffs below); these are the intentional H2
    removals/additions, not drift.

- **Public API before/after (`cargo public-api -p <package> -sss`):**
  - `bpmn-lite-engine`: **2 removals** —
    `pub fn bpmn_lite_engine::build_demo_plan() -> ...`,
    `pub fn bpmn_lite_engine::demo_initial_vars(&str, &str) -> ...`.
  - `bpmn-lite-server-runner`: **3 additions** —
    `pub mod bpmn_lite_server_runner::demo`,
    `pub fn bpmn_lite_server_runner::demo::build_demo_plan() -> ...`,
    `pub fn bpmn_lite_server_runner::demo::demo_initial_vars(&str, &str)
    -> ...`. This is the intended migration destination for the 2
    removals above, not an unplanned addition.
  - `bpmn-lite-vm`: **1 removal** —
    `pub fn bpmn_lite_vm::compute_hash(&str) -> [u8; 32]`.
  - `bpmn-lite-store-postgres`: **no diff** (`test_lock` was already
    `#[cfg(test)]`-gated, so it was never part of the normal-build public
    surface `cargo public-api` inspects — the fix closed an intra-crate
    leak, not a cross-crate one).
  - `utterance-engine`: **no diff** (same reasoning — `metrics` was
    already `#[cfg(test)]`-gated).
  - All 5 diffs matched exactly what was planned before running the
    command — no surprise additions or removals on any touched crate.

- **Removed public items and migrated consumers:**
  - `bpmn_lite_vm::compute_hash` → every one of its 100 call sites
    migrated to `bpmn_lite_types::EffectId::content_hash` (list above);
    zero remaining references anywhere in the workspace (grep-confirmed).
  - `bpmn_lite_engine::{build_demo_plan, demo_initial_vars}` → both
    callers (`bpmn-lite-server-runner/src/rest.rs`'s
    `RunnerState::try_new()` and the `start_instance` handler) migrated
    to `crate::demo::{build_demo_plan, demo_initial_vars}` in the same
    crate the functions now live in.

- **Added public items and capability justification:**
  `bpmn-lite-server-runner::demo` (`build_demo_plan`, `demo_initial_vars`)
  — the direct relocation target of the 2 removals above, not new
  behaviour. Justified by 2 real callers: `rest.rs` (production/demo-mode
  runtime) and `xtask/tests/demo_corpus_vertical.rs` (compiler-verifier
  regression test), confirmed via grep before making the module `pub`.

- **Test classification changes:** `corpus_sweep_demo_source_lowers_and_
  verifies` reclassified from intra-crate (bpmn-lite-engine) to
  multi-crate application, moved to `xtask/tests/
  demo_corpus_vertical.rs` — it now drives `bpmn-lite-server-runner`'s
  demo-plan construction through `bpmn-lite-compiler`'s verifier, which
  is a 2-capability-crate contract, not a single-crate unit test.

- **Focused tests:**
  - `cargo test -p xtask --test demo_corpus_vertical -p
    bpmn-lite-server-runner --lib`: 9 + 1 + 9 = 19 passed, 0 failed
    (`demo.rs`'s own 6 moved tests + `rest.rs`'s + `event_fanout`'s
    pre-existing tests, all under `--lib`, plus the new xtask corpus-sweep
    test).
  - `cargo test -p bpmn-lite-store-postgres --lib`: 94 passed, 0 failed —
    confirms `test_lock`'s tightened visibility didn't break either of
    its 2 real callers.

- **Workspace checks:**
  - `cargo check --workspace --all-targets`: clean, exit 0. Same 2
    pre-existing unrelated `bpmn-lite-server-designer` warnings as every
    prior tranche.
  - `cargo test --workspace --lib --bins`: 1087 passed, 0 failed, 47/47
    binaries green (prior tranche total was 1088; the 1-test delta is
    exactly `corpus_sweep_demo_source_lowers_and_verifies` moving out of
    `--lib` coverage into xtask's separate `--test` target — the 6
    `demo.rs` tests that also moved crates are still counted, just now
    attributed to `bpmn-lite-server-runner` instead of
    `bpmn-lite-engine`, net zero on their own).
  - `cargo test -p xtask --tests`: 44 passed, 0 failed, 1 pre-existing
    `#[ignore]`d (prior tranche total was 43; +1 for the new
    `demo_corpus_vertical.rs`).

- **Known deviations or explicitly parked work:** none.

- **Blind peer-review findings and dispositions:** not yet run — this
  receipt is the input to that review, not its output.

- **STOP-gate decision: blocked — awaiting peer review.**

Per R8 and Gate H2's own text ("there is no `#[cfg(test)] pub` without an
accepted written exception, and no test/demo-only public engine or VM API
remains"), **H3 does not begin until this receipt is reviewed and
accepted.**
