# Shared-crates remediation Phase 1 blueprint

**Phase:** 1 — standalone package and CI discipline
**Date:** 5 August 2026
**Shared implementation branch:** `refactor/standalone-phase1` from DSL `edded43`
**BPMN configuration branch:** `refactor/shared-crates-phase1-config` from BPMN `745b4ea`

## Confirmed owner rulings

- Licence: MIT.
- Shared crate version: `0.2.0`.
- Minimum supported Rust version: `1.95`.
- BPMN consumer base: semantic-board `main` at `745b4ea`, not the concurrent DIR-002 training branch.

## Invariants and absolute boundaries

1. This phase changes packaging, CI, safety, and developer resolution only. It does not move capabilities, change serialised contracts, change hashes, or alter application semantics.
2. All shared packages inherit version `0.2.0`, edition 2021, MSRV 1.95, repository, and MIT metadata from the workspace. Edition 2021 is preserved to avoid an unrelated source migration.
3. The MIT text is repository-wide. Git history is the authorship record; Cargo `authors` metadata is deliberately omitted rather than inventing a legal identity.
4. Pure library roots use `#![forbid(unsafe_code)]`. The unused raw-pointer traversal is deleted; no replacement unsafe block is introduced.
5. Public API changes are limited to deleting the uncalled `pub(crate)` function `find_unresolved_refs`. No external symbol is removed.
6. Existing host vocabulary is baseline debt for Phase 2/5. The new neutrality gate records an explicit file allowlist and rejects new production files containing forbidden host terms.
7. Cargo dependency validation uses resolved metadata and package IDs, not source grep alone.
8. User-global DSL/BPMN patches are removed. Equivalent opt-in patches live only in gitignored repository-local configs used for active co-development.
9. No committed consumer manifest or lockfile is changed by the local patch move.
10. Existing ignored host-fixture tests remain visible debt; they are not falsely described as standalone coverage.

## File and module plan

### Shared repository root

- `README.md`: capability statement, dependency direction, build/use examples, release and lockfile policy.
- `LICENSE`: canonical MIT text.
- `CHANGELOG.md`: `0.2.0` standalone-foundation entry.
- `Cargo.toml`: `[workspace.package]`, versioned local dependency declarations, workspace Rust/Clippy lints.
- `deny.toml`: licence, advisory, duplicate, and source policy.
- `docs/versioning.md`: SemVer, MSRV, Git release, lockfile, and compatibility rules.

### Crates

Every package manifest inherits workspace metadata and points at its crate README. Each library root begins with:

```rust
#![forbid(unsafe_code)]
```

`dsl-core/src/ast.rs` deletes this complete unused boundary:

```rust
pub(crate) fn find_unresolved_refs(program: &Program) -> Vec<&AstNode>;
```

The supported safe API remains:

```rust
pub fn find_unresolved_ref_locations(program: &Program) -> Vec<UnresolvedRefLocation>;
```

### Validation scripts

`scripts/check-dependencies.sh` contains:

```text
workspace_packages()
assert_allowed_workspace_dependencies(package, allowed...)
assert_no_host_sources()
```

It obtains `cargo metadata --locked --format-version 1` and enforces the declared local layering graph.

`scripts/check-domain-neutral.sh` contains:

```text
scan_production_files()
normalise_hits_to_paths()
compare_with_reviewed_allowlist()
```

It fails on a new application/domain token outside `.ci/domain-token-allowlist.txt` and fails when an allowlisted file no longer contains a token, forcing debt reduction to update the receipt.

`scripts/check-packages.sh` contains:

```text
package_publishable_workspace_graph()
publish_dry_run_leaf(package)
```

It packages the six releasable crates as one dependency graph without registry verification and performs full publish dry runs for the two leaf packages. Cross-layer tests live in the non-publishable integration-test crate so they cannot create a cyclic release order. Registry publication remains a Phase 7 decision.

### CI

`.github/workflows/ci.yml` defines required jobs:

- `format`;
- `check-test-clippy-doc`;
- `feature-matrix`;
- `boundaries`;
- `package`;
- `dependency-policy` using cargo-deny.

The existing layering workflow remains temporarily and runs alongside the metadata-backed gate.

### Developer Cargo configuration

- Remove `[patch."https://github.com/adamtc007/dsl"]` and `[patch."https://github.com/adamtc007/bpmn-lite"]` from `/Users/adamtc007/.cargo/config.toml` only; retain unrelated global build/registry settings.
- Create gitignored `/Users/adamtc007/Developer/ob-poc/.cargo/config.toml` with both patch sets, resolving BPMN to the selected semantic-board worktree.
- Create gitignored `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board/.cargo/config.toml` with only DSL patches.
- Add `/.cargo/config.toml` to the selected BPMN branch's `.gitignore`.

## Tests and gate

Focused checks run first, followed by:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked
bash scripts/check-layering.sh
bash scripts/check-dependencies.sh
bash scripts/check-domain-neutral.sh
bash scripts/check-packages.sh
```

Phase 1 closes only when these pass from the shared repository without the `ob-poc` checkout being required for non-ignored generic coverage. The resulting 62 ignored host/workspace-pack tests remain an explicit Phase 2/5 carry-over rather than a Phase 1 gate failure. Four previously unclassified catalogue tests were made explicit ignores; moving the cross-layer discovery test into the integration crate changed the executable grouping, so the final count is not a simple baseline-plus-four total.
