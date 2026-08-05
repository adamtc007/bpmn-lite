# Shared-crates remediation Phase 2 baseline

**Date:** 5 August 2026
**Status:** pre-edit source-schema and concurrency receipt

## Repository ledger

| Repository | Branch | Starting HEAD | Upstream | Pre-existing dirty state |
|---|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `feat/semantic-embedder` | `5ac7da7a513744e907ca110484c3a6a9472ae985` | `origin/feat/semantic-embedder` | clean |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/sage-host-boundary` | `506e931b122014b0e2bdaf44d5ed296b2bcf7f2e` | none | clean; ignored local development patch may be present |
| `/Users/adamtc007/Developer/ob-poc` | `cleanup/retire-dsl-sage` | `4ad0e338ddbb393111d0f116bcb4d53b9ef8054d` | none | pre-existing `M .cargo/config.toml.example`; ignored root development patch may be present |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `ddd143e8258b17593ab6282742fa84e5795cdb30` | not recorded | concurrent DIR-002/model work and programme documents; preserved |

Toolchains: shared DSL and BPMN use Rust/Cargo 1.95.0; ob-poc uses Rust/Cargo 1.96.1. The shared MSRV is 1.95.

## Concurrent work ledger

The second DSL worktree is `feature/sem-os-decision-board` at `edded438f07303fd954ec2a814bf3302f30e449d`; it is an ancestor of the selected shared base and has no uncommitted overlap.

The second BPMN worktree is the coordinating DIR-002 branch. Its modified server/runtime, training bundle and programme-document files are unrelated and will not be staged.

The second ob-poc worktree is `main` at `d2afc0c49d8b2b6cea8fb83f95474c17f0d4b639`. That commit reconciles BPMN pack ownership and is required input to Phase 2. Its worktree also contains unrelated generated DSL files. Phase 2 will cherry-pick the completed commit into the clean active checkout and will not touch or stage the dirty worktree.

## Source-schema inventory

The shared workspace currently has nine crates and no semantic pack crate. Three incompatible pack mechanisms exist:

1. `dsl-core/src/config/pack_loader.rs` reads only `id`, `workspaces` and `allowed_verbs`, ignores unknown fields and warns/skips malformed files. It is unsuitable for governed semantic admission.
2. `sem_os_policy/src/domain_pack.rs` owns a large filesystem/YAML manifest with application-shaped policy fields and `anyhow` errors. It is replaced by a compiled-pack projection in Phase 5, not copied into the new foundation.
3. `utterance-engine/src/bpmn_pack.rs` is the sole source for 26 BPMN candidate specifications and includes a second hard-coded positional-argument table.

ob-poc has 14 journey pack YAML files after the completed BPMN truth commit. Their common fields include identity/version/description, invocation phrases, context/questions, workspace and allowed/forbidden verbs, risk policy, UI section layout, templates, completion/progress and handoff data. The UI/runtime fields remain application-owned; a normative semantic section is added to the same source files.

The application also owns verb YAML and SemOS seed YAML. Generated `rust/dsl-source/verbs/*.dsl` files are projections, not an additional source of truth.

## Domain-vocabulary debt

Verified shared production debt includes:

- closed `SlotType` variants in `dsl_types::constellation_map_def` and a second intent slot enum in `dsl-core::config`;
- closed `FocusTarget`/`FocusKind` variants and a viewport parser default that selects CBU;
- a CBU-specific `VerbScope`;
- hard-coded governed/business/authoring verb prefixes;
- steward/compliance role-string matching.

The shared domain-token guard already prevents new production files while allowlisting this debt. Phase 2 must reduce the allowlist as generic pack-declared identifiers and policy replace these branches.

## Compatibility surfaces

- BPMN board, snapshot, adapter-payload and workbook identities are persistent compatibility surfaces. The YAML cutover must prove byte/hash equivalence.
- ob-poc's existing journey `PackManifest` and manifest hash are widely consumed. Adding semantic metadata must not alter its legacy behavior before the Phase 5 cutover.
- `semantic-decision-contracts` already supplies generic candidate, phrase, argument, action and harm types and remains the downstream contract layer. `semantic-pack` may depend on it; the contracts crate must not depend on the compiler.
- `dsl-manifest` in BPMN is a separate federated invocation protocol and is not renamed or merged into the shared semantic pack.

## Baseline verification

The immediately preceding Phase 6 receipt records:

- shared workspace check/test gates green at `5ac7da7`;
- BPMN full test total: 92 suites, 1,335 passed, 0 failed, 6 ignored at `506e931`;
- live ob-poc Sage suite: 35 passed at `4ad0e338`;
- ob-poc full workspace check remains red for the pre-existing exact DSL v0.1.5 `VerbCrudMapping::set_values` mismatch;
- broad existing format/Clippy debt is recorded and is not silently attributed to Phase 2.

Phase 2 will establish focused red/green receipts for every changed package, then repeat full gates at its ending revisions.

## Stop-condition audit

Licensing is consistent (MIT), the shared MSRV is supported by both consumers, and no database or deployment migration is required. No concurrent branch is modifying the selected shared public contracts. Persistent BPMN hashes are identifiable and testable. No charter stop condition is active at the baseline.
