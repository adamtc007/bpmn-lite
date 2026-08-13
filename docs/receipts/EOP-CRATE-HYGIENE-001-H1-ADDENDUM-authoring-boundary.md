# EOP-PLAN-CRATE-HYGIENE-001 — H1 addendum: close the `bpmn-lite-authoring` deviation

Addresses the "Known deviations" item flagged in
`EOP-CRATE-HYGIENE-001-H1-receipt.md`: `bpmn-lite-engine`'s own package
description locks "does NOT depend on ... authoring (bpmn-lite-authoring)",
but `src/tests.rs` still called `bpmn_lite_authoring::{parse_workflow_yaml,
compile_program_from_dto}` directly via a dev-dependency, contradicting that
boundary. Adam ruled this a real code smell to fix, not a permanently parked
deviation — this addendum closes it under the same H1 tranche discipline.

- **Scope delivered:** the 6 "Authoring Phase A" tests in
  `bpmn-lite-engine/src/tests.rs` (`t_auth_1_basic_sequence_yaml` through
  `t_auth_6_boundary_timer_yaml`) are exactly the multi-crate application
  pattern R3 assigns to `xtask/tests/`: they drive `bpmn-lite-authoring`'s
  YAML→DTO→IR→bytecode pipeline and feed the result into
  `bpmn-lite-engine`'s runtime. Moved verbatim (assertions unchanged) to
  `xtask/tests/authoring_engine_vertical.rs`, using only the already-public
  API of `bpmn-lite-engine` (`BpmnLiteEngine::{new, compile,
  store_compiled_program, start, ...}`, all `pub` in `engine.rs`),
  `bpmn-lite-store`, `bpmn-lite-types`, `bpmn-lite-vm`, `bpmn-lite-authoring`,
  and `bpmn-lite-compiler::GatewayDirection` — no production code changed,
  no new public surface added.
  - The file-local `FAR_FUTURE_TIMER_MS` constant (also defined separately
    in `bpmn-lite-engine/src/tests.rs` for its own remaining tests) was
    duplicated into the new file rather than shared — it's a trivial literal
    with no shared-ownership meaning, not worth a cross-crate export.
- **Files changed:**
  - `bpmn-lite-engine/src/tests.rs`: 546 lines removed (the "Authoring Phase
    A" section, lines 5021–5566 pre-edit). No other test in the file
    references this section (grep-confirmed: cross-references to
    `t_auth_6_boundary_timer_yaml` elsewhere in the file are doc-comment
    citations only, not code dependencies).
  - `bpmn-lite-engine/Cargo.toml`: `[dev-dependencies]` block (containing
    only `bpmn-lite-authoring`) removed entirely, along with its explanatory
    comment. The crate now has zero dev-dependencies and its own locked
    "does NOT depend on bpmn-lite-authoring" boundary is true of `src/`
    *and* `tests.rs` alike.
  - New: `xtask/tests/authoring_engine_vertical.rs` (568 lines, 6 tests).
- **Public API before/after (`cargo public-api -p bpmn-lite-engine -sss`):**
  no diff against the H0 baseline — confirmed directly (dev-dependency and
  test-code changes are not part of a crate's public API surface).
- **Focused tests:** `cargo test -p xtask --test authoring_engine_vertical`:
  6 passed, 0 failed.
- **Workspace checks:**
  - `cargo check --workspace --all-targets`: clean, exit 0. Same 2
    pre-existing unrelated `bpmn-lite-server-designer` warnings as every
    prior tranche — not introduced here.
  - `cargo test --workspace --lib --bins`: 1088 passed, 0 failed (H1's
    baseline was 1094; the 6-test delta is exactly the 6 tests moved out of
    `bpmn-lite-engine --lib` into xtask's separate `--test` target).
  - `cargo test -p xtask --tests`: 43 passed, 0 failed (H1's baseline was
    37; the 6-test delta reconciles exactly with the move).
- **Known deviations:** none remaining for this specific item.
  `bpmn-lite-authoring` remaining an unused dev-dependency of
  `bpmn-lite-store-postgres` (zero references, pre-existing, out of scope
  for H1) is unaffected by this addendum.
- **STOP-gate decision: blocked — awaiting peer review**, folded into the
  same H1 acceptance gate this addendum closes out.
