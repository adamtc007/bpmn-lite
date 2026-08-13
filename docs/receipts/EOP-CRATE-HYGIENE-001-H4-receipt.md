# EOP-PLAN-CRATE-HYGIENE-001 — H4 receipt

Baseline revision: `89ae3e6` (H0). Prior tranche: H3 (`30831f4`). This
tranche's revisions: `3c6a357` (H4.1), `20bf005` (H4.2, partial — see
"Deferred" below).

## H4.1 — Compiler DSL contract

- **Scope delivered:** trimmed `bpmn-lite-compiler::dsl`'s `mod.rs` from a
  15-submodule flatten (`pub use X::*` regardless of real use — H0's "broad
  implementation tree") to an explicit, evidence-backed capability surface.
  Every one of the ~65 previously re-exported symbols was individually
  grep-verified against the whole workspace (excluding the crate's own
  `src/`, but including its separate-package fuzz crate
  `bpmn-lite-compiler/fuzz` and `xtask` — both genuine cross-crate
  consumers) before any decision was made.
  - **Kept public** (real consumer, or structurally required by a kept
    item's own signature): `plan::*` (heaviest use — authoring, engine,
    server-designer, server-runner, bus-handler), `linter::{lint,
    BindingDecl, PlaceholderRegistry, StubPlaceholderRegistry}`,
    `manifest_registry::ManifestPlaceholderRegistry`,
    `ir_plan::{project_ir, IrPlanError}`,
    `closure::{validate_path_family, Diagnostic}`,
    `macros::{create_parallel_split_join, create_xor_split_join,
    CustomMacroConfig, MacroConfigList, XorBranchConfig}`, all of
    `pack_build::*` (xtask's own build tooling is the sole consumer),
    `parser::parse_workflow_str`, `refactor::{AstMutator, ToSexpr}`,
    `repeat::{repeat_n_times, RepeatNTimesError}`,
    `rpst::verify_sese_nesting`, `dag::{validate_dag, DagError}` (the
    fuzz target's own consumer), `frontend::{lower_plan, FrontendError}`,
    and 4 of `ast`'s 13 types (`JoinAst`, `JoinModeAst`, `NodeAst`,
    `WorkflowSource` — `bpmn-lite-authoring`'s real, direct consumers).
  - **Moved out of the re-export** (confirmed zero consumers anywhere,
    fuzz/xtask included): the other 9 `ast` node types, `frontend`'s
    `DslFrontend`/`WorkflowFrontend` (confirmed entirely dead even
    *internally* — `dead_code` warning, not deleted, same "flag don't
    delete" discipline as H3's `TransactionContext` finding), `linter`'s
    `SymbolResolution`, `parser`'s `parse_node_str`, `unroll`'s
    `unroll_loops`/`MAX_UNROLLED_NODES`, `macros`'s
    `create_bounded_retry_macro`. Each item's own visibility was
    downgraded from `pub` to `pub(super)` per the crate's own
    `unreachable_pub = "deny"` ratchet (already opted in) rather than
    left as an orphaned `pub` with no re-export path — the compiler's own
    lint caught every one of these and named the exact fix.
  - `compile()`'s internal call to `unroll_loops` was switched from
    relying on the (now-removed) `pub use` to bring it into scope, to a
    private `use unroll::unroll_loops;` import — no behaviour change.
- **A methodological correction mid-tranche**: an initial exclude-pattern
  bug (`grep -v "^\./bpmn-lite-compiler/"` against `grep -rl`'s actual
  no-`./`-prefixed output) silently no-op'd once, producing an apparently
  contradictory "0 vs 7 consumers" result for the same symbol on a manual
  re-check. Traced to the bug, fixed (`grep -v "^bpmn-lite-compiler/src/"`
  — excluding only the crate's own library source, not its fuzz target),
  and every number re-verified individually before use. Recorded here
  because it's exactly the kind of quiet-wrong-number failure this
  plan's "receipts or it isn't done" discipline exists to catch.
- **Public API before/after:** confirmed via `cargo public-api -p
  bpmn-lite-compiler -sss` — every removed item matched the
  zero-consumer list above; one nuance the diff surfaced and was
  verified safe: `create_parallel_split_join`/`create_xor_split_join`'s
  return type still names the now-unreachable `dsl::ast::SplitAst` in
  the diff output, but their only 2 real callers
  (`bpmn-lite-server-designer/src/rest.rs`) only ever destructure the
  returned tuple (`let (split, join) = create_xor_split_join(...)`) and
  never name `SplitAst` explicitly — confirmed by the workspace still
  compiling clean with `SplitAst` hidden. Same reasoning applies to
  `SymbolResolution` (return type of `PlaceholderRegistry::{resolve_decision,
  resolve_verb}`, whose 0 callers anywhere means this never surfaces).
- **Focused tests:** `cargo test -p bpmn-lite-compiler --lib`: 174 passed,
  0 failed. `bpmn-lite-compiler/fuzz` (standalone workspace) checks clean.
- **Workspace checks:** `cargo check --workspace --all-targets` clean
  (only the 2 new, confirmed-genuinely-dead `DslFrontend`/
  `WorkflowFrontend` warnings, plus the 2 pre-existing unrelated ones).
  `cargo test --workspace --lib --bins`: 47/47 binaries green (unchanged
  count — no tests moved). `cargo test -p xtask --tests`: 44/44
  (unchanged).

## H4.2 — BPMN type vocabulary (partial — see "Deferred")

- **Scope delivered:** the one confirmed, purely mechanical finding from
  H0's evidence: `bpmn_lite_types::ffi_bindings::resolve_binding_scalar`
  had zero external callers anywhere in the workspace (only used by the
  same file's own correlation-key resolution path) — `pub fn` → `pub(crate)
  fn`.
- **Investigated, left unchanged (2 items):**
  - `pub use uuid::Uuid` at the crate root — H0 flagged as a removal
    candidate ("zero cross-crate consumers... dependents import `uuid`
    directly"). A first grep for the literal fully-qualified path
    `bpmn_lite_types::Uuid` found 0 hits and the item was removed; `cargo
    check --workspace` immediately caught 2 real consumers
    (`bpmn-lite-kernel/src/lib.rs`, `bpmn-lite-types/benches/v2_perf.rs`)
    that reach `Uuid` through a grouped `use bpmn_lite_types::{...,
    Uuid, ...};` import — a pattern the fully-qualified-path grep
    structurally cannot see. Reverted before commit; this is the same
    class of near-miss H4.1's methodology note describes, caught by the
    same "verify via `cargo check`, not just grep" discipline.
  - `canonical`'s 4-item re-export
    (`CanonicalWriter`/`CanonicalReader`/`CanonicalDecodeError`/
    `CanonicalEncode`) — H0 flagged as "zero qualified consumers found...
    re-check before removing." Investigated: `CanonicalEncode` (the
    trait) and `CanonicalDecodeError` do have real external consumers
    (3 fuzz targets calling `T::from_canonical_bytes`/`to_canonical_bytes`,
    the trait's own convenience wrappers). `CanonicalWriter`/
    `CanonicalReader` have zero *direct* external references (nothing
    calls `canonical_encode`/`canonical_decode` directly, only the
    wrappers), but they are the parameter types of `CanonicalEncode`'s
    own trait methods — inseparable from the trait's public signature.
    Left as a coherent, untouched unit rather than partially trimmed,
    consistent with the plan's own instruction not to weaken canonical-
    encoding guarantees.
- **Public API before/after:** `cargo public-api -p bpmn-lite-types -sss`
  — exactly 1 removal (`resolve_binding_scalar`), matching the one real
  change.
- **Focused tests:** `cargo test -p bpmn-lite-types --lib`: 118 passed, 0
  failed. `bpmn-lite-types/fuzz` (standalone workspace) checks clean.
- **Workspace checks:** `cargo check --workspace --all-targets` clean
  (includes the crate's own `benches/` target, part of `--all-targets`).
  `cargo test --workspace --lib --bins`: 47/47 binaries green.

### Deferred — the R5 field-violation findings (not decided unilaterally)

H0's central H4.2 finding — public, invariant-bearing, constructor-less
fields on 3 core runtime types — was investigated but **not fixed this
tranche**, and is surfaced here rather than resolved by me alone:

- `ProcessInstance.counters: BTreeMap<u32, u32>` (H0: "bounded-loop
  invariant").
- `Fiber.control_stack: Vec<Handle>` (H0: "concurrency-table handle
  ordering") and `Fiber.loop_epoch: u32` (H0: "monotonicity").
- `ConcurrencyRecord.counters`/`rollback_domain_payload`/
  `rollback_domain_payload_hash`/`rollback_flags` (H0: "populated only
  under one opcode condition per its own doc comment — unenforced
  today").

**Why this is a materially different case from every field fix landed
in H2–H4 so far** (`DesignSessionRecord.events`, `TransactionContext.ops`
in H3): those had an existing accessor/constructor precedent already in
the same struct to route through (`visible_events()`,
`get_join_count()`), and their only real callers were app-layer code in
a handful of files. `ProcessInstance`/`Fiber`/`ConcurrencyRecord` have
**no existing accessor pattern at all** — `impl Fiber` has only a plain
`new()` constructor; every other mutation (`fiber.stack.push(...)`,
`fiber.control_stack.push(handle)`, `fiber.loop_epoch += 1`, and the
`ConcurrencyRecord` fields) happens via direct field access inside
**`bpmn-lite-kernel`'s dispatch loop** — a *different* crate, and
specifically the hottest, most correctness-critical instruction-dispatch
code in the whole workspace (per `bpmn-lite-vm`'s own doc comment
identifying this as "the interpreter... stack ops, wait/race/loop
semantics").

Closing this finding for real means: designing a new mutation API for
this state (what should `control_stack`'s push/pop contract actually be?
should `loop_epoch` expose only an `increment()` or something richer?),
then migrating every one of kernel's direct-field-access call sites to
it. That is an architectural decision about the core VM's data-access
pattern, not a mechanical visibility tightening — exactly the class of
"convenience field or speculative constructor" Gate H4's own text says
peer review should be rejecting, except here the mistake would run the
other way (inventing an API without the domain owner's design input).
Per this repo's "surface forks, don't decide them" rule, this is
recorded as an open finding, not resolved.

**`transition` module** (H0: "split candidate for H4.2 — 46 pub items,
1896 lines, largest single concentration in the crate... verify against
de-globbed imports before cutting") was also not attempted this
tranche — same-shaped work as H4.1's dsl audit, but not the tranche's
named central finding; deferred alongside the field-violation decision
rather than run partially.

## Combined workspace verification (both H4.1 and H4.2)

- `cargo check --workspace --all-targets`: clean, exit 0.
- `cargo test --workspace --lib --bins`: 47/47 binaries green.
- `cargo test -p xtask --tests`: 44/44 passed (unchanged from H3).
- `bpmn-lite-compiler/fuzz` and `bpmn-lite-types/fuzz` (both standalone
  workspaces): checked clean independently.

## STOP-gate decision: blocked — awaiting a ruling on the deferred R5
finding, then peer review of this receipt.

Per R8 and Gate H4's own text ("the compiler exposes a capability façade
rather than an implementation module tree... every retained public field
in core types has a documented data-contract reason. Peer review rejects
convenience fields and speculative constructors"), **H5 does not begin
until: (a) Adam rules on how to handle the deferred `ProcessInstance`/
`Fiber`/`ConcurrencyRecord` field findings (fix now with a real
kernel-touching migration, defer explicitly to H6's final inventory, or
rule them out-of-scope), and (b) this receipt is reviewed and
accepted.**
