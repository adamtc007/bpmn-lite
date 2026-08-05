# Shared-crates remediation Phase 2 blueprint

**Phase:** 2 — make DSL the complete configuration and pack foundation
**Date:** 5 August 2026
**Shared base:** DSL `5ac7da7a513744e907ca110484c3a6a9472ae985` on `feat/semantic-embedder`
**BPMN base:** `506e931b122014b0e2bdaf44d5ed296b2bcf7f2e` on `refactor/sage-host-boundary`
**ob-poc base:** `4ad0e338ddbb393111d0f116bcb4d53b9ef8054d` on `cleanup/retire-dsl-sage`

## Decision

Add a safe, domain-neutral `semantic-pack` crate to the shared DSL workspace. It owns the one normative source schema, admission pipeline, canonical artifact, provenance receipt and in-memory immutable registry used by DSL and, in Phase 5, SemOS. Applications own their YAML and their adapter implementations. A pack may name an adapter binding but may not embed a Rust path, SQL, shell command or executable source.

The generic semantic pack is distinct from ob-poc's journey presentation manifest. The latter contains UI sections, questions, progress messages and runbook templates. Implementation discovery showed that exact journey-file bytes are a persisted identity boundary, so adding an in-file `semantic` section would change deployed hashes before the Phase 5 consumer cutover. The controlled resolution is a sibling `rust/config/semantic-packs/*.yaml` source set with an exhaustive one-to-one drift test against `rust/config/packs/*.yaml`. This preserves deployed journey behavior and hashes while giving the shared compiler one typed projection. Phase 5 may then retire the remaining SemOS-specific loader against the admitted artifact and decide whether the sources can safely converge.

The BPMN hard-coded candidate table is transcribed into a BPMN-owned YAML pack. `utterance-engine` embeds and compiles that YAML and projects the resulting immutable capabilities into the existing public decision-board contracts. There is no hand-maintained Rust semantic mirror.

## Invariants and boundaries

1. Shared production Rust contains no ob-poc or BPMN vocabulary, role names, table names, default domain, or adapter implementation.
2. Source parsing and compilation accept bytes and metadata only. Files, HTTP, database access, watching and reload are adapters implementing `PackSource`.
3. Schema version, identity namespace and canonicalization version are explicit and admitted. Unknown fields fail closed.
4. Identifiers are validated typed values, not arbitrary strings: ASCII lowercase segments, `.`/`-` separators, 1–128 bytes, no empty segments, no leading/trailing separator and no reserved `system.` prefix for application declarations.
5. Extensions are namespaced, typed configuration values with depth, item, string and aggregate byte limits. They are canonicalized and hashed and cannot introduce executable material.
6. Validation accumulates independent diagnostics in deterministic YAML-path order. Parse diagnostics carry parser line/column when available; all diagnostics carry the source name and the best-known pack identity.
7. Compilation normalizes every unordered collection, emits deterministic canonical JSON bytes, and computes SHA-256 hashes with an explicit canonicalization-version domain separator.
8. `CompiledPack` and installed `SemanticSnapshot` are immutable value objects. Registry activation swaps an exact artifact atomically; old hashes remain resolvable.
9. Registry resolution and activation require exact identity/version/hash compatibility. Stale expected heads fail with a typed conflict.
10. Existing semantic-board and workbook hash/UUID versions do not change. BPMN adapter payload hashes remain byte-for-byte compatible.
11. Existing ob-poc journey pack bytes, hashes and runtime behavior remain unchanged; sibling semantic sources are admitted independently until the Phase 5 consumer cutover is explicit.
12. The existing `dsl-core` permissive workspace/verb pack loader is compatibility-only after this phase and must not be used for semantic admission.
13. The `sem_os_policy::domain_pack` loader is not redesigned in this phase. Phase 5 replaces it with a projection from `CompiledPack`.
14. The dirty coordinating worktree, the dirty ob-poc main worktree and `.cargo/config.toml.example` are not staged or altered.

## Shared crate public surface

Create `crates/semantic-pack` with the following modules:

```text
semantic-pack/src/
  lib.rs          public exports and compiler version
  diagnostic.rs   structured parse/validation/compile/source/registry errors
  identity.rs     PackId, CapabilityId, DomainTypeId, SlotKind, FocusKind,
                  AdapterBindingId, RoleId, GraphNodeId and validated versions
  source.rs       PackBytes, PackRequest, PackSource and typed YAML source model
  validate.rs     bounds, semantic, cross-reference and graph admission
  artifact.rs     canonical immutable compiled representation and receipts
  compile.rs      normalization, canonical encoding and content hashing
  registry.rs     PackRegistry plus thread-safe InMemoryPackRegistry
```

Normative source outline:

```yaml
schema_version: 1
pack:
  id: example.pack
  version: 1.0.0
  domain: example
  identity_namespace: example.semantic
  canonicalization_version: 1
  dependencies: []
  provenance: { source: example.yaml, revision: fixture-v1 }
declarations:
  domain_types: []
  slot_kinds: []
  focus_kinds: []
capabilities: []
graph: null
policy:
  phrase_ambiguity: reject
  abstention: { enabled: true, candidate_id: abstain.none_of_the_above }
  roles: []
extensions: {}
```

Capabilities include stable ID, adapter binding, title/summary, action class, applicability, effect, typed arguments/defaults/constraints/prompts, governed phrases, examples, contrasts, harm class, aliases and deprecation data. Graph nodes reference capabilities and declare narrowing/terminal dispositions. Role grants reference admitted capabilities.

Public API:

```rust
pub trait PackSource {
    fn load(&self, request: &PackRequest) -> Result<PackBytes, PackSourceError>;
}

pub fn parse_pack(source: PackBytes) -> Result<PackDocument, PackParseError>;
pub fn validate_pack(document: PackDocument)
    -> Result<ValidatedPack, PackValidationErrors>;
pub fn compile_pack(pack: ValidatedPack) -> Result<CompiledPack, PackCompileError>;
pub fn admit_pack(source: PackBytes) -> Result<CompiledPack, PackAdmissionError>;

pub trait PackRegistry {
    fn install(&self, pack: CompiledPack) -> Result<SemanticSnapshot, RegistryError>;
    fn activate(
        &self,
        identity: &PackIdentity,
        expected_current: Option<&ArtifactHash>,
    ) -> Result<SemanticSnapshot, RegistryError>;
    fn resolve(&self, identity: &PackIdentity) -> Result<SemanticSnapshot, RegistryError>;
    fn resolve_hash(&self, hash: &ArtifactHash) -> Result<SemanticSnapshot, RegistryError>;
}
```

`CompiledPack` exposes read-only inspection of metadata, capabilities, declared kinds, graph, policy, adapter bindings, provenance, canonical bytes and source/schema/compiler/artifact hashes. It supports graph successors, position/candidate lookup, and stable adapter resolution.

## Admission rules

- Reject unsupported schema/canonicalization versions and unknown YAML fields.
- Reject malformed or duplicate IDs, aliases, phrases, graph nodes and declarations.
- Reject undeclared slot/focus/domain kinds and missing capability/role/graph references.
- Reject missing adapter bindings and invalid argument/default combinations.
- Reject forbidden cycles, missing entry nodes, invalid edges, unreachable required nodes, invalid narrowing targets and invalid terminal dispositions.
- Reject ambiguous aliases or positive phrases when policy is `reject`.
- Reject Rust paths, SQL statements, shell fragments and source-code-shaped adapter IDs or semantic extensions.
- Enforce limits on source size, collection counts, strings, extension nesting, extension nodes and canonical artifact size.

## Generic vocabulary remediation

The shared identifier definitions become the normative `DomainTypeId`, `SlotKind`, `FocusKind` and `CapabilityId` types. Closed application enums and defaults are removed or converted through compatibility deserializers where persisted data requires it:

- `dsl_types::constellation_map_def::SlotType` becomes the validated shared `SlotKind` (with a deprecated type alias only if downstream compilation proves it necessary);
- `dsl_core::ast::FocusTarget` and `sem_os_policy::observatory::FocusKind` use pack-declared focus identifiers rather than domain variants;
- `dsl_core::config::VerbScope` uses an optional declared scope kind rather than a CBU variant;
- the viewport parser requires an explicit target or returns typed missing/ambiguous-target diagnostics;
- verb mode prefix classification and ABAC role grants are read from compiled policy declarations rather than hard-coded word lists.

Serialization compatibility fixtures are required before changing a persisted shape. If a live persisted representation cannot be versioned without orphaning state, work stops under the charter rather than guessing.

## Consumer migration

### ob-poc

1. Create `refactor/semantic-pack-sources` in the clean active checkout.
2. Integrate completed pack-truth commit `d2afc0c49d8b2b6cea8fb83f95474c17f0d4b639`; never edit the dirty `ob-poc-bpmn-pack-truth` worktree.
3. Add one typed sibling `rust/config/semantic-packs/*.yaml` source for each real `rust/config/packs/*.yaml`, referencing canonical verb IDs and application-owned adapter binding IDs. Do not mutate the persisted legacy source bytes in this phase.
4. Add an application adapter/test that compiles each sibling through `semantic-pack` and cross-checks every source identity, invocation phrase, `allowed_verbs`, `forbidden_verbs` and adapter binding against the corresponding journey manifest.
5. Commit a deterministic lock receipt outside the `*.yaml` scan glob with source hashes, dependency identities/hashes, compiler/schema versions, artifact hashes and adapter binding IDs.
6. Preserve the existing journey loader and legacy manifest hash until Phase 5.

### BPMN

1. Create `refactor/bpmn-semantic-pack` from the Phase 6 branch.
2. Author `utterance-engine/config/bpmn-semantic-pack.yaml` by transcribing every current candidate, argument, phrase, example, contrast, harm class and binder support value.
3. Add the exact shared `semantic-pack` dependency and pin it with the other shared crates.
4. Replace `all_specs`, `candidate_spec` and positional-argument tables with a once-compiled embedded pack projection. Preserve current ordering, public results and adapter payload hashes.
5. Add source/artifact lock receipt generation and a drift test. Remove the hard-coded semantic constructors after equivalence tests pass.

## Tests and gates

Shared focused gates:

```text
cargo test -p semantic-pack --all-features --locked
cargo clippy -p semantic-pack --all-targets --all-features --locked -- -D warnings
cargo doc -p semantic-pack --no-deps --all-features --locked
bash scripts/check-domain-neutral.sh
bash scripts/check-layering.sh
bash scripts/check-dependencies.sh
```

Tests cover every source type/rule, source-location diagnostics, deterministic error ordering, file/map/repeat permutations, graph validity, limits, forbidden executable material, identifier properties, round-trip and canonical hash goldens, registry stale activation and external-consumer usage.

Consumer focused gates compile every checked-in real pack, verify receipts, prove BPMN semantic/board/hash equivalence and run each repository's pin guard. Full shared workspace format/check/test/Clippy/doc/package/deny gates then run, followed by proportionate full BPMN and ob-poc gates.

## Commit sequence

1. DSL: `feat(pack): add deterministic semantic pack compiler`.
2. DSL: `refactor(types): replace closed domain vocabulary with pack identifiers` if compatibility evidence permits the whole §2.4 boundary independently.
3. BPMN: `refactor(utterance): compile semantic candidates from YAML`.
4. ob-poc: `refactor(packs): admit semantic configuration through DSL`.
5. Documentation receipt remains in the coordinating programme worktree and is not mixed into consumer commits.

Phase 2 stops at Gate 2. Phase 5 does not begin in the same gate.
