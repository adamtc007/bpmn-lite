# Shared crates standalone remediation — Phase 9 readiness receipt

**Date:** 5 August 2026

**Status:** consumer preparation complete; compatibility deletion deferred

**Shared release:** `v0.2.1` / `586431f81e2bb9101578af5167b8a35335f5a09e`

## Decision

Phase 9's destructive cleanup precondition is not met. The two application
release candidates have not been promoted to an external non-production
environment and the rollback window has therefore neither started nor closed.
Deleting public shims from the immutable `v0.2.1` contract would contradict the
plan and remove the exact API required by the qualified rollback artifacts.

The safe preparation work is complete. `ob-poc` now imports foundational
SemOS values from `sem_os_types` and constellation-map values from `dsl_types`
instead of their transitional parent modules. The shared compatibility shims
remain published, BPMN keeps its explicit old/new type-identity sentinel, and
every remaining deletion has an owner and release condition below.

Gate 9 is **not declared complete**. This receipt is the evidence-backed
readiness point from which the cleanup release can be cut after Gate 8 and the
rollback window close.

## Repository ledger

| Repository | Branch | Phase 9 readiness head | State |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `refactor/sem-os-pack-policy` | `586431f81e2bb9101578af5167b8a35335f5a09e` / `v0.2.1` | unchanged, clean and pushed |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/bpmn-semantic-pack` | `d598d7e3c0eda7bac1e1379af2d635bca7bfeca2` | unchanged, clean and pushed |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-policy-consumer` | `d84915f6` | canonical-import migration committed and pushed |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | this receipt commit | coordination documents only |

The pre-existing `ob-poc/.cargo/config.toml.example` modification and the
pre-existing coordinating-worktree changes were not staged or committed.

## Consumer preparation delivered

Commit `d84915f6` (`refactor: consume canonical SemOS leaf types`) changes
`ob-poc` without changing runtime values or serialized representations:

- replaces all `sem_os_core::types` imports with `sem_os_types`;
- replaces all `sem_os_ontology::constellation_map_def` imports with the
  `dsl_types` crate-root API;
- declares the direct leaf dependencies in every affected package;
- refreshes the lockfile under exact Git resolution and removes redundant
  Windows support-version edges selected by the previous lock graph; and
- preserves `sem_os_core` and `sem_os_ontology` wherever their non-compatibility
  APIs are still used.

The source search now finds zero uses of either deprecated import path in
`ob-poc`, including tests. BPMN production source already used the direct
`semantic-decision-contracts` API.

## Compatibility surface inventory

| Surface | Current evidence | Readiness decision |
|---|---|---|
| `sem_os_core::types::*` | Canonical definitions live in `sem_os_types`; both reviewed consumers now have zero imports of the shim. | Eligible for removal only in the post-window breaking shared release. |
| `sem_os_ontology::constellation_map_def::*` | Canonical public values are exported by `dsl_types`; both reviewed consumers now have zero imports of the shim. | Eligible for removal only in the post-window breaking shared release. |
| `sem_os_policy::decision_board` | No BPMN production import remains. One focused BPMN test intentionally proves old/new `TypeId` identity. | Retain the shim and sentinel through rollback; delete both together afterwards. |
| `CandleEmbedder::embed` and `embed_batch` | Deprecated in favour of explicit query/target methods; no reviewed direct `CandleEmbedder` consumer requires these aliases. | Eligible for post-window breaking removal after one final downstream search. |
| `CandleEmbedder::new` and pinned default model constants | Both consumers actively use the constructor. It is not marked deprecated and resolves a pinned upstream model revision. | Retain as supported API until applications are configured with deployment-owned bundle paths. |
| `sem_os_policy::gates::GateSeverity` | Crate-private compatibility import, not an application API. | Fold into direct internal imports in the cleanup release; no consumer migration required. |
| `POST /api/dsl/sage/utter` | BPMN still exposes and tests the route. It is an application wire contract, not shared-crate ownership. | Remove only through a separately versioned API deprecation after caller telemetry is empty. |
| v1 board/evidence/workbook readers and hashes | Phase 8 rollback depends on unchanged persisted and wire semantics. | Retain for the declared persistence retention period; any v2 hash needs an explicit dual-read migration. |

No old host-specific application policy variant was found that can safely be
deleted from the shared release independently of these contracts. The
domain-vocabulary allowlist remains a reviewed debt register, not permission to
add new host semantics.

## Verification receipt

### Shared workspace at the immutable release

All commands ran from a clean shared checkout:

- `cargo fmt --all -- --check` — pass;
- locked workspace check for all targets and features — pass;
- locked workspace Clippy for all targets/features with `-D warnings` — pass;
- locked full workspace tests — pass, with the model-bundle test explicitly
  ignored because no `SEMANTIC_EMBEDDER_TEST_BUNDLE` was supplied;
- rustdoc for all features with `-D warnings` — pass;
- layering, metadata dependency, and domain-neutrality guards — pass;
- workspace package guard — pass for all publishable packages; and
- crates.io publish dry-runs — pass for `dsl_types`, `sem_os_types`, and
  `semantic-decision-contracts`.

The dependency guard proves that the shared packages contain no `ob-poc` or
`bpmn-lite` source dependency and that every workspace edge follows the
declared capability layering.

### `ob-poc` consumer

- exact-Git locked metadata resolves all eight shared packages from
  `586431f81e2bb9101578af5167b8a35335f5a09e`;
- exact-Git locked workspace check for all targets/features — pass;
- affected capability suites — 489 passed, 0 failed, 28 database-dependent
  tests explicitly ignored;
- full `ob-poc` library suite — 1,814 passed, 0 failed, 214 explicitly
  ignored;
- changed-file formatting and `git diff --check` — pass; and
- workspace-wide formatting remains red only on the broad pre-existing
  baseline recorded in earlier receipts.

The ignored local Cargo patch configuration was temporarily disabled for every
exact-revision command and restored unchanged afterwards. The committed
lockfile contains no absolute local path.

### BPMN consumer

- shared-pin guard self-tests — all deliberate violation fixtures detected;
- real shared-pin guard — pass, one exact shared revision and no unused patch
  fallback;
- focused old/new contract identity test — 1 passed, 0 failed; and
- production search — no deprecated shared contract/type import.

The only old decision-board path is in the compatibility sentinel itself.

## Gate 9 assessment

| Gate | Result |
|---|---|
| Searches and Cargo metadata find no forbidden reverse dependency | pass |
| Application production source imports no deprecated shared path | pass; the BPMN sentinel test is intentionally retained |
| Documentation matches the current ownership layout | pass in the programme documents; stale in-source phase labels are carried below |
| Clean exact-revision reproducibility | pass for shared and both consumers |
| Every remaining item has an owner and intended release | pass in the ledger below |
| Both consumers shipped and rollback window closed | **not met** |

The unmet precondition prevents public compatibility deletion and therefore
prevents Gate 9 closure even though the technical readiness checks pass.

## Required deletion sequence after rollback closure

1. Freeze the deployed consumer revisions and record the rollback-window close
   approval.
2. Repeat downstream searches across both deployed revisions and any external
   crates registered by the release owner.
3. Cut a new breaking shared release (earliest compatible target: `v0.3.0`),
   rather than mutating `v0.2.1`.
4. Remove `sem_os_core::types`,
   `sem_os_ontology::constellation_map_def`,
   `sem_os_policy::decision_board`, and the deprecated embedding aliases in
   one separately reviewable cleanup series.
5. Delete the BPMN compatibility sentinel only with the decision-board shim.
6. Rerun the permanent matrix, update both exact Git pins and lockfiles, then
   repeat semantic replay and rollback qualification.

## Carry-over ledger

| ID | Carry-over | Owner | Intended release / closure condition |
|---|---|---|---|
| P9-01 | Gate 8 has no external registry, non-production target, captured traffic source, tolerance approval, or dashboard destination. | release/platform owner | before external shadow and before the rollback clock starts |
| P9-02 | **Resolved locally:** `ob-poc` commit `1b852343` adds the PG18 clean-bootstrap contract, reconciles canonical artifacts, adds CI, and binds the schema hash into the RC receipt. | `ob-poc` persistence/schema owner | closed 2026-08-05; see `shared_crates_database_bootstrap_receipt_2026-08-05.md` |
| P9-03 | The three public shared compatibility modules and two deprecated embed aliases remain. | shared-crate API owner | breaking shared release after both consumers ship and rollback closes; earliest `v0.3.0` |
| P9-04 | Both consumers use `CandleEmbedder::new`, which resolves the pinned default bundle rather than a deployment-owned local bundle. | model/deployment owners | model deployment cutover release; retain constructor until both consumers migrate |
| P9-05 | BPMN retains `/api/dsl/sage/utter`. | BPMN API owner | separately versioned API removal after external caller telemetry is empty |
| P9-06 | Persisted v1 hashes/readers have no approved retirement period or v2 migration. | semantic-contract and data-retention owners | explicit schema/hash migration release, independent of source shim removal |
| P9-07 | The shared domain-token allowlist still contains reviewed host-shaped examples, fixtures, comments, and generic governance vocabulary. | shared DSL/SemOS maintainers | classify and shrink in `v0.3.0`; semantic behaviour may move only with parity tests |
| P9-08 | In-source migration comments refer to older Phase 9/12 numbering. | shared documentation owner | correct alongside the cleanup release so the immutable `v0.2.1` source remains traceable |
| P9-09 | `ob-poc` full-workspace formatting debt and `block 0.1.6` future-incompatibility warning remain. | `ob-poc` maintenance owner | next Rust/dependency hygiene release |
| P9-10 | BPMN broad formatting/Clippy debt remains outside this programme diff. | BPMN maintenance owner | separate hygiene release |

## Preservation and conclusion

No compatibility module, persisted reader, old hash, route, fixture, or release
artifact was deleted. No external deployment was inferred or performed. The
shared `v0.2.1` release remains immutable and rollback-capable.

Phase 9 is now prepared, not complete: consumer production source uses the
canonical capability crates, every permanent local gate is green, and the
remaining deletion is bounded by an explicit external-deployment condition.
