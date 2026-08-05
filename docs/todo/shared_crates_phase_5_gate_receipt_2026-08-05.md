# Shared-crates remediation Phase 5 gate receipt

**Date:** 5 August 2026
**Scope:** Phase 5 only — move host semantic policy from shared Rust into an admitted application YAML pack
**Gate result:** PASS, with the carry-overs below. Phase 6 and Phase 7 were not started.

## Repository receipt

| Repository | Branch | Starting HEAD | Ending source HEAD | Pushed commits |
|---|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `refactor/sem-os-pack-policy` | `c65f01d514c99bf087673ce366ed3b7549217c1d` | `9b76c951a084cca6af4885609d46f8dc02637b00` | `f0e2552`, `0d44808`, `9b76c95` |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-policy-consumer` | `3265ca31f1d01591db152713ae92c79c63ee98e5` | `ec0ba7ddfe4100520a151c58ab9edbef11d45437` | `e19b3d72`, `ec0ba7dd` |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `4490426e5c1edcca34810d27611ef062f918a504` | `4490426e5c1edcca34810d27611ef062f918a504` before this documentation-only receipt | coordinating documentation only |

All source commits above were pushed to their named branches. The shared consumer revision is exactly `9b76c951a084cca6af4885609d46f8dc02637b00`; the shared crates remain version `0.2.0` and require Rust 1.95.

## Result

The intended boundary is now enforced for the Phase 5 policy surface:

```text
ob-poc YAML platform pack
        │ public semantic-pack admission
        ▼
immutable SemanticSnapshot
        │ typed generic policy API
        ▼
sem_os_policy mechanisms
        │ decisions + evidence + stable IDs
        ▼
ob-poc composition and technical adapters
```

The shared crates no longer own `ob-poc` mode allowlists, workflow command-prefix tables, safe-harbor/no-group tables, steward/admin grant tables, evidence role interpretation, or the persistent SemReg UUID constant. `AgentMode` retains only its stable value, parsing, display, default and serialization contract.

`ob-poc` now owns `rust/config/semantic-packs/platform-policy.yaml`. It is compiled through the same `semantic-pack` schema and admission path as every other semantic pack; there is no second YAML dialect or loader. The application adapter is a thin typed composition layer over `sem_os_policy::pack_policy`, not a duplicate Rust decision table.

## Public modules and contracts

Created in the shared workspace:

- `sem_os_policy::pack_policy`, exposing typed principal context, capability decisions, reasons/evidence, privilege and attribute lookup, identity namespace lookup, and capability-adapter registration;
- additive `semantic-pack` source/artifact fields for identity namespace, eligibility contexts, typed exact/prefix selectors, policy attributes and named privileges;
- public `dsl-core` green-when coverage rows, summaries and functions so consumers can qualify their own DAG corpus without private imports.

Created in `ob-poc`:

- crate `ob-poc-semantic-policy`;
- application pack `config/semantic-packs/platform-policy.yaml`;
- active application qualification suite `tests/domain_pack_config_qualification.rs`.

Compatibility-preserved interfaces:

- `AgentMode` enum/serde/default/display/parse;
- `sem_reg::ids::object_id_for` and `definition_hash` call signatures;
- ABAC re-export surface, now accepting the typed shared evidence privilege;
- `CoreServiceImpl` remains the shared service, now constructed with an immutable `SemanticSnapshot`.

Deleted from shared tests:

- ignored `dsl-core` tests that loaded the `ob-poc` verb, DAG, domain-pack and constellation corpus;
- ignored `dsl-integration-tests` suites for the `ob-poc` Lux SICAV/shape-rule/resolver corpus.

Their previously dormant application-level coverage is replaced by active `ob-poc` pack, verb-catalogue, DAG, predicate, ownership, constellation and template qualification. Generic parser and coverage assertions remain active and self-contained in `dsl-core`.

## Dependency graph

Before:

```text
ob-poc ──► shared AgentMode/ABAC/stewardship/ID helpers
                  └── embedded ob-poc policy and UUID bytes
ob-poc ──► local patches / mixed shared revisions
```

After:

```text
ob-poc YAML ──► semantic-pack 0.2.0 ──► SemanticSnapshot
                                                  │
ob-poc-semantic-policy ──► sem_os_policy 0.2.0 ◄──┘
          │
ob-poc composition roots and adapters

all shared DSL/SemOS policy crates ──► exact git revision
9b76c951a084cca6af4885609d46f8dc02637b00
```

Cold resolution was run with the repository-local patch file temporarily absent and automatically restored. `Cargo.lock` records the Git sources at the exact final revision, and a second `--locked` check plus `cargo metadata --locked` succeeded.

## Compatibility and hash receipt

- Legacy identity namespace bytes: `7a3b9f42-e1d4-5a8b-910c-4f2d6e8a1b3c` — unchanged.
- Golden object ID for `verb_contract:kyc.resolve_ubo`: `0058fae8-e8bf-51b5-bef5-f9db54637fdd` — unchanged.
- Golden canonical definition fingerprint for `{"a":1,"b":2}`: `ebe76008-f2c0-5048-b9dc-0417d6ac3b74` — unchanged.
- Existing optional-empty pack fields remain omitted from canonical encoding; the shared canonical hash golden tests pass.
- Existing 14 journey pack receipts remain reproducible. The new application policy is the fifteenth receipt:
  - source SHA-256 `0197f2b79bff5115f2e64aa537c429f4fcb266f614fe170d1f0f5aa02c5bf2fc`;
  - artifact SHA-256 `27c21684ca0cf3d52e0d7160647452f771f5c47545c36076a8796ee633517cd9`.

## Verification commands

| Command | Result |
|---|---|
| Shared baseline focused test | 381 passed, 0 failed, 7 ignored host-dependent tests |
| `cargo test --workspace --all-targets --all-features --locked` in `dsl` | 914 passed, 0 failed, 1 ignored across 27 summaries |
| `rg '#\[ignore' crates --glob '*.rs'` in `dsl` | one result only: the external-model `SEMANTIC_EMBEDDER_TEST_BUNDLE` test; zero host-checkout ignores |
| `cargo test -p dsl-core --all-targets --all-features` | 412 passed, 0 failed, 0 ignored |
| `cargo test -p ob-poc-semantic-policy --all-targets` | 4 passed, 0 failed, 0 ignored |
| `cargo test -p ob-poc-journey --test semantic_pack_sources` | 2 passed, 0 failed, 0 ignored |
| `cargo test -p ob-poc --test domain_pack_config_qualification` | 8 passed, 0 failed, 0 ignored |
| `cargo test -p ob-poc --lib` | 1,814 passed, 0 failed, 214 ignored environment/database tests |
| `cargo test -p ob-poc sem_reg::ids::tests` | 2 passed, 0 failed; 2,026 filtered |
| all-target checks for `sem_os_server`, `ob-poc-web`, `sem_os_harness`, `sem_os_obpoc_adapter`, `sem_os_postgres`, `dsl-runtime`, `ob-poc-journey`, `ob-poc-semantic-policy` | passed |
| cold `cargo check -p ob-poc-semantic-policy`, repeated with `--locked`, then `cargo metadata --no-deps --locked` | passed from Git revisions with local patches absent |
| `cargo fmt --all --check` in shared workspace and `rustfmt --check` for new application files | passed |
| `cargo fmt --all --check` across all of `ob-poc` | not a Phase 5 gate: it reports broad pre-existing formatting drift outside this change |

## Gate 5 assessment

- Shared SemOS production policy is generic and snapshot-driven: pass.
- One compiled artifact and no second YAML loader/schema: pass.
- Existing permissions and identity bytes preserved: pass for the migrated deterministic vectors and active policy parity tests.
- BPMN continues to own and compile its own pack without an `ob-poc` adapter: pass; no BPMN source changed in this phase.
- Application technical implementations remain registered in application crates; duplicate policy tables were removed: pass.
- Shared tests run without an `ob-poc` checkout or ignored host-dependent test: pass.

## Known carry-overs

1. **Restore one-for-one application macro/shape-rule regression vectors.** The shared copies were all ignored and therefore supplied zero CI protection. Active consumer tests now cover pack admission, ownership, catalogue invariants, DAG parsing, predicates, constellation loading and template harnesses, but the old Lux/macro fact baselines were not copied one-for-one. Owner: `ob-poc` DSL/configuration. Target: Phase 7 consumer release gate.
2. **Improve authored `green_when` coverage.** Current qualified floors are 1,253 verbs, 12 non-empty predicates, and 6 covered candidate states. Dormant historical assertions expected at least 1,270 verbs, 17 predicates and 9 covered candidates. Treat this as visible corpus debt, not a regression caused by this refactor. Owner: `ob-poc` domain-pack maintainers. Target: before production promotion.
3. **Run database-backed and deployment qualification.** The 214 ignored `ob-poc` library tests require database or environment services. No database migration, shadow deployment or production rollout was authorized or performed. Owner: application release. Target: Phase 8 deployment/rollback gate.
4. **Run the external-model inference test.** The sole shared ignored test requires `SEMANTIC_EMBEDDER_TEST_BUNDLE`; Phase 5 did not alter the embedder or model. Owner: model release. Target: the model promotion gate.
5. **Align the embedder and narrow duplicate BPMN dependencies.** `semantic-embedder` remains deliberately pinned to its existing revision, and the cold lock still contains both BPMN tag and feature-branch revisions. Owner: shared release/consumer cutover. Target: Phase 7.
6. **Remove residual host-shaped examples and comments from shared crates.** Generic shared production algorithms no longer contain host policy, but some test fixtures, documentation examples and host-adapter error text still use `ob-poc`-shaped names. Owner: shared crate maintainers. Target: cleanup after Phase 7 compatibility proof.
7. **Resolve repository-wide formatting debt in `ob-poc`.** New files are formatted and all changed code compiles, but a full-workspace format check would rewrite many unrelated files. Owner: `ob-poc` maintainers. Target: separate mechanical change.
8. **Tag and release only after consumer review.** This phase pins a pushed immutable revision; it does not create a new shared release tag or begin Phase 7 dependency narrowing. Owner: shared release owner. Target: Phase 7.

## Deployment, inference and rollback

No model inference comparison was required because no model or serializer changed. No application shadow deployment was performed because this phase changed source/configuration boundaries only and authorized no deployment. Safe source rollback points are shared `c65f01d514c99bf087673ce366ed3b7549217c1d` and `ob-poc` `3265ca31f1d01591db152713ae92c79c63ee98e5`; rollback was not applied to a deployed environment because no environment was changed.

## Worktree protection

The pre-existing `ob-poc/.cargo/config.toml.example` modification was not staged or committed. The local ignored `.cargo/config.toml` was restored after each cold-resolution check. Existing `bpmn-lite` runtime, training-output and `.DS_Store` changes were not staged or committed. No source in the separate BPMN semantic-board worktree or the dirty BPMN pack-truth worktree was changed.
