# Shared-crates remediation Phase 5 baseline

**Date:** 5 August 2026
**Phase:** 5 — move host semantics from SemOS Rust into YAML packs

## Selected worktrees

| Repository | Branch | Starting HEAD | Status |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `refactor/sem-os-pack-policy` | `c65f01d514c99bf087673ce366ed3b7549217c1d` | clean |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-policy-consumer` | `3265ca31f1d01591db152713ae92c79c63ee98e5` | pre-existing `M .cargo/config.toml.example` only |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `4490426e5c1edcca34810d27611ef062f918a504` | unrelated runtime/training/`.DS_Store` changes; documentation only |

The separate BPMN semantic-pack worktree is already complete at `cc924da0b7f795f7a11aa5866d27a212712c1e62` and is not a Phase 5 source target. The dirty `ob-poc-bpmn-pack-truth` worktree remains untouched.

## Toolchains

- shared DSL: `rustc 1.95.0`, Cargo `1.95.0`, `1.95-aarch64-apple-darwin` from `rust-toolchain.toml`;
- ob-poc: `rustc 1.96.1`, Cargo `1.96.1`, `1.96-aarch64-apple-darwin` from `rust-toolchain.toml`.

## Baseline findings

The Phase 2 artifact is available and consumers are exactly pinned, but host policy still exists in production Rust:

- `sem_os_types/src/agent_mode.rs` owns authoring/governed/business command-prefix tables, introspection command lists and mode feature booleans;
- `sem_os_policy/src/abac.rs` derives evidence privilege from role-name substrings;
- `sem_os_policy/src/stewardship/mod.rs` embeds `admin`/`steward` change grants;
- `sem_os_core/src/ids.rs` embeds the ob-poc SemReg UUID namespace bytes;
- `ob-poc/src/agent/verb_surface.rs` embeds safe-harbor, no-group and workflow-domain allowlists;
- ob-poc retains a compatibility copy of the SemReg UUID namespace in `src/sem_reg/ids.rs`;
- seven `sem_os_policy::domain_pack` tests remain ignored because they require the ob-poc checkout, and the shared workspace still has the ignored `config` symlink into ob-poc.

The production `AgentMode` gate has one central root-application caller (`compute_session_verb_surface`) plus application status/introspection callers. `CoreServiceImpl` has eight application construction sites. These are real compatibility surfaces, so removing shared methods requires an application policy adapter and explicit snapshot injection in the same phase.

The current 14 journey semantic sources describe journey selection and carry legacy allowed/forbidden verb lists in bounded extensions. They do not contain the global mode, workflow, role, privilege, UUID or fallback policy. Phase 5 therefore adds one application-owned policy pack rather than overloading a journey pack.

## Baseline verification

`cargo test -p semantic-pack -p sem_os_types -p sem_os_policy --all-targets --all-features --locked` passed: 381 passed, 7 ignored, 0 failed. The ignored tests are the host-checkout coupling this phase must remove or relocate.

No baseline file, source hash, UUID namespace, permission decision or caller was modified while recording this receipt.
