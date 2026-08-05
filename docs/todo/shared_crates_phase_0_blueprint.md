# Shared-crates remediation Phase 0 blueprint

**Phase:** 0 — freeze baseline and write dependency ledger
**Date:** 5 August 2026
**Coordinating plan:** `docs/todo/zed_shared_crates_standalone_remediation_plan.md`

## Invariants and absolute boundaries

1. Phase 0 is read-only with respect to production source, manifests, lockfiles, user Cargo configuration, tags, and branches.
2. Existing dirty and ignored files are user-owned. No formatter output is applied and no lockfile is regenerated.
3. The three repositories are inspected at their current exact revisions; the clean BPMN `main` worktree is additionally inspected because it contains the semantic-board consumer absent from the coordinating checkout.
4. Cargo checks that must exclude the user-global path patches run from `/tmp` with an isolated `CARGO_HOME`, an explicit `--manifest-path`, and the repository's declared Rust toolchain.
5. A failed baseline is evidence, not permission to repair it in this phase.
6. No public or persisted identity bytes are changed.
7. Phase 1 does not begin while a stop condition in the programme charter remains unresolved.

## Target artifacts

This phase creates or amends only:

- `docs/todo/shared_crates_phase_0_blueprint.md` — this execution boundary;
- `docs/todo/shared_crates_phase_0_baseline_2026-08-05.md` — evidence and dependency ledger;
- factual corrections in the coordinating plan discovered during verification.

## Inspection procedure

### Repository identity

For BPMN, DSL, and `ob-poc`, record:

- branch, HEAD, upstream, worktrees, tags at HEAD, remotes, and dirty files;
- Rust toolchain and Cargo version;
- root/workspace structure and repository instructions.

### Dependency ledger

Use `cargo metadata --locked --format-version 1 --no-deps`, Cargo manifests, and committed lockfiles to record:

- shared DSL/SemOS consumers;
- immutable and mutable source pins;
- optional feature edges;
- the `ob-poc -> bpmn-lite` edge;
- the BPMN embedding edge to `ob-poc-rust`;
- Cargo path-patch behaviour.

### Public and persistence surfaces

Trace source imports and manifest dependencies for:

- `dsl-core`, `dsl_types`, `sem_os_types`, `sem_os_core`, `sem_os_ontology`, and `sem_os_policy`;
- `ob-semantic-matcher`;
- `dsl-sage` and `ob-poc-sage`;
- semantic boards, evidence, proposal workbooks, hashes, corpora, captures, and server pending state.

Classify each shared crate/module as contract, host-neutral implementation, pack schema/runtime, persistence adapter, host adapter, or fixture/example. Where a module contains mixed responsibilities, record the split rather than pretending the crate already satisfies the target boundary.

### Baseline gates

Run, or record why a command cannot reach compilation:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo doc --workspace --no-deps --all-features --locked
```

Focused consumer checks use the clean BPMN `main` worktree. A full `ob-poc` test/lint/doc run is not attempted after its clean locked check proves that the committed lock cannot resolve without developer-global patches; subsequent commands would repeat the same pre-compilation failure or mutate the lock.

## Gate

Phase 0 closes only when the receipt:

- identifies the real consumer and persistence surfaces;
- names every pre-existing failure and environment-dependent success;
- records the BPMN branch divergence;
- maps current crates to target responsibilities;
- identifies owner decisions that prevent Phase 1.

The next phase must not start until the owner rules on the BPMN base branch and supplies an authoritative licence/versioning policy for the shared repository.
