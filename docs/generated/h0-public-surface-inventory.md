# H0 evidence — public-surface inventory (reviewer map)

Baseline revision: `89ae3e6`. Raw `cargo public-api -p <pkg> -sss` output for all 32
library packages: `docs/generated/public-api-baselines/*.txt` (machine-readable,
reproduced identically from a clean detached-HEAD worktree — see H0 receipt §4).

This document is the human-facing disposition map required by H0 work item 2, covering
the 7 priority crates from plan §1.2. Every disposition below was produced by an
independent read-only audit (grep-verified cross-crate consumer counts, not guesses) and
is a **recommendation for H2–H5 peer review**, not an executed change — H0 makes no
production edits.

## `[lints] workspace = true` opt-in audit (H0 work item 2 addendum)

**Opted in (26):** bpmn-lite-authoring, utterance-engine, bpmn-lite-compiler,
bpmn-lite-engine, bpmn-lite-kernel, bpmn-lite-server-runner, bpmn-lite-server-designer,
bpmn-lite-store, bpmn-lite-store-postgres, bpmn-lite-types, bpmn-lite-vm, ffi-catalogue,
ffi-dispatcher, ffi-types, bpmn-lite-analysis, bpmn-lite-ffi-grpc, bpmn-lite-bus-handler,
dmn-lite-bus-handler, dmn-lite-manifest-export, dmn-lite-server, dsl-bus-client,
dsl-bus-protocol, dsl-bus-server, dsl-bus-storage, dsl-manifest, xtask.

**NOT opted in (8) — unprotected by `unreachable_pub = "deny"`:** designer-graph,
dmn-lite-analysis, dmn-lite-bridge, dmn-lite-compiler, dmn-lite-engine, dmn-lite-parser,
dmn-lite-types, bpmn-lite-ffi-http.

Two of the plan's own 7 priority crates (**designer-graph**, **dmn-lite-types**) are in
the unprotected set. `designer-graph/Cargo.toml` has no `[lints]` section at all and is
still on `edition = "2021"`. **Recommend: H0 gate requires ratifying whether these 8
crates opt in before or during H1–H5**, since narrowing their surfaces during those
tranches would otherwise proceed without the ratchet catching regressions.

## bpmn-lite-types (504 pub items / 136 pub fields — counts confirmed near-exact)

Flat-facade design: 2 genuine `pub mod` (`integrity`, `session_stack`, both real
cross-crate contracts — retain), 2 correctly-hidden `pub(crate) mod`
(`integrity_rings`, `v2_verifier` — no action needed), remaining 8 source modules
flattened via `pub use *::*` at root.

- **Retain as-is**: `integrity`, `session_stack`, `artifact`, `concurrency`, `events`,
  `ffi_bindings` (except `resolve_binding_scalar` — no external caller, consider
  `pub(crate)`), `persistence`, `types`.
- **Re-check before removing** (glob-import consumers may be undercounted by
  qualified-path grep): `canonical` — zero qualified consumers found.
- **R5 violations found** (public, invariant-bearing, no constructor gate — contrast
  with the crate's own `RetryPolicy`/`ScopeFailureBudget` positive pattern):
  `ProcessInstance.counters` (bounded-loop invariant), `Fiber.control_stack`
  (concurrency-table handle ordering), `Fiber.loop_epoch` (monotonicity),
  `ConcurrencyRecord.counters`/`rollback_domain_payload*`/`rollback_flags` (populated
  only under one opcode condition per its own doc comment — unenforced today).
- **Candidate for removal**: `pub use uuid::Uuid` — zero cross-crate consumers
  (dependents import `uuid` directly).
- **Split candidate for H4.2**: `transition` module (46 pub items, 1896 lines) — largest
  single concentration in the crate; some items show zero *qualified-path* hits but
  crate is glob-imported everywhere, so verify against de-globbed imports before cutting.

`[lints] workspace = true`: **yes**.

## utterance-engine (361 pub items / ~178-183 pub fields — confirmed near-exact)

- **Confirmed R2 violation (plan's known finding)**: `#[cfg(test)] pub mod metrics` —
  zero cross-crate consumers, zero *intra-crate* consumers outside its own test
  submodules, and its own contents are already `pub(crate)` — the `pub` on the `mod`
  keyword is fully inert. Straightforward one-line fix in H2.
- **New findings — zero-consumer `pub mod`s beyond the known `metrics` case**:
  `fixtures`, `funnel` (`q9-capture`-gated), `pair` — none referenced anywhere in
  `bpmn-lite-server-designer` (the crate's sole documented external consumer), only
  used by this crate's own examples/tests/fuzz.
  - `pub use resolver_comparison::{...}` (11 items) and
    `pub use structured_choice::{...}` (8 items) — 19 combined root re-exports with
    **zero consumers anywhere in the compiled workspace**. Their only "consumer,"
    `scripts/fixtures/gameboard_api/facade_consumer.rs`, is not wired into any Cargo
    target, CI script, or build config — referenced only in receipt prose.
  - `pub use history::{MAX_HISTORY_ATTEMPTS, MAX_HISTORY_BYTES}` and
    `pub use legal_moves::MAX_ENUMERATION_CANDIDATES` — fuzz-only consumers.
- **Retain (real, wired consumers in `bpmn-lite-server-designer`)**: `board`,
  `bpmn_board`, `context`, `contract`, `corpus_schema` (schema types), `disposition`,
  `exact`, `policy`, `retrieval`, and the feature-gated `capture`/`trained_ranker`.
- **Re-litigate disposition**: `dev_capture` — real caller in `rest.rs`, but its own
  module doc says "Adam's own testing only," i.e. dev-tooling wearing a capability hat.
- Note: `build_demo_plan`/`demo_initial_vars` do **not** live in this crate (they're
  `bpmn-lite-engine`) — an earlier draft of this finding misattributed them here; the
  correct location is captured in the plan's §1.2 (see the corrected text there).

`[lints] workspace = true`: **yes**.

## bpmn-lite-compiler (171 pub items / 144 pub fields — fields exact, items ~162 by grep)

Root surface (`ir`, `lowering`, `parser`, `verifier`, `Compiler` facade) is clean and
consumer-backed — retain as-is.

`dsl` submodule is the H4.1 target: 15 private submodules, **zero `pub mod`**, but 15
`pub use` blocks at `dsl/mod.rs` flatten nearly everything onto `dsl::*` regardless of
the private module boundary. Concrete count: **~253 raw pub-marked lines**, of which:
- **9 of 15 submodules have zero external consumers** anywhere in the workspace —
  `ast` (13 types/38 fields), `dag`, `refactor`, `repeat`, `rpst`, `unroll`,
  `parser` (dsl-internal), most of `frontend`, most of `macros`/`linter`'s non-core
  surface. That's roughly **72% of dsl-tree pub items** with no real caller — the
  concrete evidence behind the plan's prose claim ("broad implementation tree").
- **Retain (real consumers)**: `plan` (heaviest-used — authoring, engine,
  server-designer, server-runner, bus-handler), `linter::{BindingDecl,
  StubPlaceholderRegistry}`, `manifest_registry::ManifestPlaceholderRegistry`,
  `ir_plan::project_ir`, `closure::validate_path_family` (+ its `Diagnostic` return
  type), `macros::MacroConfigList`, root `compile`/`CompileError`.
- **xtask-only**: `pack_build::*` — single consumer, candidate for a narrower
  xtask-facing façade rather than full crate-public surface.
- Positive-pattern note: `dsl::plan::WorkflowExecutionPlan`'s own 8 fields are already
  `pub(crate)` specifically to protect its `mathematically_proved`/`unsafe_breeches`
  invariant — this is the R5 model H4.1 should extend to the rest of the tree.

`[lints] workspace = true`: **yes**.

## dmn-lite-types (137 pub items / 113 pub fields — confirmed exact)

Cleanest of the 7 priority crates. All 13 submodules private, only flat `pub use`
re-exports at root. Every re-export group has real cross-crate consumers except 4
`#[doc(hidden)]` types (`BkmId`, `BindingId`, `PathId`, `AggregateOpKind`) explicitly
reserved for a future profile per their own doc comments — intentionally speculative,
not accidental. `VerifiedDecision`/`TypedInputContext`/`Catalogue` already model
R5-correct field/accessor splits (private invariant-bearing state, `pub(crate)`
constructors gated behind a `verify()`/`resolve_*` API). **No disposition changes
recommended** beyond noting the 4 reserved-but-unused types for the record, and the
crate's own missing `[lints]` opt-in (see audit above).

`[lints] workspace = true`: **no**.

## bpmn-lite-authoring (51 pub items / 64 pub fields — confirmed)

Genuinely the positive pattern the plan credits it as: zero `pub mod`, 10 curated
`pub use` re-exports, all traffic through the flat root vocabulary.
`TemplateStore`/`WorkflowTemplate`/`MemoryTemplateStore`/`PostgresTemplateStore` is the
strongest real R5-compliant example found in this audit (trait-object port, direct
construction of a plain data-contract struct, all live in
`bpmn-lite-server-designer`).

**Caveat, not a violation**: roughly a third of the re-exported vocabulary has zero
current external caller — `compile_and_publish` (the crate's own headline YAML-fronted
publish entry, superseded in practice by `compile_and_publish_from_dto`),
`import_zeebe_bpmn` (fully built/tested/fuzzed, no production caller),
`SlotKind`/`ParameterSlot`, `TemplateMeta`, `ErrorEdge`, `RaceArm`/`RaceArmKind`. These
read as forward-declared/architecturally-complete surface rather than
fixture/demo leakage — **recommend peer review explicitly rule whether
"capability-complete, not-yet-wired" is an accepted category distinct from the
test/demo violations found elsewhere**, rather than mechanically narrowing them.

Minor facade gap: `PublishResult.lint_diagnostics: Vec<LintDiagnostic>` exposes a field
whose element type is never re-exported at root — external code can read it but not
name the type.

Also found (not this crate's issue, but relevant to its disposition): `bpmn-lite-
bus-handler` and `bpmn-lite-store-postgres` both carry a `bpmn-lite-authoring`
dev-dependency with zero references in either crate's `src/` — likely stale, verify
against their `tests/` before pruning.

`[lints] workspace = true`: **yes**.

## bpmn-lite-store (51 pub items / 37 pub fields — confirmed)

Three parallel access paths to the same items (module-qualified, root-glob, and for
`pending` an explicit named list) — real callers use all three inconsistently
(`bpmn-lite-server-designer/src/rest.rs` imports `DesignSessionRecord` via both
`bpmn_lite_store::DesignSessionRecord` and `bpmn_lite_store::store::DesignSessionRecord`
in the same file). Recommend H3 pick one canonical façade per module.

- **`store_memory` is not actually a module-wide exposure problem**: exactly one public
  item (`MemoryStore`), zero public fields (fully encapsulated behind `RwLock`).
- **Two genuine R5 violations**: `store::DesignSessionRecord.events: Vec<...>` and
  `store::TransactionContext.ops: Vec<...>` are public mutable fields sitting next to
  inherent methods (`visible_events`, `get_join_count`) whose entire purpose is to
  preserve invariants over those same collections — external code can currently bypass
  both invariants via direct field mutation.
- **Dead root re-exports**: `PendingInvocation`, `MemoryPendingInvocationStore`,
  `PendingInsertOutcome` at crate root — zero cross-crate consumers anywhere (only
  `pending::PendingInvocationStore` the trait is used cross-crate).
- **Scheduled-removal bridge, not stable contract**: `transition_from_tick_ops`,
  `TickOperation`, `AlreadyConsumedError` — already `#[doc(hidden)]`, self-documented as
  a temporary T4→T7 compatibility shim, but still cross-crate load-bearing in
  `bpmn-lite-server-runner` and `bpmn-lite-store-postgres` today. Track for removal on
  the stated T7 milestone rather than folding into the stable-contract classification.

`[lints] workspace = true`: **yes**.

## designer-graph (49 pub items / 62 pub fields — confirmed exact)

6 `pub mod`, not the 5 the crate's own module doc comment claims. The doc comment
(`lib.rs`, "pub-scope audit, 2026-07-29") names exactly `board_candidate`, `ops`,
`positional`, `productions`, `schema` as "deliberate, addressed-by-name API surface" —
**`runbook` is an undocumented 6th public module**, not covered by that audit at all.

- **`runbook` (`render_operation`, `render_runbook`)**: exactly one call site in the
  entire workspace (`bpmn-lite-server-designer/src/rest.rs:3013`). Looks like an
  implementation detail promoted to `pub` opportunistically rather than an audited
  capability. **Recommend**: fold into `bpmn-lite-server-designer` as a private helper,
  or explicitly add it to the ratified justification list if kept.
- **Retain (real, heavy cross-crate use)**: `board_candidate`, `ops`, `positional`,
  `productions`, `schema` — all consumed as production code (not just tests) by both
  `bpmn-lite-server-designer` and `utterance-engine`.
- **No R5 violations found** — `DesignerDag`'s guard-budget/retry-policy fields were
  already tightened to `pub(crate)` + accessor methods in a prior gate (self-corrected).
  All other public fields across `BoardCandidate`, `StagedCandidate`, `RegionBranch`,
  the 4 `productions::*Bindings` structs, `Provenance`, `NodeKey` are plain immutable
  data-contract fields.

`[lints] workspace = true`: **no** (also `edition = "2021"`, no `[lints]` section at
all — confirmed by direct read of `designer-graph/Cargo.toml`).

## Summary — public modules/re-export groups with zero non-test external consumers

(H0 required-evidence item: "a list of every currently public module with zero
non-test external consumers")

| Crate | Item | Notes |
| --- | --- | --- |
| bpmn-lite-store-postgres | `test_lock` (`#[cfg(test)] pub mod`) | Confirmed zero cross-crate consumers workspace-wide. |
| utterance-engine | `metrics` (`#[cfg(test)] pub mod`) | Confirmed zero cross-crate + zero intra-crate (outside own submodules) consumers; contents already `pub(crate)`. |
| utterance-engine | `fixtures`, `funnel`, `pair` | Zero consumers in `bpmn-lite-server-designer`; example/fuzz-only. |
| utterance-engine | `resolver_comparison::*` (11 items), `structured_choice::*` (8 items) | Zero consumers anywhere in the compiled workspace; only "consumer" is an uncompiled fixture file. |
| bpmn-lite-types | `pub use uuid::Uuid` | Zero cross-crate consumers. |
| bpmn-lite-compiler::dsl | `ast`, `dag`, `refactor`, `repeat`, `rpst`, `unroll`, dsl-internal `parser` | Zero external consumers; ~72% of dsl-tree pub items. |
| bpmn-lite-store | `pending::{PendingInvocation, MemoryPendingInvocationStore, PendingInsertOutcome}` (root re-exports) | Zero cross-crate consumers at the root alias path. |
| designer-graph | `runbook` | One call site total; undocumented against the crate's own 5-module audit. |
| bpmn-lite-authoring | `import_zeebe_bpmn`, `compile_and_publish`, `SlotKind`, `ParameterSlot` | Test/fuzz-only or architecturally-complete-but-unwired — distinct category, see crate section above. |

This list is prioritization evidence for H1/H2 peer review, not a removal order — per
the plan's own §1.2 framing.
