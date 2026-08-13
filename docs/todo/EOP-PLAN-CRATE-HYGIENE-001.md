# EOP-PLAN-CRATE-HYGIENE-001 — Capability boundaries, public API, and test topology

| Field | Value |
| --- | --- |
| Status | **DRAFT — pre-execution peer review required** |
| Baseline reviewed | `efda5ad` (2026-08-13) |
| Evidence | Public-surface and test-topology review, 2026-08-13 |
| Execution model | One tranche per change set; STOP and peer review at every tranche close |
| Does not authorise | Production refactors, API removals, or test moves before this plan is ratified |

---

## 0. Objective

Make every public declaration a deliberately supported **capability contract** and
make test placement show which contract is being verified. A test must never be
the sole reason an implementation detail, fixture, demo helper, or application
composition path remains public.

The desired end state is:

1. Capability crates publish only the types, traits, and functions required by
   their consumers, through a small crate-root façade.
2. Application composition (servers, demos, database+transport wiring, and
   vertical slices) resides in the application harness, not in a capability
   crate's unit test suite or incidental public modules.
3. Tests are visibly classified as **intra-crate**, **inter-crate contract**, or
   **multi-crate application** tests.
4. Every public API removal or addition is measured with `cargo public-api` and
   explained in its tranche receipt.

### Important terminology ruling

In a Rust multi-crate workspace, `pub` cannot literally exist only in the final
application crate: a shared type, trait, wire contract, or capability entry
point must be `pub` to cross a crate boundary. The rule is therefore:

> `pub` denotes a supported capability contract, never an implementation,
> fixture, test-enablement convenience, demo, or accidental module path.

Application-level code may compose those contracts, but it must not force an
unrelated capability crate to publish its internals merely to make the
composition convenient.

---

## 1. Evidence and starting position

### 1.1 Existing control

The workspace already denies `unreachable_pub` in the root lint policy
(`[workspace.lints.rust] unreachable_pub = "deny"`, root `Cargo.toml:50-52`).
This is an opt-in ratchet, not automatic workspace-wide: a crate is only
covered once its own `Cargo.toml` sets `[lints] workspace = true`. It
prevents unreachable public declarations, but it does **not** determine
whether a reachable item belongs to the supported capability contract. This
plan adds the missing contract and test-topology review.

`cargo-public-api` is also **already wired into CI**, not a tool this plan
introduces: `.github/workflows/production-gates.yml` installs it and runs
`scripts/check-semantic-gameboard-boundaries.py`, which diffs `cargo
public-api -p <package> -sss` against a committed baseline
(`scripts/baselines/semantic-gameboard-public-api-v1.json`) — but scoped
narrowly today (utterance-engine, bpmn-lite-server-designer). H0/H6 extend
this existing gate workspace-wide; they do not stand up a new one, and must
not duplicate or conflict with it.

`cargo check --workspace --all-targets` passed at the reviewed baseline. Two
unrelated warnings exist in `bpmn-lite-server-designer`; they are not silently
absorbed into this work.

### 1.2 Surface pressure points

These counts are the reviewed bare-`pub` inventory, including public fields;
they are prioritisation evidence, not an instruction to make all items private.

| Crate | Public items | Public fields | Review position |
| --- | ---: | ---: | --- |
| `bpmn-lite-types` | 504 | 136 | High-value vocabulary; needs contract-vs-construction audit. |
| `utterance-engine` | 361 | 183 | Broadest module exposure; developer/example tooling is mixed with capability API. |
| `bpmn-lite-compiler` | 171 | 144 | `dsl` is exposed as a broad implementation tree. |
| `dmn-lite-types` | 137 | 113 | Vocabulary audit after the BPMN core pattern is proven. |
| `bpmn-lite-authoring` | 51 | 64 | Mostly good root façade; preserve it as the positive pattern. |
| `bpmn-lite-store` | 51 | 37 | Port types are valid; module-wide exposure needs review. |
| `designer-graph` | 49 | 62 | Named subdomains need an explicit capability justification. |

The review found three immediately suspicious, test/demo-only public paths:

- `bpmn_lite_store_postgres::test_lock` is `#[cfg(test)] pub mod`, yet has only
  in-crate users.
- `utterance_engine::metrics` is `#[cfg(test)] pub mod`, yet has no external
  users.
- `bpmn_lite_vm::compute_hash` is the crate's sole public API. The crate
  documents itself as "compatibility" (execution moved to `bpmn-lite-kernel`),
  not test-only — that "test-only" label belongs only to the unrelated
  private `json_path` submodule. Most call sites are tests and proof
  binaries, but **`bpmn-lite-server-runner/src/grpc.rs` has two live,
  non-test production callers**: `start_process` (line 552) and `signal`
  (line 621) both call `compute_hash` to validate client-supplied payload
  hashes in the real gRPC service. Any disposition in H2 must migrate this
  caller in the same tranche, not just delete the public item.

The engine's `build_demo_plan` and `demo_initial_vars` are public and **do**
have an identified external consumer: `bpmn-lite-server-runner/src/rest.rs`
imports both — `build_demo_plan()` in `RunnerState::try_new()` (line 58) and
`demo_initial_vars(...)` inside the live `POST /bpmn/instances/start` handler
(line 273), wired into that server's real `Router`. `rest.rs` self-documents
as "demo-mode only... For production process queries use the gRPC surface",
so the underlying judgment (this belongs in an application/demo harness, not
the engine capability crate) still stands — but H2 must plan the migration of
this real caller, not treat the helpers as unreferenced.

### 1.3 Test topology at baseline

There are 40 Cargo integration-test targets. Their external compilation is
useful: it is the correct way to validate a crate's supported public surface.
Their location and dependencies are not yet consistently meaningful.

Known multi-crate/application candidates include:

| Current location | Why it is not a single capability test | Proposed destination |
| --- | --- | --- |
| `bpmn-lite-engine/tests/a11_ffi_end_to_end.rs` | Engine + store + VM helper + DMN bridge/compiler/parser + FFI catalogue/dispatcher/types. | `xtask/tests/ffi_vertical.rs` |
| `bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs` | Postgres migration/store + engine + bus handler. | `xtask/tests/bus_postgres_vertical.rs` |
| `dmn-lite-compiler/tests/end_to_end.rs` | Explicit parser → compiler → engine vertical slice. | `xtask/tests/dmn_vertical.rs` |
| `bpmn-lite-store-postgres/src/store_postgres.rs` test module | Contains engine construction, compilation, kernel replay, FFI dispatcher and bus-storage scenarios alongside SQL-store tests. | Retain SQL-store contract tests; extract vertical scenarios to `xtask/tests/`. |
| `bpmn-lite-server-runner/tests/*` | Server application wiring with engine, stores, FFI owners and transport. | `xtask/tests/runner_application.rs`, unless a test is narrowed to a runner-owned HTTP/gRPC contract. |

This table is a review queue, not permission to move every integration test.
Tests that require a directly adjacent public type to construct a valid input
may remain **inter-crate contract tests**. The deciding question is whether the
test proves the subject crate's contract or proves a composed application flow.

---

## 2. Non-negotiable rules

**R1 — Capabilities, not modules.** Prefer a named crate-root function, trait,
or type over `pub mod`. A public module requires a written justification: it is
an intentional named capability, it has non-test consumers, and a module path
is part of the compatibility contract.

**R2 — No test-only production surface.** `#[cfg(test)] pub` requires an
exception approved in the tranche receipt. The expected form is private
`#[cfg(test)] mod`; integration tests must not need test helpers exported from
the production library.

**R3 — Test classification is a boundary decision.** Every moved or new test
declares exactly one class:

| Class | Home | Allowed dependencies | Assertion target |
| --- | --- | --- | --- |
| Intra-crate | `src/**` under `#[cfg(test)]` | The subject crate and ordinary test libraries; no workspace sibling used solely for test setup. | Private algorithms and local invariants. |
| Inter-crate contract | `<capability-crate>/tests/` | Subject crate plus the minimum direct public contract needed to construct input or observe output. | The subject crate's documented root API. |
| Multi-crate application | `xtask/tests/` | Explicit application dependency set, including stores, transport, servers and adapters. | A named vertical user/operator flow. |

**R4 — Tests use supported paths.** Inter-crate and application tests import
crate-root API unless the module path is itself a ratified capability. A test
does not bless a private field or a convenience re-export as public API.

**R5 — No speculative public field.** Public fields are only for stable data
contracts where direct construction and pattern matching are intended. Stateful
or invariant-bearing values use constructors, accessors, and validated builders.

**R6 — No opportunistic architecture changes.** This plan does not merge
crates, change runtime semantics, alter persistence schemas, or redesign wire
formats. Discovery of a required such change is a STOP condition.

**R7 — Public API evidence is mandatory.** Each tranche records the before/after
output of `cargo public-api -p <package> -sss`; any unexpected addition or
surviving item is a STOP condition.

**R8 — Peer review is a gate.** No tranche begins until the preceding tranche's
receipt, public-API diff, focused tests, workspace check, and authorship-blind
peer review have been accepted.

---

## 3. Decisions required before execution

Peer review must ratify or amend these decisions. If any remains open, only
Tranche H0 may execute.

1. **Application-test home — proposed:** `xtask/tests/` is the sole home for
   multi-crate application tests. `xtask` is explicitly an application harness,
   not a shared capability crate.

   **Fork to rule on:** `xtask` exists today but is narrow ops/build tooling
   (docker smoke/stress, fuzz orchestration, DSL pack build/check) depending
   only on `bpmn-lite-bus-handler`, `bpmn-lite-compiler`, `dsl-manifest`, and
   has no `tests/` directory. Housing the five H1 vertical scenarios there
   requires adding `bpmn-lite-engine`, `bpmn-lite-store*`, the FFI crates, and
   the DMN crates as new dev-dependencies — a real expansion of what is today
   a deliberately narrow ops CLI's build surface. Peer review must explicitly
   accept this build-surface growth, or direct that those dev-dependencies be
   scoped behind a feature/workspace split, before H1 begins.
2. **Compatibility policy — proposed:** this is an internal workspace cleanup;
   intentional public-API removals are permitted tranche-by-tranche, with every
   known consumer migrated in the same tranche. No semantic-version promise is
   implied until an external-consumer policy is separately approved.
3. **Public-module policy — proposed:** keep a public module only for a genuine
   named protocol/domain (for example generated protobuf or a stable wire
   namespace). All other capability crates use private modules and explicit
   crate-root re-exports.
4. **Core value policy — proposed:** `bpmn-lite-types` and `dmn-lite-types` may
   expose stable data vocabulary, but not unchecked construction or mutable
   representation merely because sibling crates currently use it.
5. **Scope order — proposed:** test topology first, narrow low-risk test/demo
   exposure second, then high-cardinality API facades. This prevents tests from
   blocking each visibility reduction.

---

## 4. Tranche map

```text
H0  Baseline and contract map        no production changes; peer-review evidence
H1  Test topology and app harness    classify tests; create xtask vertical home
H2  Test/demo escape hatches         remove P0 public test/demo-only paths
H3  Application and port façades     server/store/FFI module boundaries
H4  Compiler and core vocabulary     DSL façade; types contract and field audit
H5  Utterance and designer surface   dev/example tooling extraction; named subdomains
H6  Ratchet and final receipt        CI inventory/diff policy; closure review
```

H0 is mandatory. H1 precedes H2. H3, H4, and H5 are independently reviewable
after H2, but must not run concurrently when they touch the same consumer or
public-API baseline. H6 follows all accepted surface changes.

---

## H0 — Baseline and capability-contract map

**Tier:** CAREFUL. **Production changes:** forbidden.

### Work

1. Record a committed `cargo public-api` baseline for every library package.
   Store outputs in a reviewable, generated location with package name and
   baseline revision; do not hand-edit them.
2. Inventory every public module, root re-export, and public field in the
   priority crates. For each, record: owning capability, real consumer(s),
   whether it is a stable contract, and proposed disposition: retain, narrow,
   relocate, or remove. Include every crate that has **not** opted into
   `[lints] workspace = true` — `unreachable_pub = "deny"` only applies to
   crates that opt in, so un-opted crates are currently unprotected by the
   ratchet this plan leans on.
3. Classify all 40 integration-test targets and all unit-test modules under R3.
   Flag every workspace `dev-dependency` used only to assemble an application
   scenario.
4. Confirm the exact public API baseline command used in CI and demonstrate it
   can run from a clean checkout.

### Required evidence

- A machine-readable public API inventory plus a concise reviewer-facing map.
- A test inventory with subject crate, classification, dependencies, and target
  contract/flow.
- A list of every currently public module with zero non-test external consumers.
- No tracked code or manifest changes outside H0 evidence artifacts.

### Gate H0

Peer review ratifies the decisions in §3, resolves any disputed consumer, and
approves the H1 migration list. No visibility is reduced yet.

---

## H1 — Test topology and application harness

**Tier:** CAREFUL.

### Work

1. Establish `xtask/tests/` as the multi-crate application harness, with test
   files named by user-visible flow rather than by a component implementation.
   Its Cargo dependencies are explicitly test/application composition
   dependencies, never imported by a capability crate's library code.
2. Move the accepted H0 vertical scenarios, preserving their assertions and
   naming each flow in a module-level doc comment.
3. Split mixed Postgres store tests: SQL and persistence-contract tests remain
   in `bpmn-lite-store-postgres`; scenarios requiring engine/compiler/kernel,
   FFI dispatcher, or bus storage move to the application harness.
4. Keep legitimate inter-crate tests in their subject crate, but rewrite them
   to use ratified root APIs. Add a short comment only where the adjacent type
   is necessary to construct an input contract.
5. Remove now-unneeded workspace `dev-dependencies` from capability crates.

### Required tests

- Each moved vertical test passes from `xtask`.
- Each retained contract test fails to compile if its subject crate's intended
  root contract is removed, rather than relying on a private/module-accidental
  path.
- Focused old and new target runs pass; then
  `cargo check --workspace --all-targets` passes.

### Gate H1

The test inventory has no unclassified multi-crate test. Capability-crate test
dependencies no longer include application composition solely for setup. Peer
review confirms that no behavioural assertion was lost in a move.

---

## H2 — Test-only and demo escape hatches

**Tier:** CAREFUL.

### Work

1. Make `bpmn-lite-store-postgres::test_lock` private under `#[cfg(test)]`.
2. Make `utterance-engine::metrics` private under `#[cfg(test)]`; retain only
   test-local access required by its internal tests.
3. Resolve `bpmn-lite-vm::compute_hash` according to the ratified owner:
   move it to the true domain contract if payload hashing is public behaviour,
   or move its callers to application/test support and retire the public VM
   compatibility surface. **This has a real production caller** —
   `bpmn-lite-server-runner/src/grpc.rs`'s live `start_process` and `signal`
   RPCs (lines 552, 621) — which must be migrated to the ratified owner in
   this tranche, not left calling a removed/relocated item.
4. Move engine demo-plan construction and demo-variable helpers to `xtask` or a
   dedicated demo binary. **`bpmn-lite-server-runner/src/rest.rs` is a real,
   wired-in consumer** (`RunnerState::try_new()` and the
   `POST /bpmn/instances/start` handler) — migrate it to the new owner in the
   same tranche; do not treat these helpers as unreferenced.

### Required tests

- Intra-crate tests for the test lock and metrics continue to run without a
  public test API.
- Every proof/demo harness continues to produce the same hash/plan behaviour
  through its new owner.
- `cargo public-api` shows exactly the expected removals and no additions.

### Gate H2

There is no `#[cfg(test)] pub` without an accepted written exception, and no
test/demo-only public engine or VM API remains. Peer review accepts each API
removal and its migrated caller list.

---

## H3 — Application and port façades

**Tier:** CAREFUL.

### Work

1. Review `bpmn-lite-server-runner` and `bpmn-lite-server-designer` module
   exports. Keep only an intentional server construction/transport contract;
   make REST handlers, event fanout, DTOs, and proposal mechanics application
   internals unless an approved external consumer requires them.
2. Review `bpmn-lite-store`'s `pending`, `store`, and `store_memory` modules.
   Retain port traits and deliberately constructible test/memory adapters at the
   root; remove module-wide access where it is not a supported namespace.
3. Review `ffi-types::wire` and generated-protobuf module exposure separately.
   A protocol namespace may remain public, but implementation/adaptor modules
   may not be made public by analogy.
4. Update inter-crate consumers to use narrow root façades or a ratified named
   protocol path. Do not use re-export aliases just to preserve every old path.

### Required tests

- Consumer compilation proves each retained façade is sufficient.
- HTTP/gRPC contract tests run through the retained server entry point, not by
  calling handler-private functions.
- `cargo public-api` identifies only planned removals; server application tests
  run from `xtask` where they involve multiple capabilities.

### Gate H3

No server/application module is public solely because sibling binaries or
tests need it. Every retained public module has a peer-reviewed consumer and
capability statement.

---

## H4 — Compiler and core vocabulary contracts

**Tier:** CAREFUL; split into separate commits for H4.1 and H4.2.

### H4.1 — Compiler DSL contract

Replace the broad `bpmn_lite_compiler::dsl` implementation tree with an
explicit DSL/planning capability surface. Retain only the parser/compiler entry
points and stable plan/value types genuinely consumed by authoring, engine,
designer, and the application harness. Move parsing, linting, AST machinery,
pack building, refactoring and construction details behind private modules
unless their own capability is ratified.

### H4.2 — BPMN type vocabulary

Audit `bpmn-lite-types` by capability: immutable IDs/value vocabulary,
serialised wire/persistence records, validated artifacts, transition commands,
and internal construction/verification state. Replace public mutable fields on
invariant-bearing types with minimum constructors/accessors/builders. Do not
weaken canonical encoding, verifier, integrity, or persistence guarantees.

`dmn-lite-types` follows the same pattern only after H4.2 establishes a
reviewed method; it is not silently included in the first core-vocabulary edit.

### Required tests

- Compile-only consumer tests for each retained public compiler and type entry
  point.
- Negative compilation/API checks where practical: private construction paths
  must be unavailable outside their crate.
- Existing canonical-byte, verifier, persistence, and transition-invariant
  suites pass unchanged or with an explicit, reviewed replacement assertion.
- Per-package and workspace `cargo public-api` diffs are recorded.

### Gate H4

The compiler exposes a capability façade rather than an implementation module
tree. Every retained public field in core types has a documented data-contract
reason. Peer review rejects convenience fields and speculative constructors.

---

## H5 — Utterance and designer surface

**Tier:** CAREFUL.

### Work

1. Separate `utterance-engine`'s production capability API from fixtures,
   corpus generation, capture/reporting, training utilities, benchmarking, and
   examples. Examples that currently force library modules public move to
   `xtask` or a dedicated application binary.
2. Retain a public utterance module only where it is a named, stable capability
   with a real application consumer. Flatten or narrow it to a crate-root
   façade where named module access is not contractual.
3. Apply the same consumer-and-capability test to `designer-graph`'s six public
   subdomains. Preserve a public named domain only where its namespace carries
   meaning beyond code organisation.
4. Do not alter Q9/capture feature gating as a side effect; moving a helper is
   not authorisation to change capture policy.

### Required tests

- Existing examples/tools continue as application-harness commands or tests.
- The designer server compiles and runs its retained public capabilities without
  importing unapproved utterance/designer implementation modules.
- Feature-gated build matrix passes for default, `q9-capture`, and any affected
  trained-ranker/candle feature combination.

### Gate H5

Developer tooling no longer defines the production crate API. Every remaining
public utterance/designer module has a named capability, supported consumer,
and crate-surface test.

---

## H6 — Enforcement, final inventory, and close receipt

**Tier:** CAREFUL.

### Work

1. Extend the existing `cargo-public-api` gate (already installed and run in
   `.github/workflows/production-gates.yml` via
   `scripts/check-semantic-gameboard-boundaries.py` against
   `scripts/baselines/semantic-gameboard-public-api-v1.json`, currently scoped
   to utterance-engine and bpmn-lite-server-designer) to cover every library
   package's approved baseline — do not stand up a second, conflicting gate.
   Preserve the existing `unreachable_pub = "deny"` ratchet; do not replace it.
2. Add a lightweight source policy check that rejects new `#[cfg(test)] pub`
   unless listed in a reviewed exception file (expected empty after H2).
3. Add a test-topology manifest/check: every integration test target carries a
   classification, and `xtask/tests` is the only multi-crate application-test
   home.
4. Write the final inventory and a removal/migration table. Remove temporary
   compatibility shims unless peer review explicitly retained one with an expiry
   and owner.

### Required tests

- Full `cargo check --workspace --all-targets`.
- Relevant focused test suites plus the full workspace test command agreed at
  H0 (database-backed suites may use their documented CI service).
- Public API baseline/diff check passes from a clean checkout.
- Topology check reports no unclassified integration target and no unapproved
  test-only public item.

### Gate H6

Peer review accepts the final receipt. The receipt must enumerate every retained
public module, every intentional public-API removal, every approved exception
(normally none), test moves, and all commands/results. Only then is the plan
closed.

---

## 5. Tranche receipt template

Each tranche closes with this exact minimum record:

```markdown
## H<id> receipt

- Scope delivered:
- Files/packages changed:
- Public API before/after (`cargo public-api`):
- Removed public items and migrated consumers:
- Added public items and capability justification: none / list
- Test classification changes:
- Focused tests:
- Workspace checks:
- Known deviations or explicitly parked work:
- Blind peer-review findings and dispositions:
- STOP-gate decision: accepted / blocked
```

---

## 6. Stop conditions

Stop immediately and return to peer review if any tranche finds:

- an external consumer outside this workspace or a compatibility promise not
  represented in the reviewed baseline;
- a required schema, wire-format, persistence, or runtime-semantics change;
- a test whose purpose cannot be classified without changing its assertion;
- a public item whose consumer is ambiguous or whose capability owner is
  contested;
- a public-API diff containing an unplanned addition or surviving path;
- a move that would make test support itself a new shared production API.

No tranche resolves these by retaining `pub` "for now". It records the evidence,
states the decision required, and waits for review.

---

## 7. Pre-execution peer-review checklist

- [ ] Ratify the terminology ruling: capability contracts may be public;
      implementation and test-enablement may not.
- [ ] Ratify `xtask/tests` as the multi-crate application-test harness.
- [ ] Approve the H0 public-API baseline artifact format and location.
- [ ] Confirm the H1 candidate moves, including the Postgres-store split.
- [ ] Decide whether server libraries retain a small external construction API
      or become binary/application internals.
- [ ] Decide the public payload-hash owner before H2 begins.
- [ ] Confirm H4's core-type field audit is an intentional compatibility change,
      not a mechanical visibility sweep.
- [ ] Confirm default and feature-gated CI commands for H5.
- [ ] Confirm that each tranche requires a separate STOP-gate approval.

**Status:** no execution approved. Begin only with H0 after the checklist is
ratified.
