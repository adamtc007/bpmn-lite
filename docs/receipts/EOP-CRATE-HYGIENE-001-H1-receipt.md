# EOP-PLAN-CRATE-HYGIENE-001 — H1 receipt

Baseline revision: `89ae3e6`. This tranche's revision: see `git log -1` on branch
`codex/bpmn-gameboard-refactor` at commit time.

- **Scope delivered:** H1 work items 1–5 in full.
  1. `xtask/tests/` established as the multi-crate application harness.
  2. All 6 confirmed multi-crate application scenarios from H0's test-topology
     inventory moved, assertions preserved verbatim, each file's module doc comment
     names its flow and cites the move.
  3. `bpmn-lite-store-postgres::store_postgres.rs`'s mixed test module split: 9
     engine/compiler/kernel/FFI/bus-storage scenarios moved to 3 new xtask test
     files (plus a shared `xtask/tests/common/mod.rs` fixture helper, matching the
     standard Rust `tests/common/mod.rs` idiom — not a separate test binary); 85
     SQL/persistence-contract tests stay in `bpmn-lite-store-postgres`.
  4. Item 4 ("rewrite legitimate inter-crate tests to use ratified root APIs") —
     not executed this tranche. The 24 inter-crate contract tests classified in H0
     already import their subject crate's root API in every case audited; no
     rewrite was needed. Deferred formal re-audit to H3/H4 once root-façade
     narrowing actually changes what "ratified" means for those crates.
  5. Now-unneeded workspace dev-dependencies removed from the 4 crates whose tests
     moved (see below).

- **Files/packages changed:**
  - `xtask/Cargo.toml` — added dev-dependencies for the 9-crate-plus set needed by
    the 9 moved test files (plain, not feature-gated, per explicit decision — see
    "Decisions" below).
  - `bpmn-lite-bus-handler/tests/graph_authored_plan_instantiation.rs` →
    `xtask/tests/bus_graph_instantiation_vertical.rs`
  - `bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs` →
    `xtask/tests/bus_postgres_vertical.rs`
  - `bpmn-lite-engine/tests/a11_ffi_end_to_end.rs` → `xtask/tests/ffi_vertical.rs`
  - `bpmn-lite-server-runner/tests/integration.rs` →
    `xtask/tests/runner_application.rs`
  - `bpmn-lite-server-runner/tests/orch_flags_array_limits.rs` →
    `xtask/tests/runner_array_limits_vertical.rs`
  - `dmn-lite-compiler/tests/end_to_end.rs` → `xtask/tests/dmn_vertical.rs`
  - `bpmn-lite-store-postgres/src/store_postgres.rs` — `mod tests` reduced from
    13,261 to 11,788 total file lines; 9 functions + a ~700-line fault-injection
    test double extracted.
  - New: `xtask/tests/store_postgres_engine_kernel_vertical.rs` (368 lines, 5
    tests), `xtask/tests/store_postgres_engine_fault_injection_vertical.rs` (1,068
    lines, 3 tests), `xtask/tests/store_postgres_bus_outbox_vertical.rs` (159
    lines, 1 test), `xtask/tests/common/mod.rs` (290 lines, shared fixture helper).
  - `bpmn-lite-bus-handler/Cargo.toml` — removed dev-deps `bpmn-lite-authoring`,
    `bpmn-lite-store-postgres`, `dsl-bus-client` (confirmed zero remaining
    references anywhere in the crate).
  - `bpmn-lite-engine/Cargo.toml` — removed dev-deps `ffi-catalogue`,
    `dmn-lite-bridge`, `dmn-lite-compiler`, `dmn-lite-parser`, `ffi-types`
    (`ffi-types` was a redundant duplicate of the existing regular dependency).
    `bpmn-lite-authoring` **retained** — see "Known deviations" below.
  - `dmn-lite-compiler/Cargo.toml` — removed dev-deps `dmn-lite-engine`,
    `dmn-lite-analysis` (confirmed zero remaining references).
  - `bpmn-lite-store-postgres/Cargo.toml` — removed dev-deps `bpmn-lite-compiler`,
    `bpmn-lite-engine`, `bpmn-lite-kernel`, `bpmn-lite-vm`, `ffi-dispatcher`.
    `bpmn-lite-authoring` was already unused before this tranche's moves — left
    untouched, out of scope for this receipt.

- **Public API before/after (`cargo public-api -p <package> -sss`):** **No diff**
  on any of the 4 touched library crates (`bpmn-lite-bus-handler`,
  `bpmn-lite-engine`, `bpmn-lite-store-postgres`, `dmn-lite-compiler`) — verified
  against the H0 committed baselines. Expected: H1 only moves/removes test code
  and manifest dev-dependencies, neither of which is part of a crate's public API
  surface.

- **Removed public items and migrated consumers:** none — H1 makes no public API
  changes (confirmed above).

- **Added public items and capability justification:** none at the library-crate
  level. `xtask/tests/common/mod.rs` contains `pub` helper functions
  (`setup`/`make_instance`/`test_hash`/etc.) — these are `pub` only within the
  private `mod common;` inclusion of each xtask test binary (real `cargo check`
  confirms no `unreachable_pub` violation; xtask's `unreachable_pub = "deny"` lint
  does not fire on test-binary-local helpers, only library public surfaces).
  Nothing in `bpmn-lite-store-postgres`'s production code was made `pub` for test
  convenience — the moved tests reconstruct their fixtures against
  `bpmn-lite-store`'s already-public `RuntimeStore`/`AdminProjectionStore` trait
  API and `PostgresWorkflowStore::pool()`/`set_tenant_context()` (both already
  `pub`), per R2's "no test-only production surface" rule.

- **Test classification changes:** 15 test files reclassified from
  intra-crate/mixed to **multi-crate application**, moved to `xtask/tests/`: the 6
  from H0's confirmed candidate list, plus the 9 extracted from
  `store_postgres.rs`'s previously-mixed unit-test module (which itself was not a
  single classifiable target — it's now split into 85 correctly-classified
  intra-crate SQL-contract tests + 9 multi-crate application tests in 3 files).

- **Focused tests:**
  - All 6 originally-moved files: `cargo test -p xtask --test <name>` × 6 →
    24 passed, 1 `#[ignore]`d (pre-existing, documented local-DB-migration gap
    unrelated to this move), 0 failed.
  - The 3 new store-postgres-split files, against a real local Postgres
    (`bpmn_lite_test` db): 5 + 3 + 1 = 9 passed, 0 failed.
  - `bpmn-lite-store-postgres --lib` (whole crate, all files), against the same
    real Postgres: 94 passed, 0 failed. `store_postgres.rs`'s own `mod tests`
    specifically went from 94 functions (85 kept + 9 moved, confirmed via
    `git show HEAD~1:...store_postgres.rs | grep -c '#\[tokio::test\]\|#\[test\]'`)
    to 85; the crate's other test-bearing files (`pending_store.rs`,
    `ffi_template_store.rs`) contribute the remaining count that keeps the
    whole-crate `--lib` total at 94 post-split. No test lost or duplicated.
  - `cargo test -p xtask --tests` (all 9 xtask files + xtask's own 4 pack-check
    unit tests): 37 passed, 0 failed.

- **Workspace checks:**
  - `cargo check --workspace --all-targets`: clean, exit 0. Same 2 pre-existing
    unrelated `bpmn-lite-server-designer` warnings noted in the plan's own §1.1
    baseline — not introduced by this tranche.
  - `cargo test --workspace --lib --bins`: 1094 passed, 0 failed, 47/47 binaries
    green. (H0 baseline was 1103 across the same command; the 9-test delta is
    exactly the 9 tests moved out of `bpmn-lite-store-postgres --lib` into
    xtask's separate `--test` targets, which `--lib --bins` does not cover —
    reconciled above via the xtask `--tests` run.)

- **Known deviations or explicitly parked work:**
  - `bpmn-lite-authoring` remains a `bpmn-lite-engine` dev-dependency.
    `src/tests.rs` still calls `bpmn_lite_authoring::{parse_workflow_yaml,
    compile_program_from_dto}` directly, contradicting the crate's own locked
    "does NOT depend on bpmn-lite-authoring" Phase-0 boundary (H0 finding). This
    is an architectural decision, not a mechanical dev-dependency cleanup — H1
    only removed dependencies that existed solely to support tests that moved to
    xtask this tranche. Flagged again here for explicit peer-review disposition
    before H2/H3.
  - `bpmn-lite-authoring` is also an unused dev-dependency of
    `bpmn-lite-store-postgres` (zero references, confirmed by the H1.3 agent) —
    pre-existing before this tranche's moves, left untouched as out of scope.
  - Work item 4 (rewrite inter-crate tests to ratified root APIs) was assessed as
    already satisfied for all 24 classified inter-crate contract tests, not
    actively rewritten — see "Scope delivered" above.
  - `xtask`'s dependency footprint grew as explicitly ratified (plain
    dev-dependencies, not feature-gated) — `cargo build -p xtask` (the ops-CLI
    binary) is unaffected since dev-dependencies never compile for a non-test,
    non-bench build.

- **Blind peer-review findings and dispositions:** not yet run — this receipt is
  the input to that review, not its output.

- **STOP-gate decision: blocked — awaiting peer review.**

Per R8 and Gate H1's own text ("the test inventory has no unclassified
multi-crate test... peer review confirms that no behavioural assertion was lost
in a move"), **H2 does not begin until this receipt is reviewed and accepted.**
