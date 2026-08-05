# Zed implementation plan: standalone DSL, SemOS, embedding, and Sage boundaries

**Coordinating repository:** `/Users/adamtc007/dev/bpmn-lite`
**Shared source workspace:** `/Users/adamtc007/dev/dsl`
**Host application:** `/Users/adamtc007/Developer/ob-poc`
**Reviewed:** 5 August 2026
**Amended:** 5 August 2026 — factual corrections and structural fixes from the verification review against dsl@edded43, bpmn-lite@745b4ea, ob-poc@d76d8be9. Open decisions live in §20 (fork register).
**Purpose:** remove `ob-poc` application coupling from the Rust crates shared with `bpmn-lite`, establish independently testable and releasable crate boundaries, and cut both applications over without changing their business behaviour.

## 1. Instructions to the Zed implementation session

Execute this plan one phase at a time. Do not perform the work as one cross-repository rewrite.

At the start of every phase:

1. Read this plan completely.
2. Read `AGENTS.md` and repository-local instructions in every repository that will be touched.
3. Record the current branch, exact HEAD, worktree status, Rust toolchain, and relevant Cargo dependency revisions for all three repositories.
4. Treat every pre-existing modification and untracked file as user-owned. Do not format, stage, commit, move, or revert unrelated work.
5. Check for other worktrees and concurrent branches before selecting files to change.
6. State the phase, expected files, tests, compatibility risks, and commits before editing.
7. Add or update tests alongside production changes.
8. Run focused checks first and repository-wide checks only after focused checks pass.
9. Stop at the phase gate and report the exact commands and outcomes before proceeding.

If a required source repository has moved, locate it from Cargo metadata and Git dependency sources rather than assuming a path. Never replace an immutable Git dependency with a moving branch. Temporary local path overrides may be used during development, but they must not be committed in consumer repositories.

This document is a programme charter, not a full-block execution plan. Before dispatching any phase to a full-block implementation session, author a phase-specific blueprint containing the complete target function/module skeletons and explicit "Invariants & Absolute Boundaries" per the repository planning contract. Do not hand this document directly to a generator.

**Execution order (ruled 2026-08-05, F1):** 0 → 1 → 3 → 4 → 6 (forensic ruling only) → 2 → 5 → 7 → 8 → 9. Phases 3 (contracts extraction, a pure code move) and 4 (embedder dependency surgery) do not depend on Phase 2's pack-schema unification, which is the long pole; do not gate them behind it.

## 2. Objective and completion boundary

The completed system must have three explicit ownership layers:

```text
host-neutral shared Rust crates
          ^                         ^
          |                         |
ob-poc YAML packs + adapters   BPMN YAML packs + adapters
          ^                         ^
          |                         |
      ob-poc app                 bpmn-lite app
```

The shared workspace provides mechanisms, typed YAML schemas, pack loading and compilation, contracts, validation, canonical encoding, deterministic policy machinery, model inference, and transport-neutral REPL types. Application and domain semantics are configuration: verbs, nouns, argument rules, DAGs, candidate phrases, applicability, effects, roles, policy mappings, and pack topology are declared in versioned YAML and compiled into immutable semantic artifacts. Shared Rust code interprets those artifacts; it does not encode a built-in application profile.

Application Rust code is limited to technical integration that cannot be configuration: process composition, persistence adapters, transport handlers, model/runtime setup, and concrete capability execution behind stable IDs. Those adapters must not duplicate semantic rules already represented by a pack.

A fourth dependency edge exists today and is outside this three-layer model: `ob-poc` consumes 13 `bpmn-lite` crates (git tag `v0.2.0`, plus `bpmn-lite-server-runner` by rev), including the confusingly named `dsl-bus-*` and `dsl-manifest` crates, which belong to `bpmn-lite`, not the shared DSL workspace. This app→app edge must be recorded in the Phase 0 ledger and either explicitly justified as technical integration or scheduled for removal; it must not remain undeclared.

The implementation is complete only when:

- the shared crates compile, test, lint, document, and package without either application checkout;
- no production source in the shared crates imports an `ob-poc` or BPMN application crate;
- no shared production API hard-codes CBU, KYC, mandate, tollgate, client-group, MIC/BIC pricing, or application role semantics;
- the generic embedding crate has no SQLx, pgvector, PostgreSQL, or host-schema dependency;
- `bpmn-lite` depends only on the narrow shared crates it uses;
- `ob-poc` owns its versioned YAML pack sources, database adapters, and technical host integration—not a parallel Rust implementation of its semantic policy;
- BPMN owns its versioned YAML pack sources and technical integration—not hard-coded candidate or verb policy;
- the public shared API lets an application load, validate, compile, activate, inspect, and execute against packs without importing private modules or adding domain-specific branches to the engine;
- Sage/REPL protocol types are shared only if transport- and domain-neutral;
- dependency revisions are immutable and reproducible;
- both applications pass their compatibility and deployment gates;
- rollback revisions and a carry-over ledger are recorded.

## 3. Non-negotiable invariants

### I-1 — Dependency direction

Shared crates never depend on `ob-poc`, `bpmn-lite`, their database schemas, or their server implementations. Host adapters depend inward on shared contracts.

### I-2 — Behaviour preservation

Extraction must not change candidate ordering, scores, dispositions, slot binding, authorisation, or existing host command semantics unless a separately approved compatibility change is documented.

### I-3 — Stable persistent identities

Changes to UUID namespaces, canonical hashes, board hashes, or evidence hashes require a versioned migration. Never silently change the bytes used by an existing schema version. (Note: there is no distinct workbook hash today — `ProposalWorkbook` carries `board_hash` and `evidence_record_hash` by reference; do not invent one as part of a "compatibility" move.)

### I-4 — No optional-feature theatre

Disabling a feature must actually remove its dependencies and modules from the resolved graph. `default-features = false` is not sufficient when SQLx or pgvector remains unconditional.

### I-5 — Typed library boundaries

Reusable public library APIs return domain-specific error types. `anyhow` may be used by binaries, tooling, and top-level application adapters but not as an undifferentiated public contract.

### I-6 — Safe core

Pure contract, model, parser, and policy crates use `#![forbid(unsafe_code)]`. Any unavoidable unsafe code must be isolated in a narrowly scoped crate with a documented safety argument and dedicated tests.

### I-7 — Deterministic canonical encoding

Persistent hashes use explicit, versioned tags and canonical field encodings. Rust `Debug` output is never a persistent wire or hash format.

### I-8 — Reproducible release

Consumers use a published crate version or an exact Git revision. Tags, crate versions, lockfiles, source revisions, and release notes agree.

### I-9 — No hidden deployment expansion

This remediation must not introduce new databases, services, model downloads, runtime network dependencies, or schema migrations unless the relevant phase explicitly approves them.

### I-10 — Configuration is the domain source of truth

Application and domain semantics live in versioned YAML pack sources and their compiled artifacts. Shared or host Rust code must not contain a second authoritative copy of verbs, DAG edges, phrase evidence, slot rules, role mappings, or domain policy.

### I-11 — Complete public capability API

Applications use supported public APIs to load, validate, compile, register, activate, inspect, and evaluate packs. A missing public interface is repaired in the shared crate; it is not worked around by reaching into private modules, copying engine logic, or adding a host-specific branch.

### I-12 — Typed configuration, not stringly runtime behaviour

YAML is a source representation, not an unvalidated runtime object graph. Deserialize with schema versions, deny unknown fields where compatibility permits, validate identifiers and cross-references, compile into typed immutable structures, and execute only admitted artifacts. Arbitrary YAML/JSON values, SQL fragments, Rust type names, and executable snippets are not extension mechanisms.

## 4. Target source ownership

### 4.1 Shared workspace

Keep host-neutral code in `/Users/adamtc007/dev/dsl`. A future repository rename to `semantic-platform` may be considered after the boundary work; it is not required for this implementation.

The intended crate layout is:

```text
crates/
  dsl-model/                    # host-neutral AST/value/model types
  dsl-syntax/                   # parser, printer, canonical syntax
  semantic-pack/                # typed YAML schema, loader, validator, compiler
  sem-os-types/                 # identifiers and primitive SemOS contracts
  sem-os-core/                  # ports and host-neutral orchestration
  semantic-decision-contracts/  # boards, evidence, disposition, workbook
  sem-os-policy/                # generic policy algorithms over compiled packs
  semantic-embedder/            # tokenizer/model/embedder only
  repl-contracts/               # optional transport-neutral Sage/REPL protocol
```

Existing crate names may be retained where a rename would add risk. The capability boundary is mandatory; the spelling is not.

The actual workspace today contains exactly seven crates: `dsl_types`, `dsl-core`, `sem_os_types`, `sem_os_core`, `sem_os_ontology`, `sem_os_policy`, `dsl-integration-tests`. Every name in the target layout above is new; the Phase 0 ledger must map each real crate to its target destination explicitly rather than treating the layout as partially existing.

Known defect in the shared workspace itself: the repo root contains an untracked, gitignored `config` symlink pointing into `/Users/adamtc007/Developer/ob-poc/rust/config`, relied on by 59 `#[ignore]`d tests ("requires ob-poc config/"). This is a physical host coupling of the shared workspace and is in scope: Phase 0 records it; Phases 2/5 eliminate it (host fixtures become versioned pack fixtures compiled through the public API).

### 4.2 `ob-poc`

Keep domain semantics as YAML sources and technical host code under `/Users/adamtc007/Developer/ob-poc`:

```text
config/semantic-packs/          # verbs, DAGs, phrases, slots, role/policy mappings
rust/crates/
  ob-poc-capability-adapters/   # implementations selected by stable capability ID
  ob-poc-semantic-matcher/      # host orchestration around the shared embedder
  ob-poc-semantic-postgres/     # repositories and schema-qualified SQL
  ob-poc-sage-adapter/          # HTTP/UI and session composition
```

The YAML packs own CBU/KYC/trading/custody/billing vocabulary, host roles, verb definitions, DAGs, applicability, evidence, and semantic policy. Rust adapters own schema-qualified SQL, client-group lookup mechanics, feedback repositories, concrete capability execution, and HTTP/UI composition. Rust adapters refer to stable IDs declared by packs but do not redefine pack semantics.

`config/semantic-packs/` does not exist today. The real sources are under `rust/config/`: `packs/` (13 pack YAMLs), `verbs/` (~150 verb YAMLs), and `sem_os_seeds/` (~157 files: `dag_taxonomies/`, `domain_packs/`, `constellation_families/`, `constellation_maps/`, etc.). `dsl-source/verbs/*.dsl` is a **generated** mirror of `config/verbs/*.yaml` (via `src/bin/verb_to_dsl.rs`) — it is output, not source, and must not be treated as a second authority. Reconcile these real trees into the target layout rather than creating a parallel new directory.

### 4.3 `bpmn-lite`

Keep BPMN pack sources and hosting in `/Users/adamtc007/dev/bpmn-lite`:

```text
manifests/                            # existing verb manifests (bpmn-v1.0.0.yaml, dag/closure files)
dsl-manifest/                         # existing manifest loader crate
utterance-engine/src/bpmn_pack.rs     # today: the hard-coded semantic pack (to be replaced by YAML)
utterance-engine/src/bpmn_board.rs
bpmn-lite-server-designer/src/rest.rs
bpmn-lite-server-designer/src/proposal.rs
```

Corrections to the original draft: `bpmn-lite-server-designer/src/sage_adapter.rs` does not exist (the designer has only `rest.rs`, `proposal.rs`, `lib.rs`), and **no BPMN YAML semantic pack exists anywhere today** — `bpmn_pack.rs` builds the candidate slices from hard-coded `&'static str` tuples. The BPMN leg of Phase 2.5 is therefore *authorship of the first BPMN YAML pack plus a loader cutover*, not migration of an existing directory. The existing `manifests/` + `dsl-manifest` verb-manifest layer is a separate, real artifact class and must be reconciled with (not duplicated by) the semantic pack schema.

The BPMN YAML pack owns BPMN vocabulary and semantic decisions. The BPMN Rust layer loads/compiles that pack, constructs decisions through the shared API, binds admitted stable capability IDs to BPMN operations, orchestrates proposals, and provides routes/UI integration.

## 5. Phase 0 — Freeze the baseline and write the dependency ledger

### Work

Create a dated receipt in each affected repository or one coordinating receipt in `bpmn-lite/docs/todo` containing:

- repository path, branch, HEAD, upstream, and dirty files;
- all worktrees and concurrent branches;
- `rustc`, Cargo, and rust-toolchain versions;
- `cargo metadata --locked` dependency edges for the relevant crates;
- current Git revisions used by both consumers;
- current crate versions and repository tags;
- current default and optional features;
- current build, test, formatting, lint, and documentation results;
- current runtime artifacts or images that include the crates.

Record the exact public APIs used by each application. At minimum trace:

- every `sem_os_policy`, `sem_os_ontology`, `sem_os_core`, and `sem_os_types` import in `bpmn-lite`;
- every `ob-semantic-matcher` import in `bpmn-lite`;
- every `dsl-sage` or `ob-poc-sage` import, if any;
- the corresponding uses in `ob-poc`;
- persisted board, evidence, workbook, candidate, or model-bundle records.

The ledger must additionally record these already-verified facts (2026-08-05):

- **Three-way pin skew:** ob-poc pins the six dsl crates by mutable git **tag** `v0.1.5`; bpmn-lite pins by rev `fa51217` (= tag v0.1.6); dsl `main` is at `edded43`, now tagged `v0.1.7` (F4 executed 2026-08-05, closing CO-01). Tag pins violate the settled "exact pins (hash), never floors" decision and Phase 7's own rule; Phase 7 converts both consumers to exact-rev pins at the shared release candidate.
- **The dsl `config` symlink into ob-poc** (see §4.1).
- **The ob-poc → bpmn-lite crate edge** (13 crates at tag v0.2.0, see §2).
- **The `embed` path's third repository:** `utterance-engine` imports `ob-semantic-matcher` from `github.com/adamtc007/ob-poc-rust` @ `ff3f12c7` (default-off feature) — a pin not covered by `scripts/check-shared-pin.sh`.
- **Live concurrent branch (stop condition §18):** `bpmn-lite` has active work on `feat/dir-002-phase-c-slm-training` touching `utterance-engine`; the DIR-002 serving loop is in flight. Coordinate before touching those contracts.
- **BPMN worktree split:** the coordinating checkout is currently `feat/dir-002-phase-c-slm-training` at `ddd143e` and does not contain the semantic-board files (`utterance-engine/src/bpmn_pack.rs` and `bpmn_board.rs`). Those files and the shared DSL pins are present in the separate clean `main` worktree at `745b4ea`. Select and record the integration base before changing BPMN consumer contracts; do not implement against the training branch by accident.
- **Dev-override mechanism:** local development uses a user-global `[patch]` in `~/.cargo/config.toml`, which pollutes consumer `Cargo.lock` files on ordinary builds (stripped git-source lines, `[[patch.unused]]` entries). This, not committed path overrides, is the real hygiene risk (fork F5).

Classify every shared production symbol as one of:

1. host-neutral contract;
2. host-neutral implementation;
3. YAML pack schema/compiler/runtime;
4. persistence adapter;
5. host/UI adapter;
6. fixture or example.

Do not move code in this phase.

### Required baseline checks

Run, or record why the environment cannot run:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo doc --workspace --no-deps --all-features
```

Do not reformat an entire dirty repository to make the baseline green. Separate pre-existing failures from remediation regressions.

### Gate 0

- The ledger identifies every consumer and persisted compatibility surface.
- Dirty user work is preserved.
- Each later move has a named source, destination, and consumer.
- No production behaviour changes.

## 6. Phase 1 — Establish standalone package and CI discipline

This phase creates reliable gates before changing boundaries.

### Shared workspace metadata

Add or reconcile:

- root `README.md`, `LICENSE`, and `CHANGELOG.md`;
- `[workspace.package]` metadata for edition, Rust version, licence, repository, and authorship policy;
- workspace lint configuration;
- crate-level descriptions and READMEs;
- a documented versioning policy;
- a committed lockfile policy appropriate to the workspace;
- dependency licence/advisory policy using the repository-standard tool.

Do not claim a licence without owner confirmation if the current repository has no authoritative licence record. Record that as an explicit blocker rather than inventing one. **Resolved 2026-08-05:** the owner selected MIT, shared crate version `0.2.0`, and MSRV Rust 1.95. Phase 1 applies those rulings consistently; the earlier absence of licence and versioning records remains captured in the Phase 0 baseline receipt.

**Fork F5 — ruled: per-repo (2026-08-05).** This phase removes the dsl/bpmn-lite `[patch]` blocks from `~/.cargo/config.toml` and replaces them with per-repo, uncommitted, gitignored root `.cargo/config.toml` files carrying only the patches that repo needs, present only while actively co-developing. Note ob-poc already commits `rust/.cargo/config.toml` (aliases/env) — the patch file goes at the repo *root* so Cargo's hierarchical merge keeps the committed file untouched. This scopes lockfile pollution to opted-in repos; it does not eliminate it — `git checkout -- Cargo.lock` discipline and the pin gate remain the defense.

### CI

Replace the layering-only assurance with required jobs for:

- formatting;
- workspace check and tests;
- Clippy with warnings denied;
- documentation with warnings denied;
- minimal/default/all-feature combinations for feature-bearing crates;
- `cargo package --list` and `cargo publish --dry-run` for releasable crates;
- dependency/layer validation using Cargo metadata rather than source grep alone;
- a check that application/domain tokens do not occur in shared production source, with a reviewed allowlist for fixtures and documentation.

Keep the existing layering script temporarily, but do not treat it as sufficient.

### Safety cleanup

Rewrite or remove the raw-pointer lifetime extension in the DSL AST traversal (`dsl-core/src/ast.rs:748`, inside `find_unresolved_refs`). Note it is `#[allow(dead_code)]` and `pub(crate)` with no callers — prefer deletion over rewriting. Add `#![forbid(unsafe_code)]` to pure crates and verify with CI.

### Gate 1

- Shared CI is green from a clean checkout.
- Pure crates forbid unsafe code.
- Package metadata is sufficient for independent consumption.
- No capability moves have yet changed consumer APIs.

## 7. Phase 2 — Make DSL the complete configuration and pack foundation

Per the ruled execution order, this phase runs **after** Phases 3 and 4 (which are independent of it) and before Phase 5. Do not refactor SemOS (Phase 5) around a new configuration model until this gate passes; SemOS must consume the DSL pack contract established here rather than define a parallel loader or schema. The contracts extraction (Phase 3) and embedder extraction (Phase 4) do not depend on this gate.

### 2.1 Define the normative pack source model

Inventory the existing manifest, DAG, verb, phrase, argument, applicability, effect, role, and policy YAML. Reconcile duplicate or drifting schemas into one versioned, typed pack source contract owned by the shared DSL workspace.

The source schema must cover every application/domain declaration needed by DSL and SemOS, including:

- pack identity, schema version, domain identity, dependencies, and provenance;
- verbs/capabilities and stable canonical IDs;
- DAG nodes, edges, entry conditions, narrowing rules, and terminal dispositions;
- typed arguments, requirements, defaults, prompts, and validation constraints;
- phrases, locale, evidence role, positive examples, and negative contrasts;
- applicability, effects, risk/harm classification, and abstention metadata;
- roles, capability grants, and other declarative policy mappings;
- adapter binding IDs, without embedding Rust paths, SQL, or executable source;
- compatibility aliases and deprecation metadata;
- identity namespace and canonicalization version.

Use namespaced extension records only where the core schema cannot yet express a legitimate capability. Extensions must be size-bounded, preserved canonically, included in validation/hashing, and forbidden from bypassing admission. Do not use unrestricted `serde_yaml::Value` as the normal model.

### 2.2 Implement the typed source-to-artifact pipeline

The shared DSL workspace must expose a deterministic pipeline:

```text
YAML bytes
  -> parse with source locations
  -> schema validation
  -> cross-reference and graph validation
  -> semantic validation
  -> canonical normalization
  -> immutable compiled pack artifact
  -> content hash and provenance receipt
```

Compilation must reject:

- unknown required semantics or unsupported schema versions;
- duplicate or malformed canonical IDs;
- missing DAG nodes, invalid edges, cycles where prohibited, and unreachable required nodes;
- missing adapter bindings;
- ambiguous aliases or phrases where the policy forbids them;
- invalid argument/default combinations;
- role grants to unknown capabilities;
- non-canonical or unstable namespace declarations;
- unbounded nesting, aliases, metadata, or aggregate sizes;
- host SQL, source-code fragments, and Rust type paths in semantic fields.

Diagnostics must include pack identity, source file, YAML path, and line/column when the parser provides it. Batch validation should report independent errors deterministically rather than stopping at the first failure.

### 2.3 Expose a complete public application API

Add or stabilize public interfaces equivalent to:

```rust
pub trait PackSource {
    fn load(&self, request: &PackRequest) -> Result<PackBytes, PackSourceError>;
}

pub fn parse_pack(source: PackBytes) -> Result<PackDocument, PackParseError>;
pub fn validate_pack(document: PackDocument) -> Result<ValidatedPack, PackValidationErrors>;
pub fn compile_pack(pack: ValidatedPack) -> Result<CompiledPack, PackCompileError>;

pub trait PackRegistry {
    fn install(&self, pack: CompiledPack) -> Result<SemanticSnapshot, RegistryError>;
    fn resolve(&self, identity: &PackIdentity) -> Result<SemanticSnapshot, RegistryError>;
}
```

The exact API may differ, but an application must be able to perform all of the following through documented public interfaces:

- load from bytes, files, embedded resources, or a host-provided source;
- validate without activating;
- compile deterministically;
- install/activate atomically;
- retain and address immutable prior snapshots;
- inspect verbs, DAG position, candidates, arguments, policy, and provenance;
- traverse/query the compiled DAG;
- resolve a stable adapter binding ID;
- explain validation and decision inputs;
- serialize/deserialize the compiled artifact if supported;
- compare source, schema, compiler, and artifact hashes;
- reject stale or incompatible snapshots.

Filesystem access, database access, watching/reload, and HTTP fetching belong behind ports or application adapters. Core compilation accepts bytes/data and performs no ambient I/O.

### 2.4 Remove application vocabulary from DSL Rust types

Replace closed host-specific production variants and defaults. Verified inventory (2026-08-05):

- `SlotType::{Workspace, Cbu, Entity, EntityGraph, Case, Tollgate, Mandate}` (`dsl_types/src/constellation_map_def.rs:153-162`);
- `FocusKind::Cbu` and friends (`sem_os_policy/src/observatory/orientation.rs:96-109`);
- `FocusTarget::{Cbu, InstrumentType}` and `VerbScope::Cbu` (`dsl-core/src/ast.rs:1035,1050`, `config/types.rs:846`);
- the parser default that silently selects CBU (`dsl-core/src/viewport_parser.rs:340-346`);
- the hard-coded business/authoring/governed verb-prefix tables (`sem_os_types/src/agent_mode.rs:110-145`);
- the steward/compliance role-string matching (`sem_os_policy/src/abac.rs:220-227`);
- application tables and primary keys used as semantic meaning.

(MIC/BIC/pricing exists only as one doc comment at `ast.rs:1052` — there are no such variants; the original draft overstated this.)

**Fork F3 — ruled: ratified (2026-08-05).** Replacing `SlotType`'s closed enum with pack-validated identifiers is the resolution of the map-root fork deferred under the R1 ruling (the dropped `SlotType::Workspace` runtime change / phantom `bpmn_dags` table). Ratified: slot kinds are pack-declared; a workspace-rooted map is legal iff a pack declares the kind, and the DAG reference becomes a compiler-validated pack cross-reference. No closed-set carve-out.

Use validated identifiers backed by pack declarations, for example:

```rust
pub struct DomainTypeId(String);
pub struct SlotKind(String);
pub struct FocusKind(String);
pub struct CapabilityId(String);
```

Define normalization, namespaces, length and character limits, reserved values, ordering, serialization, and typed errors. Do not replace useful types with arbitrary strings.

Existing application vocabulary moves to the owning application's YAML pack, not to an `ob-poc-dsl-profile` Rust crate. The generic parser either requires an explicit target, derives it from the active compiled pack, or returns a typed ambiguity; it never contains a CBU fallback.

### 2.5 Migrate and validate the real packs

Migrate the checked-in `ob-poc` YAML sources to the normative schema, and **author** the BPMN pack (no BPMN YAML exists today — `utterance-engine/src/bpmn_pack.rs` is the hard-coded source to transcribe and then retire). Do not maintain a hand-written Rust mirror.

Add a pack lock/receipt containing at least:

- source file hashes;
- schema version;
- compiler version;
- dependency pack identities and hashes;
- compiled artifact hash;
- declared adapter binding IDs.

Add drift checks that regenerate/compile the pack and fail when checked-in manifests, DAG projections, generated artifacts, or receipts no longer agree with their YAML sources.

### Required tests

- unit tests for every schema type and validation rule;
- golden parse/diagnostic tests with source locations;
- deterministic compilation under file-order, map-order, and repeated-run permutations;
- canonical artifact and hash golden vectors;
- graph reachability, invalid-edge, cycle, narrowing, and terminal-disposition tests;
- unknown-field/version and extension-bound tests;
- property tests for identifiers and source-to-artifact round trips;
- committed `ob-poc` and BPMN pack compilation tests;
- proof that no application checkout is required to test the generic compiler;
- proof that domain words occur only in pack fixtures/examples, not shared production branching;
- public-API integration tests written as an external consumer crate.

### Gate 2

- DSL is the single typed loader/compiler/runtime foundation for application semantic packs.
- Every required application operation is available through a documented public API.
- `ob-poc` and BPMN semantics compile from YAML without domain-specific branches in shared Rust.
- The compiled result is deterministic, immutable, versioned, and provenance-addressed.
- SemOS implementation work may now begin against this admitted artifact contract.

## 8. Phase 3 — Extract `semantic-decision-contracts`

### New crate responsibility

Create `/Users/adamtc007/dev/dsl/crates/semantic-decision-contracts` containing only stable, serialisable decision contracts and their validation/canonical hashing:

- canonical candidate, domain, snapshot, graph, board, evidence, and workbook identities;
- candidate semantic slice;
- semantic decision board;
- phrase evidence and phrase roles;
- lane and candidate evidence;
- inference evidence and disposition;
- proposal workbook, slots, provenance, and status;
- contract-specific typed errors;
- canonical encoding and schema-version handling.

Target dependencies should remain narrow: normally `serde`, `sha2`, and `thiserror`, plus a small host-neutral vocabulary crate if `ActionClass` and `HarmClass` cannot live here without a cycle.

Do not pull `sem_os_core`, Tokio, SQL parsing, YAML loading, tracing, database code, or application policy into this crate.

### Compatibility rule

First move the existing implementation without changing serialized field names, enum tags, ordering, or hashes. Preserve compatibility through re-exports from the former module for one deprecation window if needed.

Note: dsl@edded43 already cement-locked the v1 board/evidence hashing with per-field sensitivity, determinism/permutation, and length-prefix collision tests — half of "freeze golden vectors for v1" exists. The extraction must carry those tests across unchanged; hash v2 versions them, never breaks them. Also: bpmn-lite separately uses blake3 for its own serializer identities (`utterance-engine/src/exact.rs`) — a different layer; this plan must not imply a single hash family across repos.

When the crate set changes, update `bpmn-lite/scripts/check-shared-pin.sh` (`SHARED_PACKAGES`, currently the six `sem_os_*`/`dsl*` packages) in lockstep in the same consumer commit, with its `--self-test` fixtures — otherwise the pin gate goes red or, worse, hollow.

Add golden tests using captured BPMN and `ob-poc` values:

- JSON bytes remain compatible;
- board and evidence hashes remain identical (there is no distinct workbook hash — see I-3);
- deserialization still rejects invalid hashes, duplicate identities, non-finite scores, and invalid state transitions;
- both old and new import paths describe the same Rust types during the compatibility window.

### Canonical hash v2

Verified sites: five `format!("{:?}", …)` uses inside the framed board/evidence digests (`decision_board.rs:569,580,601,633,955`) plus one the original draft missed — `sem_os_policy/src/acp_projection.rs:146-160`, which is **unframed** (`hasher.update` concatenation with no length prefixes), so it carries a field-boundary-ambiguity defect on top of the Debug dependence and should be fixed first.

Handle removal of `Debug`-formatted enum hashing as a separate sub-phase:

1. Freeze golden vectors for the current algorithm as schema/hash version 1.
2. Introduce explicit stable tags for version 2.
3. Add readers/verifiers for both versions where persisted v1 records can exist.
4. Emit only v2 after consumer compatibility has shipped.
5. Expire or migrate pending v1 workbooks according to an owner-approved operational rule.

Never alter v1 hash output while retaining the v1 schema identifier.

### Gate 3

- The new crate builds and tests by itself.
- Existing consumers can use compatibility re-exports without behaviour changes.
- Golden v1 hashes are frozen.
- Hash v2, if implemented, is explicitly versioned and migration-tested.

## 9. Phase 4 — Extract the pure semantic embedder

### New crate responsibility

Create `/Users/adamtc007/dev/dsl/crates/semantic-embedder` or another independently owned neutral repository if model licensing or release cadence requires it.

It may contain:

- the `Embedder` trait and typed errors;
- tokenizer and candidate-pair serialization contracts;
- Candle/native model loading and inference;
- deterministic normalization;
- model bundle/card validation;
- optional Hugging Face download support behind an explicit feature;
- test fixtures and a deterministic fake embedder.

It must not contain:

- SQLx or pgvector;
- PostgreSQL repositories;
- the `"ob-poc"` schema;
- CBU/KYC/client-group logic;
- feedback or centroid persistence;
- population binaries tied to an application database;
- host HTTP handlers.

### Feature matrix

Design actual feature isolation, for example:

```text
default = []
candle = [...]
huggingface-download = ["candle", ...]
```

Prove with Cargo metadata and minimal-feature builds that database packages are absent.

### `ob-poc` adapter

Retain or rename the current matcher as an application crate. Move SQLx, pgvector, repositories, client-group resolution, feedback, centroid persistence, and population commands into `ob-poc`-owned crates. They consume `semantic-embedder`; the embedder never consumes them.

### BPMN cutover

Change `utterance-engine` to depend directly on `semantic-embedder`. Preserve the existing default-off embedding feature and current deterministic fallback when no model is configured. Today the `embed` feature pulls `ob-semantic-matcher` from a **third repository** (`github.com/adamtc007/ob-poc-rust` @ `ff3f12c7`, `default-features = false`) — a pin outside `check-shared-pin.sh`'s coverage. After cutover, extend `SHARED_PACKAGES` (or add a parallel assertion) so the embedder pin is gated too, in the same commit.

### Required tests

- no-feature pure contract build;
- Candle feature build and deterministic fixture inference;
- invalid bundle and incompatible tokenizer failures are typed;
- native inference output matches the pre-extraction implementation within the existing tolerance;
- `cargo tree` proves SQLx and pgvector are absent from the `utterance-engine`/embedding feature closure (NOT from `bpmn-lite` as a whole — sqlx is a legitimate workspace dependency of nine of bpmn-lite's own server/store crates and will remain in the lock regardless; pgvector is already absent from the bpmn-lite lock);
- `ob-poc` database integration tests remain green.

### Gate 4

- `bpmn-lite` no longer imports `ob-semantic-matcher`.
- Pure embedding has no database or host dependency.
- `ob-poc` owns all matcher persistence and population behaviour.

## 10. Phase 5 — Move host semantics from SemOS Rust into YAML packs

SemOS must consume the immutable DSL artifact admitted in Phase 2. It must not parse a separate policy YAML dialect or reconstruct the pack from application Rust types.

### Remove application semantics from Rust

Move the following declarations into the owning application's versioned YAML pack:

- command-prefix families for CBU, trading profile, investor, custody, deal, billing, KYC, screening, and ownership;
- role-name interpretation for steward, compliance, and regulatory officers;
- `ob-poc` deterministic UUID namespaces;
- application authorisation mappings;
- mode/capability eligibility rules;
- verb-to-action/harm classification;
- DAG narrowing and candidate applicability;
- compatibility aliases and defaults.

Move host configuration lookup and file watching behind the Phase 2 `PackSource`/registry ports. Move integration tests that require `ob-poc/config` into the application repository or compile copied, versioned fixture packs through the public API.

### Generic SemOS execution model

SemOS operates only on admitted, typed snapshots, conceptually:

```rust
pub fn build_decision_board(
    snapshot: &SemanticSnapshot,
    position: &GraphPosition,
    context: &TurnContext,
) -> Result<SemanticDecisionBoard, DecisionError>;

pub fn evaluate_capability(
    snapshot: &SemanticSnapshot,
    principal: &PrincipalContext,
    capability: &CapabilityId,
) -> Result<CapabilityDecision, PolicyError>;
```

The exact functions may differ. The invariant is that the snapshot contains the declared verbs, graph, phrases, arguments, namespace, and policy mappings. Generic Rust algorithms validate and evaluate those typed declarations deterministically. Do not create `ObPocPolicy`, `BpmnPolicy`, `CapabilityProfile` implementations, command-prefix match statements, or role-name branches as substitutes for pack data.

Concrete side effects still require application code. A pack selects a stable `CapabilityId`/adapter binding; the host registers an implementation for that ID. SemOS may check that a binding exists, but the adapter performs the real external action. YAML must never contain arbitrary executable code, SQL, or dynamic library paths.

### Public SemOS API completeness

Through supported public interfaces an application must be able to:

- construct a SemOS service from an immutable semantic snapshot;
- supply turn/principal/graph context without host-specific types;
- enumerate applicable verbs/candidates without truncating the authoritative board;
- evaluate deterministic policy and receive typed reasons/evidence;
- resolve ambiguity, abstention, and escalation;
- start/update/materialize a proposal workbook;
- inspect the active snapshot and policy fingerprints;
- replace a snapshot atomically while retaining in-flight snapshot identity;
- register and resolve concrete capability adapters by stable ID;
- audit every decision back to pack/artifact hashes;
- receive typed compatibility, validation, policy, and adapter errors.

If any application must import a private module or reproduce an algorithm to do this, extend the shared public API before cutting over the application.

### Identity migration

Do not replace the existing `semantic-os:ob-poc:sem_reg` namespace in place. Represent it as the `ob-poc` v1 namespace in pack metadata. Shared code consumes the validated namespace from the compiled artifact. Existing identities continue resolving; new namespace versions require a mapping or dual-read migration.

### Error boundaries

Replace public `anyhow::Result` in reusable libraries with typed errors. Preserve context by wrapping lower-level errors with sources. Keep `anyhow` in binaries, migration tools, and application composition roots.

### Gate 5

- Shared SemOS production source contains mechanisms, not host command or role policy.
- SemOS consumes the Phase 2 compiled artifact and has no second YAML schema/loader.
- Existing `ob-poc` permissions and identities remain byte-for-byte compatible where required.
- BPMN supplies its own YAML pack without importing an `ob-poc` semantic adapter.
- Application Rust code registers technical capability implementations but contains no duplicate semantic decision table.
- Shared tests run without an `ob-poc` checkout or ignored host-dependent test.

**Execution status (5 August 2026): complete.** Shared revision
`9b76c951a084cca6af4885609d46f8dc02637b00` and `ob-poc` consumer revision
`ec0ba7ddfe4100520a151c58ab9edbef11d45437` satisfy Gate 5. See
[`shared_crates_phase_5_gate_receipt_2026-08-05.md`](shared_crates_phase_5_gate_receipt_2026-08-05.md).

## 11. Phase 6 — Define the Sage/REPL boundary

### Forensic decision first

Determine whether Sage represents:

1. a transport-neutral REPL protocol;
2. a reusable server runtime; or
3. an `ob-poc` application UI.

Only the first category automatically belongs in the shared workspace. A reusable server runtime may be shared only if it has no host routes, assets, policy, database assumptions, or domain vocabulary.

The forensic evidence is already in (2026-08-05): `dsl-sage` is an **orphan crate** — zero consumers anywhere (only its workspace-member entry and its own dev-dep reference it); the live REPL protocol types are declared inline in ob-poc's `src/api/repl_routes_v2.rs` (~5.8k lines) and `crates/ob-poc-sage/`, with hard-coded persona/phase literals. This is category 3 (application UI). **Fork F2 — ruled (2026-08-05): no shared `repl-contracts` crate; revisit only if a second Sage consumer materializes.** This phase therefore reduces to the host-adapter cleanup below plus retiring or clearly quarantining the orphan `dsl-sage` crate.

### Shared contract, if justified

Create `repl-contracts` for transport-neutral request/response/session/proposal types. It must not contain Axum routes, static HTML, `create-cbu`, BPMN operations, or application domains.

### Host adapters

- `ob-poc` keeps its Sage routes, persistence, domain catalogue, and UI behaviour.
- `bpmn-lite-server-designer` keeps BPMN routes and UI integration.
- Replace hard-coded `create-cbu`, `ob-poc:cbu.create`, and `ob-poc` fallback behaviour in the BPMN server with injected BPMN candidates or isolate it as an explicitly named legacy demo fixture.

Do not call the local keyword classifier the shared Sage engine if it does not execute the shared protocol/runtime.

### Gate 6

- Ownership is documented and reflected by dependencies.
- BPMN server production behaviour contains no `ob-poc` command fallback.
- Shared protocol tests prove serialization compatibility.
- Each host can run independently with its own adapter.

**Execution status (5 August 2026): complete and integrated.** The orphan
`dsl-sage` retirement commit `4ad0e338ddbb393111d0f116bcb4d53b9ef8054d` is an
ancestor of the active `ob-poc` consumer branch. The reviewed BPMN host-boundary
change was integrated into the coordinating branch as `3e3ecf1`: the legacy
route and request shape remain compatible, but retry and diagnostic actions now
require explicit caller context and cannot invent `create-cbu` or an `ob-poc`
command. No shared REPL crate was created under ruling F2. See
[`shared_crates_phase_6_receipt_2026-08-05.md`](shared_crates_phase_6_receipt_2026-08-05.md).

## 12. Phase 7 — Consumer cutover and dependency narrowing

### Shared release candidate

Before consumer edits:

1. make the shared workspace clean and green;
2. update crate versions consistently;
3. generate release notes describing moved APIs and compatibility re-exports;
4. create an immutable signed or annotated release tag according to repository policy;
5. record the exact commit and source checksum;
6. do not remove compatibility re-exports yet.

If crates are not published to a registry, consumers must use the exact Git revision. Do not use `branch =`, an unpinned tag without verification, or a developer path. **ob-poc violates this today**: it pins all six dsl crates by mutable `tag = "v0.1.5"` (one release behind bpmn-lite's `rev = fa51217`/v0.1.6). Part of this phase is converting ob-poc to exact-rev pins and closing the three-way skew against the shared release candidate.

Every change to the shared crate set or pinned rev must update `bpmn-lite/scripts/check-shared-pin.sh` (`SHARED_PACKAGES` + self-test fixtures) in the same consumer commit.

### `bpmn-lite`

Narrow dependencies to:

- `semantic-decision-contracts`;
- the smallest vocabulary crate needed for action/harm classes;
- `semantic-embedder` only behind the existing optional feature;
- `repl-contracts` only if the shared Sage boundary is actually used.

Remove direct `sem_os_policy` and `ob-semantic-matcher` dependencies where they exist only to reach these APIs. Run `cargo tree` and record the dependency reduction.

### `ob-poc`

Cut over to the same shared release while loading its YAML packs and composing persistence, concrete capability, matcher, and Sage adapters. Do not duplicate the extracted shared code or semantic policy back into host Rust.

### Compatibility tests

- captured utterances produce the same candidate/disposition outcomes;
- decision boards and evidence remain compatible for the same schema/hash version;
- proposal workbooks survive restart and round-trip;
- current model bundle outputs remain within their declared tolerance;
- `ob-poc` authorisation and command resolution remain unchanged;
- BPMN has no transitive SQLx/pgvector dependency from embedding;
- no consumer uses compatibility re-exports in new source.

### Gate 7

- Both applications build against the same immutable shared revision.
- Dependency trees contain no unintended application reverse edge.
- Focused, workspace, integration, and replay tests pass.
- Compatibility differences are either zero or owner-approved and versioned.

**Execution status (5 August 2026): complete.** The qualified shared release is
the annotated tag `v0.2.1` at
`586431f81e2bb9101578af5167b8a35335f5a09e`. BPMN consumes that exact
revision at `de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a`; `ob-poc` consumes
both exact revisions at `e51dda11a02f4993e8023b934c4fd1df4293b983`.
Locked metadata contains one shared source and one BPMN source, and the three
full workspace suites are green. The earlier `v0.2.0` tag remains immutable but
is superseded because final CI qualification exposed a warnings-as-errors
Clippy failure. See
[`shared_crates_phase_7_receipt_2026-08-05.md`](shared_crates_phase_7_receipt_2026-08-05.md).

## 13. Phase 8 — Deployment and rollback

The shared crates are libraries, so deployment means releasing immutable source artifacts, rebuilding consumers, and promoting the resulting application artifacts. Do not treat a local workspace build as deployment.

### Promotion order

1. Release/tag the shared crates.
2. Build and test `ob-poc` and `bpmn-lite` against the exact release revision.
3. Publish application release candidates with SBOMs and dependency receipts.
4. Deploy to non-production/shadow environments.
5. Replay captured traffic and compare semantic evidence, decisions, hashes, latency, and failures.
6. Promote one consumer at a time; the shared release remains immutable.
7. Observe before removing compatibility paths.

### Shadow checks

Compare at minimum:

- candidate set and ordering;
- lane and final scores;
- disposition and clarification behaviour;
- board/evidence/workbook versions and hashes;
- model loading, memory, and latency;
- request error categories;
- database query count in `ob-poc`;
- BPMN server dependency/image size;
- rollback readability of persisted records.

### Rollback

Rollback must be possible by reverting each consumer to its previous exact shared revision and application artifact. Therefore:

- do not delete old tags;
- do not mutate released model bundles;
- retain readers for persisted v1 hashes/workbooks through the rollback window;
- do not deploy an irreversible persistence migration in the same release as the library cutover;
- record the previous and new revision in the deployment receipt.

### Gate 8

- Shadow comparison meets the owner-approved equality/tolerance policy.
- Rollback has been exercised in a non-production environment.
- Both application artifacts identify the shared source revision at runtime or in build metadata.
- Operational dashboards distinguish contract-version errors from model or host-adapter failures.

**Execution status (5 August 2026): local release qualification complete;
external promotion held.** Immutable application release candidates, SBOMs,
dependency receipts, isolated PostgreSQL-backed shadow probes, and rollback
rehearsals exist for both consumers. BPMN's three-case decision replay was
strictly identical across the candidate and rollback artifact. `ob-poc`'s
canonical ACP policy and persisted V2 session semantics were identical across
its candidate and exact prior RC, and the candidate now packages all runtime
YAML packs and snapshots. Gate 8 is not declared complete because no external
registry/non-production target, captured production traffic set, owner-approved
tolerance policy, or dashboard destination has been supplied. The rehearsal
also found that `ob-poc`'s SQLx migration set cannot bootstrap an empty database
and that its canonical schema export omits current control-plane tail migrations.
See
[`shared_crates_phase_8_receipt_2026-08-05.md`](shared_crates_phase_8_receipt_2026-08-05.md).

## 14. Phase 9 — Remove compatibility scaffolding

**Execution status (5 August 2026): readiness complete; destructive cleanup
deferred by the phase precondition.** `ob-poc` production and test source now
imports canonical `sem_os_types` and `dsl_types` values directly, both
consumers resolve the immutable shared `v0.2.1` revision, and the permanent
local dependency/reproducibility gates pass. Public shims, persisted readers,
and the BPMN compatibility sentinel remain because Gate 8 external promotion
has not occurred and the rollback window has not closed. The eventual removal
must be a new breaking shared release, not a mutation of `v0.2.1`. See
[`shared_crates_phase_9_readiness_2026-08-05.md`](shared_crates_phase_9_readiness_2026-08-05.md).

Only after both consumers have shipped and the rollback window has closed:

- remove deprecated re-exports;
- remove old host-specific variants from shared APIs;
- remove obsolete feature aliases;
- remove temporary dual-read logic if retention policy permits;
- delete copied fixtures only after canonical replacements exist;
- update architecture diagrams and crate ownership documentation;
- minimize lockfiles and feature matrices;
- close or explicitly carry over all deferred findings.

Make removals in separate commits from functional extraction so they can be reviewed and reverted independently.

### Gate 9

- Searches and Cargo metadata find no forbidden reverse dependencies.
- No application imports deprecated shared paths.
- Documentation matches the actual source layout.
- CI proves clean checkout reproducibility.
- The carry-over ledger contains an owner and intended release for every remaining item.

## 15. Permanent test and release matrix

| Surface | Pull request | Shared release | Consumer deployment |
|---|---|---|---|
| Formatting and Clippy | Required | Required | Recorded |
| Minimal/default/all features | Required | Required | Exact selected features |
| Unit/property tests | Required | Required | Recorded |
| Golden serialization/hash vectors | Required | Required | Replay sample |
| Cargo dependency/layer audit | Required | Required | SBOM comparison |
| Package/publish dry run | When packaging changes | Required | N/A |
| `ob-poc` compatibility | Focused when affected | Full | Shadow replay |
| BPMN compatibility | Focused when affected | Full | Shadow replay |
| Model bundle compatibility | When embedder changes | Full | Shadow inference |
| Rollback/read-old-data | When schema changes | Required | Exercised |

## 16. Commit and review strategy

Use small, capability-aligned commits. A suitable sequence is:

1. `ci: establish standalone shared-crate gates`
2. `refactor: remove unsafe DSL traversal`
3. `feat(dsl): define the versioned semantic pack schema and public API`
4. `feat(dsl): compile deterministic admitted artifacts from YAML`
5. `refactor: migrate ob-poc and BPMN semantics from Rust into YAML packs`
6. `feat: extract semantic decision contracts`
7. `feat: version canonical decision hashes`
8. `refactor(sem-os): evaluate role and capability policy from compiled packs`
9. `feat: extract host-neutral semantic embedder`
10. `refactor(ob-poc): isolate semantic persistence adapters`
11. `refactor: define transport-neutral REPL contracts`
12. `refactor(bpmn): cut over to narrow shared crates and its BPMN pack`
13. `refactor(ob-poc): cut over to shared release and its application packs`
14. `docs: publish release and deployment receipts`
15. `cleanup: remove expired compatibility paths`

Do not mix cross-repository changes into one apparent atomic commit. Instead, reference prerequisite commit IDs and release tags in each consumer commit message and pull request.

## 17. Required final handoff

The Zed session must finish with a Markdown receipt containing:

- every repository, branch, starting HEAD, and ending HEAD;
- every created, moved, deleted, and compatibility-re-exported public module;
- old and new dependency graphs;
- exact shared release tag/revision and crate versions;
- commands run and exact pass/fail/skip counts;
- serialization and hash compatibility results;
- model inference comparison results;
- application shadow/deployment results;
- rollback revision and rollback-test result;
- known carry-overs, owners, and target releases;
- confirmation that unrelated pre-existing work was not committed.

## 18. Stop conditions

Stop and request owner direction rather than guessing if:

- existing persisted boards or workbooks cannot be identified by schema/hash version;
- changing the UUID namespace would orphan identities;
- domain vocabulary is consumed externally and no compatibility contract exists;
- model or dataset licensing prevents moving the embedder into the proposed shared workspace;
- repository licensing is absent or contradictory;
- a concurrent branch is modifying the same public contracts;
- the required Rust/MSRV policy conflicts between consumers;
- deployment would require an irreversible database migration;
- golden semantic outcomes change without an approved product decision.

## 19. Explicit non-goals

This plan does not:

- redesign BPMN execution semantics;
- retrain or promote a new language model merely because the embedder moves;
- merge the three repositories into a monorepo;
- require publishing proprietary crates to a public registry;
- replace PostgreSQL or the existing production deployment platform;
- rewrite the UI;
- remove useful domain fixtures from tests where they are clearly fixtures;
- promise a repository rename before the ownership boundary is correct.

The essential outcome is not a particular folder name. It is that generic crates can be understood, built, tested, versioned, and consumed without importing either host application's policy or infrastructure.

## 20. Fork register (2026-08-05 verification review)

Surfaced, not silently decided. Rulings recorded here as they land.

| Fork | Question | Recommendation | Status |
|---|---|---|---|
| F1 | Re-sequence phases: 0 → 1 → 3 → 4 → 6 (forensic) → 2 → 5 → 7+ | Yes — Phases 3/4 don't depend on Phase 2's schema unification | **Ruled: yes** (2026-08-05) |
| F2 | Shared `repl-contracts` crate, given `dsl-sage` is an orphan and the live protocol is inline application code | No shared crate; revisit only on a second Sage consumer | **Ruled: no shared crate** (2026-08-05). Phase 6 reduces to host-adapter cleanup + retiring/quarantining `dsl-sage` |
| F3 | §2.4 `SlotType` closed enum → pack-validated IDs resolves the map-root fork deferred under the R1 ruling (`SlotType::Workspace` / phantom `bpmn_dags`) | Ratify: slot kinds become pack-declared; workspace-rooted maps legal iff a pack declares the kind; DAG refs become compiler-validated pack cross-refs | **Ruled: ratified** (2026-08-05). The deferred R1 map-root fork is resolved by the generic mechanism; no closed-set carve-out in §2.4 |
| F4 | Tag dsl `edded43` as `v0.1.7` first (closes CO-01) so Phase 0 baselines against a tagged rev | Yes | **Ruled: yes; executed** — annotated tag `v0.1.7` at `edded43` pushed 2026-08-05 |
| F5 | Global `~/.cargo/config.toml` `[patch]`: keep with lockfile-restore discipline, or move to per-repo uncommitted `.cargo/config.toml` | Per-repo: delete the dsl/bpmn-lite patch blocks from the global config; each consumer repo carries an uncommitted, gitignored root `.cargo/config.toml` with only the patches it needs, present only while co-developing | **Ruled: per-repo** (2026-08-05). Caveat stands: this scopes lockfile pollution, it does not eliminate it — restore discipline + the pin gate remain the defense. Phase 1 implements the switch |
| F6 | Which BPMN history is the consumer base: the active DIR-002 training branch (`ddd143e`) or semantic-board `main` (`745b4ea`)? | Base the remediation on semantic-board `main`, then rebase/merge the DIR-002 work under its own owner before touching shared utterance contracts | **Ruled: semantic-board `main`** (2026-08-05). The remediation branch starts at `745b4ea`; DIR-002 remains separately owned |
| F7 | Shared workspace licence, crate version, and MSRV | MIT, `0.2.0`, Rust 1.95 | **Ruled as recommended** (2026-08-05); Phase 1 implements the metadata and policy |
