# Shared-crates remediation Phase 1 receipt

**Phase:** 1 — standalone package and CI discipline
**Date:** 5 August 2026
**Result:** implementation and local gate complete; changes remain uncommitted for phase-gate review

## Owner rulings applied

- Licence: MIT.
- Shared package version: `0.2.0`.
- Minimum supported Rust version: 1.95.
- BPMN consumer base: semantic-board `main` at `745b4ea`, isolated from the concurrent DIR-002 training branch.
- Development overrides: repository-local, uncommitted and gitignored Cargo patch configuration; no global DSL/BPMN patch blocks.

## Repository ledger

| Repository | Phase branch | Starting and current HEAD | Phase 1 tracked changes |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `refactor/standalone-phase1` | `edded438f07303fd954ec2a814bf3302f30e449d` | standalone metadata, licence, documentation, safety cleanup, CI, dependency/domain/package gates, release-order test relocation |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/shared-crates-phase1-config` | `745b4ea0780be8811bb5c1f4ab42d71067a4d178` | ignored local-config rule and portable DSL patch example |
| `/Users/adamtc007/Developer/ob-poc` | `chore/dead-code-phase-0-visibility` | `d76d8be9842c960e06841a4cc661d03ad44fbe73` | portable local DSL/BPMN patch example only; no application source change |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `ddd143e8258b17593ab6282742fa84e5795cdb30` | programme documents and receipts only; pre-existing DIR-002 changes preserved |

No phase commits or tags have been created yet. Ending HEAD therefore equals starting HEAD in each repository.

## Implemented boundaries

The DSL workspace now owns consistent workspace metadata for all seven packages: edition 2021, Rust 1.95, MIT, repository URL and version `0.2.0`. The six library crates have crate READMEs and neutral descriptions; the integration-test crate is explicitly non-publishable. The root now contains `README.md`, `LICENSE`, `CHANGELOG.md`, `docs/versioning.md` and a committed lockfile policy.

Every library root forbids unsafe Rust. The unused raw-pointer implementation of `find_unresolved_refs` was deleted. Its sole private test was redirected to the supported safe `find_unresolved_ref_locations` API. There is no public API removal.

The shared CI now checks formatting, locked all-target/all-feature compilation and tests, Clippy with warnings denied, rustdoc with warnings denied, a DSL feature matrix, metadata-backed dependency direction, domain-vocabulary debt, packaging, publish dry runs, advisories, licences and dependency sources.

Cross-layer `discovery_pipeline` coverage moved from `sem_os_core/tests` to the non-publishable `dsl-integration-tests` crate. This removes the `sem_os_core` development back-edge to `sem_os_policy`, making the library release graph acyclic without reducing executed coverage.

Four verb-catalogue tests that require the `ob-poc` catalogue are now explicit ignores instead of failing in a standalone checkout. The executed suite reports 900 passed, 0 failed and 62 ignored. Host-pack test migration remains Phase 2/5 work.

## Dependency and developer-resolution result

The user-global Cargo file contains no DSL or BPMN `[patch]` sections. Active opt-in configurations now live at:

- `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board/.cargo/config.toml` — DSL patches only;
- `/Users/adamtc007/Developer/ob-poc/.cargo/config.toml` — DSL and BPMN patches.

Both files are ignored and uncommitted. Their portable `.example` counterparts are tracked. No consumer manifest or consumer lockfile was intentionally changed.

The dependency-policy gate found `anyhow 1.0.102` affected by `RUSTSEC-2026-0190`. The DSL lockfile now resolves `anyhow 1.0.103`, and `cargo-deny 0.20.2` passes advisories, bans, licences and sources. The two `wit-bindgen` versions remain a warning-level transitive duplicate via the current WASI dependency graph.

## Verification receipt

Toolchain: `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`.

| Command | Outcome |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo check --workspace --all-targets --all-features --locked` | pass |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | pass |
| `cargo test --workspace --all-targets --all-features --locked` | pass — 900 passed, 0 failed, 62 ignored |
| `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked` | pass |
| `cargo check -p dsl-core --no-default-features --locked` | pass |
| `cargo check -p dsl-core --all-features --locked` | pass |
| `bash scripts/check-layering.sh` | pass |
| `bash scripts/check-dependencies.sh` | pass |
| `bash scripts/check-domain-neutral.sh` | pass |
| `bash scripts/check-packages.sh` | pass — six library archives; full dry-run for `dsl_types` and `sem_os_types` |
| `cargo-deny 0.20.2 check` | pass — advisories, bans, licences and sources |

The package gate initially exposed the cross-layer development dependency cycle, and the advisory gate initially exposed the vulnerable `anyhow` lock. Both failures were remediated and rerun green; they are retained here as evidence that the new gates are effective.

## Compatibility and deployment

This phase made no serialized schema, canonical encoding, UUID namespace, decision-board hash, evidence hash, inference model or runtime behaviour change. It performed no database migration, model promotion, deployment or application cutover. Consequently serialization replay, inference comparison, deployment shadow and rollback tests are not applicable to Phase 1.

## Carry-overs

1. Migrate the 62 ignored host/workspace-pack tests to versioned pack fixtures through the public pack API in Phases 2 and 5.
2. Reduce the reviewed domain-token allowlist as application vocabulary moves out of shared production source.
3. Publish library crates in dependency order and replace consumer development patches with immutable release pins in Phase 7.
4. Reconcile the warning-level duplicate `wit-bindgen` versions when the upstream WASI graph converges; it is not a source or advisory failure.
5. Continue with Phase 3 only after this phase-gate receipt is accepted, per the ruled execution order.

## User-work preservation

The coordinating BPMN checkout's pre-existing `.DS_Store`, `bpmn-lite-server-runner/src/bus_runtime.rs`, model-training receipts, and `docs/.DS_Store` were not modified, staged or reverted by this phase. The concurrent DIR-002 branch was not used as the consumer implementation base.
