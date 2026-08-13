# EOP-PLAN-CRATE-HYGIENE-001 — H3 receipt

Baseline revision: `89ae3e6` (H0). Prior tranche: H2 (`9a65092`). This
tranche's revision: see `git log -1` on branch `codex/bpmn-gameboard-refactor`
at commit time.

- **Scope delivered:** H3 work items 1–4, scoped down from initial
  assumptions by 3 independent research passes (one per crate family) run
  before any edit — findings below.

  1. **`bpmn-lite-server-runner` and `bpmn-lite-server-designer` module
     exports** (work item 1): reviewed exhaustively.
     - `bpmn-lite-server-designer` needed **no changes** — already exactly
       what H3 wants: `mod proposal` is private (proposal mechanics were
       already fully application-internal, unreachable outside the
       crate), `pub mod rest` exposes exactly 2 externally-usable items
       (`DesignerState::try_new_from_env`, `designer_router`), and both are
       consumed only by the crate's own `[[bin]]` target
       (`bin/bpmn-lite-demo-designer.rs`) — a same-package binary, which
       (unlike a test) technically *requires* `pub` to link at all
       (`pub(crate)` does not cross the lib↔bin compilation-unit boundary
       in Cargo). This is precisely the plan's "intentional server
       construction/transport contract," not a leak.
     - `bpmn-lite-server-runner`: same lib↔bin-target reasoning applies to
       `rest::{RunnerState, runner_router}` (needed by
       `bin/bpmn-lite-demo-runner.rs`) and `grpc::{BpmnLiteService,
       RequestLimits, ServerMetrics, proto}` (needed by `main.rs` and, for
       several items, `fuzz/fuzz_targets/wire_decode.rs` — a genuine
       separate cross-crate consumer) — all confirmed real, not test-only.
       `event_fanout::EventFanout` confirmed needed by
       `xtask/tests/runner_array_limits_vertical.rs`, a genuine cross-crate
       test consumer. **One genuine finding**: `demo::demo_initial_vars`
       (moved here in H2) had zero callers outside `rest.rs` itself
       (unlike `build_demo_plan`, which `xtask/tests/
       demo_corpus_vertical.rs` genuinely needs) — tightened `pub fn` →
       `pub(crate) fn`.
     - `grpc::BpmnLiteService`'s 10 public fields (no constructor) were
       reviewed against R5: both construction sites (`main.rs`,
       `xtask/tests/runner_array_limits_vertical.rs`) build it via a
       direct, complete struct literal — this is R5's own explicit
       carve-out ("public fields only for stable data contracts with
       intended direct construction"), not a violation. No invariant
       exists between its fields for a constructor to protect. Reviewed
       and retained as-is; not an opportunistic redesign into a
       builder/constructor pattern (R6).
  2. **`bpmn-lite-store`'s `pending`, `store`, `store_memory` modules**
     (work item 2): closed the 2 genuine R5 field violations H0 already
     flagged, plus the dead root re-export H0 flagged:
     - `store::DesignSessionRecord.events` (public `Vec`, sitting next to
       `visible_events`/`graph_edit_payloads_as_of`, whose entire purpose
       is to preserve the G6.1 undo-jump-chain invariant over that
       collection): field changed to `pub(crate)` (same-crate
       `store_memory.rs` still appends directly — plain private would not
       have sufficed, since Rust field privacy scopes to the *defining
       module and its descendants*, not the whole crate, and
       `store_memory` is a sibling module, not a descendant, of `store`).
       Added `pub fn events(&self) -> &[DesignSessionEvent]` for the 2
       real cross-crate readers (`bpmn-lite-server-designer/src/rest.rs`,
       5 read sites; `bpmn-lite-store-postgres/src/store_postgres.rs`, 6
       read sites in its own test module) and `pub fn new(...)` for the 1
       real cross-crate constructor
       (`bpmn-lite-store-postgres/src/store_postgres.rs:4563`, replaying a
       session's DB rows into the aggregate — struct-literal construction
       from a different crate is impossible once a field is non-public,
       regardless of `pub(crate)` vs private, since that visibility never
       reaches a different crate).
     - `store::TransactionContext.ops` (public `Vec`, sitting next to
       `add_op`/`get_join_count`): field changed to private. **Additional
       finding not in H0's evidence**: `TransactionContext` itself has
       **zero callers anywhere in the workspace** (grep-confirmed — not
       even in `bpmn-lite-store`'s own tests) — genuinely dead code, not
       just an under-encapsulated live type. Field visibility tightened
       (safe, zero blast radius) but the struct itself was **not
       deleted** — dead-code removal is a bigger call than this tranche's
       "module export review" scope covers cleanly; flagged here for the
       H6 final inventory to rule on explicitly.
     - Dead root re-export (H0 finding, confirmed): `pub use pending::{
       InsertOutcome as PendingInsertOutcome, MemoryPendingInvocationStore,
       PendingInvocation, PendingInvocationStore};` at crate root removed
       entirely. Grep-confirmed every real cross-crate caller of all 4
       items already used the module-qualified `bpmn_lite_store::
       pending::*` path exclusively (`bpmn-lite-store-postgres/src/
       pending_store.rs:7`, `store_postgres.rs:5060`,
       `bpmn-lite-server-runner/src/bus_runtime.rs:25`) — zero callers via
       the flat root path anywhere. `pub mod pending;` (unchanged) is now
       the one canonical façade for this module, closing the "3 parallel
       access paths" H0 flagged for `pending` specifically.
     - **`store`/`store_memory`'s own module-qualified-vs-root-glob
       inconsistency** (H0's other finding, e.g. `DesignSessionRecord`
       reachable as both `bpmn_lite_store::DesignSessionRecord` and
       `bpmn_lite_store::store::DesignSessionRecord`): reviewed, **left
       as-is**. Unlike `pending`, both paths for `store`/`store_memory`
       items have real, live callers today; forcing a single canonical
       import path would mean rewriting import statements across
       `bpmn-lite-server-designer`, `bpmn-lite-store-postgres`, and
       others for a style/consistency concern, not a capability leak —
       both paths already resolve to the same supported public items. R6
       bars opportunistic architecture/style rewrites; this is noted, not
       forced, in this tranche.
     - `store_memory` itself: confirmed (per H0) already minimal — one
       public item (`MemoryStore`), zero public fields. No change needed.
  3. **`ffi-types::wire` and generated-protobuf exposure** (work item 3):
     reviewed, **no changes**. `ffi-types` has no `build.rs`/protobuf
     codegen at all. `wire` is the *only* `pub mod` in the crate; every
     other module (`canonical`, `idempotency`, `owner`, `record`,
     `schema`, `snapshot`, `template`) is already private with individual
     `pub use` re-exports at crate root. `wire`'s 3 items (`FfiCall`,
     `FfiResult`, `FfiIncidentClass`) are the genuine FFI dispatch-boundary
     wire contract (A2 §7), consumed by 7 real cross-crate callers, all
     via the crate-root re-export. No "implementation/adaptor module
     public by analogy" case exists in this crate — the plan's item-3
     concern doesn't apply here.
  4. **Update inter-crate consumers to narrow façades** (work item 4):
     done as part of items 1–2 above — every construction/read site that
     needed to change (5 files: `bpmn-lite-store/src/store.rs`,
     `bpmn-lite-store-postgres/src/store_postgres.rs`,
     `bpmn-lite-server-designer/src/rest.rs`,
     `bpmn-lite-store/src/lib.rs`, `bpmn-lite-server-runner/src/demo.rs`)
     now uses the accessor/constructor or the one remaining canonical
     path. No re-export aliases were added to preserve any old path.

- **Files/packages changed:**
  - `bpmn-lite-store/src/lib.rs` — dead `pending` root re-export block
    removed (comment explains why).
  - `bpmn-lite-store/src/store.rs` — `DesignSessionRecord.events` →
    `pub(crate)`, `DesignSessionRecord::{new, events}` added;
    `TransactionContext.ops` → private (struct confirmed dead, not
    removed — flagged for H6).
  - `bpmn-lite-store-postgres/src/store_postgres.rs` — 1 construction
    site (`DesignSessionRecord { .. }` → `DesignSessionRecord::new(..)`)
    and 6 test-module read sites (`.events` → `.events()`) updated.
  - `bpmn-lite-server-designer/src/rest.rs` — 5 read sites (`.events` →
    `.events()`) updated.
  - `bpmn-lite-server-runner/src/demo.rs` — `demo_initial_vars`: `pub fn`
    → `pub(crate) fn`.
  - `docs/generated/public-api-baselines/{bpmn-lite-store,
    bpmn-lite-server-runner}.txt` — updated to the new approved state.

- **Public API before/after (`cargo public-api -p <package> -sss`):**
  - `bpmn-lite-store`: **removals** — `DesignSessionRecord.events` field,
    `TransactionContext.ops` field, `PendingInsertOutcome` (enum + both
    variants), `MemoryPendingInvocationStore` (struct + all 5 trait-impl
    methods), `PendingInvocation` (struct + all 11 fields/methods),
    `PendingInvocationStore` (trait + all 6 methods) at the root path
    (all 4 remain reachable via `bpmn_lite_store::pending::*`, unchanged
    there — confirmed via the same diff showing no change to the
    `pending` module's own contents). **Additions** —
    `DesignSessionRecord::{new, events}`.
  - `bpmn-lite-store-postgres`: **no diff** (construction/read-site fixes
    only; no public item changed there).
  - `bpmn-lite-server-runner`: **1 removal** —
    `demo::demo_initial_vars`.
  - `bpmn-lite-server-designer`: **no diff** (confirmed already correct;
    zero edits made to this crate).
  - All diffs matched exactly what was planned before running the
    command.

- **Removed public items and migrated consumers:**
  - `DesignSessionRecord.events`/`TransactionContext.ops` fields → all 12
    real call sites (11 reads across `rest.rs`/`store_postgres.rs`, 1
    construction in `store_postgres.rs`) migrated to the new
    accessor/constructor.
  - `bpmn_lite_store::{PendingInsertOutcome, MemoryPendingInvocationStore,
    PendingInvocation, PendingInvocationStore}` root re-export → zero
    consumers existed via that path (grep-confirmed); nothing to migrate.
  - `bpmn_lite_server_runner::demo::demo_initial_vars` → its 1 caller
    (`rest.rs`, same crate) needed no code change, only the `pub(crate)`
    downgrade.

- **Added public items and capability justification:**
  `DesignSessionRecord::new`/`DesignSessionRecord::events` — direct
  replacement for the removed public field, justified by the 1 real
  cross-crate constructor call and 11 real cross-crate read call sites
  identified above.

- **Test classification changes:** none — no test moved crates this
  tranche.

- **Focused tests:**
  - `cargo test -p bpmn-lite-store --lib`: 46 passed, 0 failed.
  - `cargo test -p bpmn-lite-store-postgres --lib` (real Postgres): 94
    passed, 0 failed — including the 5 restart/replay tests that read
    `.events()` directly.
  - `cargo test -p bpmn-lite-server-designer --lib`: 91 passed, 0
    failed — including the 5 rest.rs call sites that read `.events()`.
  - `cargo test -p xtask --tests`: 44 passed, 0 failed (unchanged from
    H2) — `runner_array_limits_vertical.rs` and `runner_application.rs`
    are the HTTP/gRPC contract tests running through the retained
    `BpmnLiteService`/`grpc::proto` entry points per H3's "Required
    tests."

- **Workspace checks:**
  - `cargo check --workspace --all-targets`: clean, exit 0. Same 2
    pre-existing unrelated `bpmn-lite-server-designer` warnings as every
    prior tranche.
  - `cargo test --workspace --lib --bins`: 47/47 binaries green (count
    unchanged from H2 — no tests moved or added this tranche).

- **Known deviations or explicitly parked work:**
  - `TransactionContext` is confirmed entirely dead code (zero callers
    anywhere in the workspace) — field visibility tightened for R5
    hygiene, but the struct itself was not removed; flagged for H6's
    final inventory to explicitly rule on deletion.
  - `store`/`store_memory`'s module-qualified-vs-root-glob import-path
    inconsistency (real items reachable both ways) reviewed and
    deliberately left alone — not a capability leak, and forcing one path
    would mean rewriting many live call sites for a style concern alone,
    which R6 bars as an opportunistic change absent a real boundary
    problem.

- **Blind peer-review findings and dispositions:** not yet run — this
  receipt is the input to that review, not its output.

- **STOP-gate decision: blocked — awaiting peer review.**

Per R8 and Gate H3's own text ("no server/application module is public
solely because sibling binaries or tests need it. Every retained public
module has a peer-reviewed consumer and capability statement"), **H4 does
not begin until this receipt is reviewed and accepted.**
