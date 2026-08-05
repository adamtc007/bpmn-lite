# Shared-crates remediation Phase 3 receipt

**Phase:** 3 — extract `semantic-decision-contracts`
**Date:** 5 August 2026
**Result:** shared extraction complete and pushed; BPMN consumer cutover verified and committed

## Delivered boundary

The shared DSL workspace now contains a standalone `semantic-decision-contracts` leaf crate at version `0.2.0`, licensed MIT and requiring Rust 1.95. Its only production dependencies are `serde`, `sha2`, `hex` and `thiserror`. It owns the complete semantic decision-board, evidence, disposition and proposal-workbook contract surface, including `ActionClass` and `HarmClass`.

`sem_os_policy::decision_board` remains a compatibility re-export for one deprecation window. `sem_os_ontology::verb_contract` re-exports the same action and harm types. Compatibility tests prove that the old and new paths have identical Rust `TypeId`s and can be used across the boundary without adapters or conversions.

The BPMN consumer now imports decision contracts directly from `semantic-decision-contracts` in `utterance-engine`, the designer server and their fuzz targets. `sem_os_policy` remains only as a development dependency for a focused compatibility test. All seven shared packages resolve from the same immutable DSL Git revision.

## Repository and commit ledger

| Repository | Branch | Phase commit | Publication state |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `feat/semantic-decision-contracts` | `ca006a785e1545cf71e2870c4dffe9d7bb5147e8` — `feat: extract semantic decision contracts` | pushed to `origin/feat/semantic-decision-contracts` |
| `/Users/adamtc007/dev/dsl` | Phase 1 ancestor | `7d4cc10a903af93b3e7fc243dc2dfda3977050c5` — `ci: establish standalone shared-crate gates` | included in the pushed branch history |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/semantic-decision-contracts` | `0bf5c052b3b36548543747f15d05948202483811` — `refactor: consume semantic decision contracts` | committed locally; not pushed by this phase |

No `ob-poc` application source, database schema, model bundle, YAML pack or deployment file changed.

## Frozen compatibility evidence

The extraction preserves the existing v1 serialization and SHA-256 algorithms. Tests assert fixed, pre-extraction golden values rather than deriving expected output from the implementation under test:

- board hash: `ce7c20d907063b3c0219a7255f58865ff96c44e873e8b3ba03a2c4d47cf79de3`;
- evidence hash: `bbff36362654e2367d1ead8533532f112f97374cad78a78db0c6dc964be253a8`;
- populated board and evidence JSON: fixed compact golden documents;
- old/new path identity: exact `TypeId` equality and cross-path function acceptance.

Hash v2 and the separate unframed ACP projection digest were intentionally not introduced. They require a separately approved migration and pending-record compatibility rule. BPMN-specific BLAKE3 serializer identities are unchanged.

## Verification receipt

### Shared DSL workspace

| Command or gate | Outcome |
|---|---|
| formatting, locked all-target/all-feature check | pass |
| workspace Clippy with `-D warnings` | pass |
| workspace tests | pass |
| `semantic-decision-contracts` tests | pass — 16 unit tests |
| `semantic-decision-contracts` doctests | pass — 28 doctests |
| `sem_os_policy` old/new compatibility tests | pass — 2 tests |
| rustdoc with `-D warnings` | pass |
| layering, dependency, domain-neutral and package gates | pass |
| `cargo-deny 0.20.2 check` | pass; existing warning-only duplicate `wit-bindgen` versions remain |

### BPMN consumer

| Command or gate | Outcome |
|---|---|
| shared-pin guard self-tests | pass |
| real shared-pin guard | pass — all seven packages at `ca006a785e1545cf71e2870c4dffe9d7bb5147e8`; no unused-patch fallback |
| locked workspace check, all targets and features, using the exact Git pin | pass |
| locked workspace tests, all targets and features, using the exact Git pin | pass |
| focused utterance-engine compatibility test | pass |
| focused utterance-engine and designer tests | pass |
| utterance-engine fuzz-bin locked check | pass |
| designer-server fuzz-bin locked check | pass |
| `git diff --check` | pass |

The BPMN repository's full formatting and Clippy gates are not newly green. Formatting has pre-existing unrelated drift, and Clippy reports pre-existing findings in compiler lowering plus intentionally unused capture helpers. The Phase 3 change did not suppress or broaden those lints, and the shared crate itself passes the strict Clippy gate. These baseline debts remain explicit carry-overs rather than being folded into this type-ownership extraction.

## Dependency result

The BPMN root, workspace lockfile and both affected fuzz lockfiles resolve the shared source at the exact immutable commit `ca006a785e1545cf71e2870c4dffe9d7bb5147e8`. The fuzz lockfiles shrink because direct use of the leaf contract crate no longer pulls the SemOS policy dependency closure into those fuzz projects.

Local path patches remain opt-in, repository-local and ignored. Exact-Git verification temporarily disabled the local patch file and restored it afterward; no global Cargo configuration was introduced.

## Carry-overs

1. Decide and implement canonical hash v2 only with an explicit migration plan for stored/pending records.
2. Remove the `sem_os_policy::decision_board` compatibility export after the documented deprecation window and consumer inventory is empty.
3. Repair the BPMN baseline formatting and Clippy debt in a separate, reviewable maintenance change.
4. Continue to Phase 4, the pure semantic embedder extraction, only after this phase gate is reviewed and explicitly approved.
5. Reconcile the warning-only duplicate `wit-bindgen` dependency when the upstream WASI graph converges.

## User-work preservation

The coordinating BPMN checkout's pre-existing `.DS_Store`, `bpmn-lite-server-runner/src/bus_runtime.rs`, model-training receipts and `docs/.DS_Store` were not modified, staged or reverted. The concurrent DIR-002 implementation branch was not used as the consumer cutover base.
