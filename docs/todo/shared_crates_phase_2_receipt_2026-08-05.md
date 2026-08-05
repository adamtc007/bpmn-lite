# Shared-crates remediation Phase 2 receipt

**Date:** 5 August 2026
**Phase:** 2 — make DSL the complete configuration and pack foundation
**Outcome:** configuration foundation complete; SemOS policy-consumer cutover explicitly carried to Phase 5

## Delivered

The shared DSL workspace now contains the safe, MIT-licensed `semantic-pack` crate. It provides strict typed YAML parsing, deterministic accumulated validation diagnostics, canonical normalization, SHA-256 source/artifact receipts, immutable compiled artifacts, stable adapter binding lookup, graph traversal and an atomic content-addressed registry with stale-activation fencing.

The public API admits exact bytes without ambient I/O and separately exposes parse, validate, compile, install, activate, identity/hash resolution and read-only inspection of declarations, capabilities, arguments, graph, policy, extensions and provenance. Generic `DomainTypeId`, `SlotKind`, `FocusKind` and `CapabilityId` identifiers replace the reviewed closed application-specific DSL types. The generic viewport parser no longer defaults an omitted target to CBU.

BPMN's 26-candidate Rust registry was transcribed to `utterance-engine/config/bpmn-semantic-pack.yaml`. Production now embeds and admits that source once and projects the compiled capabilities into the existing board contract. The old hard-coded constructors and positional binder mirror were removed. Exact board behavior, candidate coverage and adapter payload hashes remain pinned by tests and a checked-in source/artifact receipt.

`ob-poc` now owns 14 sibling semantic YAML sources under `rust/config/semantic-packs/`, one for each existing journey manifest. The application adapter exposes byte/file admission through the shared compiler. Tests exhaustively cross-check identities, phrases, allowed/forbidden verbs and bindings, and `rust/config/semantic-packs.lock` records each source hash, schema/compiler version, dependencies, bindings and artifact hash.

## Controlled source-layout decision

The blueprint originally proposed adding a `semantic` section to each existing journey YAML. Implementation inspection established that the raw bytes of those files are already hashed as persistent journey identity. Mutating them would violate behavior/identity preservation before the Phase 5 cutover. The semantic sources therefore live beside, rather than inside, the legacy manifests. A one-to-one cross-source drift test prevents either representation changing silently. No existing journey YAML byte or hash changed.

## Exact revisions and commits

### Shared DSL

Branch `feat/semantic-pack`, pushed at `c65f01d514c99bf087673ce366ed3b7549217c1d`:

- `23a5c5e` — `feat(pack): add deterministic semantic pack compiler`
- `1896623` — `refactor(types): use pack-declared semantic identifiers`
- `9ca621b` — `fix(pack): distinguish semantic prose from executable material`
- `7da4b10` — `feat(pack): expose typed declarations and extensions`
- `c65f01d` — `fix(release): verify publish tiers portably`

### BPMN consumer

Branch `refactor/bpmn-semantic-pack`, pushed at `cc924da0b7f795f7a11aa5866d27a212712c1e62`:

- `c4218b1` — `refactor(utterance): compile semantic candidates from YAML`
- `c94d1b2` — `chore(deps): pin final semantic pack API`
- `cc924da` — `chore(deps): pin qualified shared release revision`

### ob-poc consumer

Branch `refactor/semantic-pack-sources`, pushed at `3265ca31f1d01591db152713ae92c79c63ee98e5`:

- `c39fa572` — `fix: reconcile BPMN operations pack ownership`
- `7a371019` — `refactor(packs): admit semantic configuration through DSL`
- `3265ca31` — `chore(deps): pin qualified semantic pack revision`

The pre-existing `ob-poc/.cargo/config.toml.example` modification remains unstaged and untouched.

## Deterministic artifact receipts

- BPMN source SHA-256: `343fdb2fd9fa2b09e1aea4ae11f7ff869f651cb5fe34c08a1078e96eb37435a7`
- BPMN artifact SHA-256: `ca698f442c4aa7eb1a80bddcf55a280fd44dc8fe6ccc4745b00ebac459d8d1ef`
- BPMN semantic profile snapshot: `bpmn-semantic-profile-v1:a2c8a4003d3a02e765ba5b7d75b664a268e40d5ab2c39b2daa3f1e5a725316d9`
- ob-poc: 14 source/artifact receipts in `rust/config/semantic-packs.lock`; regeneration produces an empty diff.

## Verification

Shared DSL at the final revision:

- `cargo fmt --all -- --check` — pass.
- `cargo check --workspace --all-targets --all-features --locked` — pass.
- `cargo test --workspace --all-targets --all-features --locked` — pass; no failures (environment-dependent integration/model tests remain explicitly ignored).
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` — pass.
- `cargo doc --workspace --no-deps --all-features --locked` — pass.
- `bash scripts/check-domain-neutral.sh` — pass; no new host vocabulary and reviewed legacy debt unchanged.
- `bash scripts/check-layering.sh` — pass.
- `bash scripts/check-dependencies.sh` — pass.
- `bash scripts/check-packages.sh` — pass; all publishable workspace packages assembled and the crates.io-independent leaf tier passed publish dry-runs.
- `cargo deny check` — not run because `cargo-deny` is not installed locally.

BPMN against exact shared revision `c65f01d...`:

- locked `utterance-engine` check — pass.
- `cargo test -p utterance-engine --all-targets --all-features --locked` — 68 passed, 4 ignored, 0 failed across library and integration targets.
- `bash scripts/check-shared-pin.sh --self-test` and real guard — pass; all nine shared packages resolve from one exact revision with no unused-patch fallback.
- Strict Clippy remains blocked only by pre-existing dead-code warnings in `utterance-engine/src/capture.rs`; Phase 2 introduced no new warnings.

ob-poc against exact shared revision `c65f01d...`:

- locked `ob-poc-journey` all-target/all-feature check — pass.
- `cargo test -p ob-poc-journey --all-features --locked` — 26 passed, 0 ignored, 0 failed including integration tests and doctests.
- `cargo clippy -p ob-poc-journey --all-targets --all-features --locked -- -D warnings` — pass.
- semantic receipt regeneration diff — pass (empty).

## Known carry-overs into Phase 5

The new DSL artifact is the single normative foundation for new semantic configuration, but two existing SemOS policy entry points still contain application vocabulary and have not yet been cut over to the admitted artifact:

1. `sem_os_types/src/agent_mode.rs` still contains the legacy authoring/governed/business verb-prefix tables and introspection subcommand lists. `ob-poc` has production callers of the compatibility API.
2. `sem_os_policy/src/abac.rs` still infers evidence privilege from role-name substrings such as steward/compliance. That policy must become an exact pack-declared privilege/grant mapping.

Removing either in Phase 2 would have changed externally consumed behavior without first threading an admitted snapshot into the callers. Phase 5 owns that consumer migration: project mode and privilege policy from `CompiledPack`, change call sites to require the active snapshot/policy, prove parity from application-owned YAML, then delete the compatibility tables. The legacy `dsl-core` and `sem_os_policy::domain_pack` loaders likewise remain compatibility-only until that cutover; no new semantic consumer uses them.

## Gate 2 disposition

The source model, compiler, registry, public API, real BPMN/ob-poc YAML compilation, deterministic receipts and exact consumer pins are complete. SemOS can now be implemented against the admitted artifact contract.

The stronger end-state claim—“all shared SemOS production code is free of host vocabulary”—is deliberately not made at this gate. It becomes the mandatory deletion criterion in Phase 5 for the two named compatibility policies above. Per the plan's phase boundary, Phase 5 was not started in this session.
