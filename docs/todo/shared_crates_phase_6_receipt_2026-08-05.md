# Shared-crates remediation Phase 6 receipt

**Date:** 5 August 2026
**Gate:** Phase 6 — Sage/REPL ownership and host-adapter cleanup
**Result:** complete with pre-existing repository-wide gate debt recorded below

## Outcome

Sage remains an `ob-poc` application capability. No shared `repl-contracts` crate was created. Cargo metadata proved the separate `dsl-sage` package had no consumer except its own integration-test self-dependency, so the orphan package was retired. The live `ob-poc-sage` crate, inline REPL V2 routes/types, persistence, persona logic and UI behavior were not moved or changed.

The BPMN designer's legacy utterance route no longer invents `create-cbu`, `ob-poc:cbu.create` or an `ob-poc` domain. Its response shape and route remain compatible. Retry and diagnostic actions now require caller-supplied BPMN context; missing context produces a non-mutating explanatory response.

## Repository and commit ledger

| Repository | Phase branch | Starting HEAD | Ending HEAD | Resulting status |
|---|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `feat/semantic-embedder` | `5ac7da7a513744e907ca110484c3a6a9472ae985` | unchanged | clean |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/sage-host-boundary` | `2665c06ad42ef51a54e42c7739546edfc6ccbf49` | `506e931b122014b0e2bdaf44d5ed296b2bcf7f2e` | clean |
| `/Users/adamtc007/Developer/ob-poc` | `cleanup/retire-dsl-sage` | `333975b7c453758f5fabfdba76b2a0875df5da05` | `4ad0e338ddbb393111d0f116bcb4d53b9ef8054d` | only pre-existing `M .cargo/config.toml.example` remains |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `ddd143e8258b17593ab6282742fa84e5795cdb30` | unchanged | concurrent work and programme docs preserved |

Commits:

1. ob-poc `4ad0e338` — `cleanup: retire orphan dsl-sage crate`.
2. BPMN `506e931` — `fix(designer): remove ob-poc utterance fallbacks`.

Neither new consumer branch has been pushed. No shared release or tag was created in this phase. The selected shared revision remains exact commit `5ac7da7a513744e907ca110484c3a6a9472ae985`, version 0.2.0, MIT, MSRV 1.95.

## Source and dependency changes

Deleted from ob-poc:

```text
audits/surface/dsl-sage.txt
rust/crates/dsl-sage/Cargo.toml
rust/crates/dsl-sage/src/{audit,confirmation,context,extractor,instantiator,lib,matcher,orchestrator,repl,types}.rs
rust/crates/dsl-sage/tests/{compliance_pilot,instantiation,orchestrator,pack_matching_eval,parameter_extraction}.rs
```

Updated `rust/Cargo.toml` to remove the package from the workspace and `rust/Cargo.lock` to remove its self-referential package entry. The retirement deletes 6,857 lines. No public module was moved or compatibility-re-exported because there was no consumer.

Dependency graph before:

```text
ob-poc workspace
  ├─ ob-poc-sage  -> live application Sage dependencies
  └─ dsl-sage     -> DSL frontend crates + its own test-util self-dependency
                     (no incoming production edge)
```

Dependency graph after:

```text
ob-poc workspace
  └─ ob-poc-sage  -> live application Sage dependencies (unchanged)
```

Post-change `cargo metadata --locked --no-deps` lists `ob-poc-sage` and no `dsl-sage` package.

Updated BPMN `bpmn-lite-server-designer/src/rest.rs`:

- retained `POST /api/dsl/sage/utter` as a compatibility route;
- renamed the implementation to `designer_utterance_compat_endpoint` / `classify_designer_utterance` so it is not represented as a shared Sage engine;
- added optional `target_node_id` and `unresolved_verb` request context;
- emits a retry macro only for the supplied node ID;
- emits `AddVerbStub` only for an explicit/injected verb;
- qualifies unqualified explicit imports into the BPMN host domain;
- retains explicitly qualified external domains;
- fails closed when the required context is missing.

The production classifier block contains no `create-cbu` or `ob-poc` fallback. Explicit ob-poc workflow examples and the vendored invocation manifest elsewhere in the preview compiler remain deliberate cross-domain demo/catalogue fixtures; Phase 2 owns their pack/manifest reconciliation.

## Verification

Ignored root `.cargo/config.toml` development patches were temporarily disabled for exact-revision Cargo gates and restored after every command.

### Shared DSL

| Command | Outcome |
|---|---|
| `cargo check --workspace --all-targets --all-features --locked` | pass at `5ac7da7` |
| Git status | clean; no Phase 6 changes |

### BPMN

| Command | Outcome |
|---|---|
| `cargo check -p bpmn-lite-server-designer --all-targets --all-features --locked` | pass |
| `cargo test -p bpmn-lite-server-designer --all-features --locked` | 57 passed, 0 failed, 1 ignored |
| `cargo check --workspace --all-targets --all-features --locked` | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 92 suites; 1,335 passed, 0 failed, 6 ignored, 0 measured, 0 filtered |
| `cargo clippy -p bpmn-lite-server-designer --all-targets --all-features --no-deps --locked -- -D warnings` | pass; dependency compilation reports the existing non-denied `utterance-engine::CapturePipeline` dead-code warning |
| `RUSTDOCFLAGS='-D warnings' cargo doc -p bpmn-lite-server-designer --no-deps --all-features --locked` | pass |
| `git diff --check` before commit | pass |
| source search over the production classifier | no `create-cbu` or `ob-poc` match |

The broader `cargo clippy -p bpmn-lite-server-designer --all-targets --all-features --locked -- -D warnings` remains red in pre-existing dependencies: two `collapsible_if` findings in `bpmn-lite-kernel` and `match_like_matches_macro` / `too_many_arguments` findings in `bpmn-lite-compiler`. The package's own `--no-deps` lint gate is green.

`cargo fmt --package bpmn-lite-server-designer -- --check` remains red on three pre-existing rustfmt differences in `rest.rs` around workbook dry-run transitions. The Phase 6 diff itself was formatted and the pre-existing regions were not changed.

### ob-poc

| Command | Outcome |
|---|---|
| metadata assertion that `dsl-sage` is absent | pass |
| `cargo check -p ob-poc-sage --all-targets --all-features --locked` | pass |
| `cargo test -p ob-poc-sage --all-features --locked` | 35 passed, 0 failed, 0 ignored |
| `cargo fmt --package ob-poc-sage -- --check` | pass |
| `cargo clippy -p ob-poc-sage --all-targets --all-features --locked -- -D warnings` | pass |
| `RUSTDOCFLAGS='-D warnings' cargo doc -p ob-poc-sage --no-deps --all-features --locked` | pass |
| `git diff --check` before commit | pass |

The exact full-workspace `cargo check --workspace --all-targets --all-features --locked` remains red at a known shared-pin incompatibility outside Phase 6: `sem_os_obpoc_adapter/src/scanner.rs` initializes `VerbCrudMapping::set_values`, but ob-poc's exact DSL `v0.1.5` pin does not contain that field. Phase 7 already owns conversion from the stale mutable tag to the shared exact release revision. The failure occurs after the retired package is absent from metadata and is unrelated to Sage ownership.

Full ob-poc `cargo fmt --all -- --check` also remains red across extensive pre-existing files. No unrelated formatting was applied.

## Compatibility, hashes and deployment

- A cement test deserializes the old two-field request without either new optional context field.
- Response JSON field names and variants are unchanged.
- Existing escape/deployment navigation responses are unchanged.
- No board, evidence, workbook, canonical hash, UUID namespace, database record or schema changed; hash migration testing is therefore not applicable.
- No model code or bundle changed. Phase 4's bit-for-bit native inference comparison remains the governing model receipt and was not repeated.
- No application image was built or deployed. Shadow traffic and deployment testing are outside this forensic cleanup phase.
- Each host builds its relevant Sage/designer adapter independently in the focused gates above.

## Rollback

- BPMN rollback base: `2665c06ad42ef51a54e42c7739546edfc6ccbf49`; revert `506e931` to restore the former classifier.
- ob-poc rollback base: `333975b7c453758f5fabfdba76b2a0875df5da05`; revert `4ad0e338` to recover the orphan package and surface snapshot.
- No persistence or deployment rollback is required because the phase changed source/workspace membership only.

## Carry-overs

| Carry-over | Owner / phase | Target |
|---|---|---|
| ob-poc uses stale mutable DSL tag `v0.1.5`, causing the `VerbCrudMapping::set_values` full-workspace failure | Shared/ob-poc cutover, Phase 7 | same exact shared release revision in both consumers |
| BPMN preview compiler still includes an explicitly vendored ob-poc invocation manifest and demo fixtures | DSL/BPMN pack reconciliation, Phase 2 | distinguish generic external invocation catalogues from BPMN semantic policy |
| Legacy route spelling `/api/dsl/sage/utter` remains for wire compatibility | BPMN owner, Phase 9 or separately versioned API deprecation | remove only after callers migrate |
| No second reusable Sage protocol consumer exists | Architecture owner | reconsider `repl-contracts` only when a real second consumer supplies evidence |
| BPMN broad Clippy and local `rest.rs` formatting baselines are red outside this diff | BPMN maintainers | separate hygiene commit |
| ob-poc full-workspace formatting baseline is red | ob-poc maintainers | separate hygiene campaign |

## Preservation statement

The pre-existing ob-poc `.cargo/config.toml.example` modification was neither edited, staged nor committed. The coordinating DIR-002 worktree, model artifacts, runner edit, `.DS_Store` files and earlier programme documents were not staged or committed. Ignored development patch files were restored after exact-revision checks. No user-owned modification was reverted.

Phase 6 stops here. The next programme phase is Phase 2 and requires its own fresh baseline and full-block blueprint review before code changes.
