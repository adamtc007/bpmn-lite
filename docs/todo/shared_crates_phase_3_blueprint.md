# Shared-crates remediation Phase 3 blueprint

**Phase:** 3 — extract `semantic-decision-contracts`
**Date:** 5 August 2026
**Shared base:** DSL `7d4cc10a903af93b3e7fc243dc2dfda3977050c5` on `feat/semantic-decision-contracts`
**BPMN base:** `43dcbcacb1e40396b96be1eb3c401e3e32b9900b` on `refactor/semantic-decision-contracts`

## Invariants and absolute boundaries

1. This is a type-ownership move. JSON field names, enum spellings, collection ordering, validation, transition rules, SHA-256 preimages and v1 digest bytes must not change.
2. `semantic-decision-contracts` is a safe, host-neutral leaf crate. Its production dependencies are limited to `serde`, `sha2`, `hex` and `thiserror`; it must not depend on DSL, SemOS, Tokio, YAML, SQL, tracing, persistence or either application.
3. `ActionClass` and `HarmClass` move into the contract leaf because candidate contracts contain them. `sem_os_ontology::verb_contract` publicly re-exports the exact same types for compatibility; no duplicate enum is introduced.
4. The complete existing `sem_os_policy::decision_board` implementation moves without algorithmic edits. That former module becomes a documented compatibility re-export for one deprecation window.
5. Old and new import paths must have identical Rust `TypeId`s. Compatibility is not implemented through conversion wrappers.
6. Existing v1 hash algorithms continue to emit exactly the captured baseline hashes. Hash v2 and the separate unframed ACP projection digest are not changed in this extraction; v2 requires a distinct migration sub-phase and an owner-approved pending-record rule.
7. BPMN-specific BLAKE3 serializer identities remain untouched and distinct from the shared SHA-256 contract hashes.
8. No `ob-poc` application source, database schema, model bundle, YAML pack or deployment file changes in this phase.
9. BPMN switches its decision-contract imports to the new crate, while a focused compatibility test retains and verifies the old path.
10. The BPMN shared-pin guard must include the seventh shared package and prove all seven resolve from one immutable DSL revision. Local path patches remain development-only.

## Shared crate structure

```text
crates/semantic-decision-contracts/
  Cargo.toml
  README.md
  src/lib.rs
```

`src/lib.rs` owns:

```text
ActionClass, HarmClass
CanonicalCandidateId, DomainIdentity, SnapshotIdentity, GraphRevision, WorkbookId
BoardHash, EvidenceRecordHash
PhraseRole, PhraseEvidence
ArgumentKind, ArgumentSpec, NegativeContrast, CandidateSemanticSlice
ResolvedPosition, SemanticDecisionBoard
EvidenceLane, FiniteScore, LaneScore, CandidateEvidence, InferenceEvidence
DispositionPolicy, InferenceDisposition, DecisionRecord, decide
ProposalStatus, SlotRequirement, SlotValue, SlotValueState
BindingProvenance, WorkbookSlot, ProposalWorkbook
DecisionBoardError
```

The current constructors, custom deserializers, canonical sort/dedup logic, completeness checks and closed workbook transition table move with those types.

## Compatibility seams

`sem_os_policy/src/decision_board.rs` becomes:

```rust
#[deprecated(note = "import semantic_decision_contracts directly")]
pub use semantic_decision_contracts::*;
```

The module itself remains available through `sem_os_policy::decision_board`; individual re-exported items are not marked deprecated in this phase to avoid flooding existing consumers with warnings under `-D warnings`.

`sem_os_ontology/src/verb_contract.rs` publicly re-exports `ActionClass` and `HarmClass` from the new leaf. Existing YAML serialization and imports therefore remain byte- and source-compatible.

## Golden and compatibility tests

Carry the existing decision-board tests into the new crate unchanged, then add fixed baseline assertions for:

- one populated BPMN-shaped board JSON value and its v1 board hash;
- its complete evidence JSON value and v1 evidence hash;
- invalid/tampered board and evidence hashes;
- duplicate candidate identities and evidence lanes;
- non-finite score deserialization;
- illegal workbook transitions;
- deterministic candidate and evidence ordering;
- old/new path `TypeId` equality and cross-path function acceptance.

The golden values are captured from the pre-extraction implementation and then asserted after the move. No test regenerates its expected digest from the implementation under test.

## Workspace and release changes

- Add the crate as workspace member/version `0.2.0` with inherited MIT, edition 2021 and MSRV 1.95 metadata.
- Add it as a workspace dependency of `sem_os_ontology`, `sem_os_policy` and the integration-test crate where needed.
- Extend metadata/layering, package, documentation and CI feature/leaf checks.
- Update README/CHANGELOG/versioning documentation with the new capability boundary.
- Keep the library release graph acyclic; the new crate is a first-tier publish dry-run.

## BPMN consumer cutover

- Add a root workspace dependency pinned to the exact new DSL commit.
- Replace production `sem_os_policy::decision_board` imports with `semantic_decision_contracts` in `utterance-engine` and the designer server.
- Keep `sem_os_policy` as a dev-only compatibility dependency for the focused old/new path test during the deprecation window.
- Update fuzz manifests and lockfiles where fuzz targets import the contract types.
- Extend `scripts/check-shared-pin.sh`, all self-test fixtures and its success receipt from six to seven packages.
- Run the pin guard self-test before the real guard.

## Gate commands

Shared workspace:

```text
cargo fmt --all -- --check
cargo check -p semantic-decision-contracts --all-targets --locked
cargo test -p semantic-decision-contracts --all-targets --locked
cargo test -p sem_os_policy --test decision_board_compat --locked
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
bash scripts/check-layering.sh
bash scripts/check-dependencies.sh
bash scripts/check-domain-neutral.sh
bash scripts/check-packages.sh
cargo-deny check
```

BPMN consumer:

```text
bash scripts/check-shared-pin.sh --self-test
bash scripts/check-shared-pin.sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo check --manifest-path utterance-engine/fuzz/Cargo.toml --bins --locked
cargo check --manifest-path bpmn-lite-server-designer/fuzz/Cargo.toml --bins --locked
```

Phase 3 stops at this gate. No Phase 4 embedder work begins until the extraction receipt is reviewed.
