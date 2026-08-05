# Shared DSL/SemOS standalone remediation — Phase 0 baseline

**Recorded:** 5 August 2026
**Scope:** `/Users/adamtc007/dev/dsl`, its BPMN consumer, and `/Users/adamtc007/Developer/ob-poc`
**Result:** dependency ledger complete; implementation paused at explicit programme stop conditions

## Executive result

The plan's architectural diagnosis remains valid, but the execution base has drifted. The coordinating BPMN checkout is a DIR-002 training branch at `ddd143e`; it does not contain the semantic-board implementation now on clean `main` at `745b4ea`. Implementing consumer changes in the current checkout would target the wrong history.

The shared DSL repository is also demonstrably not standalone:

- its only CI job is a source-layering guard;
- formatting and warnings-denied Clippy fail;
- a clean locked build is obscured by user-global path patches unless Cargo configuration discovery is isolated;
- 59 tests are ignored because they require the `ob-poc` configuration tree;
- an untracked, gitignored absolute symlink named `config` points to `ob-poc/rust/config`;
- a test presented as using the “real verb catalogue” fails when the workspace is executed without that host checkout;
- the repository has no README, licence, or changelog, and crate versions (`0.1.0`) do not track repository tags (`v0.1.7`).

No production source, manifest, lockfile, user Cargo configuration, tag, or branch was changed in this phase.

## Repository receipt

| Repository/worktree | Branch | HEAD | Upstream/status |
|---|---|---|---|
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `ddd143e8258b17593ab6282742fa84e5795cdb30` | tracks matching origin; dirty user files listed below |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `main` | `745b4ea0780be8811bb5c1f4ab42d71067a4d178` | clean, matches `origin/main`; contains the semantic-board consumer |
| `/Users/adamtc007/dev/dsl` | `main` | `edded438f07303fd954ec2a814bf3302f30e449d` | clean tracked state, matches `origin/main`, annotated tag `v0.1.7` |
| `/Users/adamtc007/dev/dsl-sem-os-decision-board` | `feature/sem-os-decision-board` | `edded438f07303fd954ec2a814bf3302f30e449d` | second worktree at the same commit |
| `/Users/adamtc007/Developer/ob-poc` | `chore/dead-code-phase-0-visibility` | `d76d8be9842c960e06841a4cc661d03ad44fbe73` | clean, matches matching origin |
| `/Users/adamtc007/Developer/ob-poc-bpmn-pack-truth` | `main` | `d2afc0c49d8b2b6cea8fb83f95474c17f0d4b639` | clean separate main worktree |

### Pre-existing user files preserved

The coordinating BPMN checkout already contained:

```text
M  .DS_Store
M  bpmn-lite-server-runner/src/bus_runtime.rs
M  utterance-engine/train_py/bundles/eval_scores.json
M  utterance-engine/train_py/bundles/modernbert-base/training_card.json
M  utterance-engine/train_py/split_manifest.json
?? docs/.DS_Store
```

None was formatted, staged, reverted, or otherwise changed by this phase.

### Toolchains

| Repository | Declared toolchain | Observed Cargo/rustc |
|---|---|---|
| BPMN | Rust `1.95` | `rustc 1.95.0`, Cargo `1.95.0` |
| DSL | Rust `1.95` with rustfmt, Clippy, rust-src, rust-analyzer | `rustc 1.95.0`, Cargo `1.95.0` when explicitly selected |
| `ob-poc` | Rust `1.96` with rustfmt, Clippy, rust-src, rust-analyzer | toolchain declared; commands selected `RUSTUP_TOOLCHAIN=1.96` |

The consumer MSRV policy is not documented. The current 1.95/1.96 mismatch must be ruled before a shared `[workspace.package].rust-version` is declared.

## Branch drift requiring a ruling

The active BPMN branch and `main` are not interchangeable:

- active branch `ddd143e` contains the shared-crates plan but has no `utterance-engine/src/bpmn_pack.rs` or `utterance-engine/src/bpmn_board.rs`;
- `main` at `745b4ea` contains both files, the proposal-workbook integration, shared DSL dependencies, pack gates, mapper tests, and the completed semantic-board programme;
- active DIR-002 work has modified `utterance-engine` training/model artifacts and the branch is explicitly named by the plan as concurrent work;
- `main...HEAD` contains distinct commits on both sides, so this is not a simple fast-forward discrepancy.

Phase 3/4 consumer work must therefore target an owner-selected integration base. Recommendation: use semantic-board `main` as the remediation base and integrate DIR-002 separately under its current owner.

## Dependency ledger

### Shared workspace packages

All seven packages are version `0.1.0`:

| Current crate | Current responsibility | Target classification/destination |
|---|---|---|
| `dsl_types` | constellation/DAG DTOs, including closed host-flavoured slot variants | host-neutral model; generic identifier/value contracts |
| `dsl-core` | parser, AST, compiler, configuration loaders/validators, executable-plan types | split conceptually into `dsl-model`, `dsl-syntax`, and `semantic-pack`; retain names if compatibility requires |
| `sem_os_types` | foundational SemOS DTOs plus hard-coded agent-mode command families | host-neutral SemOS primitive contracts after host semantics move to packs |
| `sem_os_core` | store ports, principal, IDs, resolver, seed and frontier machinery | host-neutral orchestration and ports; filesystem/config composition moves behind pack-source ports |
| `sem_os_ontology` | ontology definition bodies and verb contract vocabulary | host-neutral vocabulary/pack model; decision-only vocabulary may move with contracts |
| `sem_os_policy` | ABAC, projection, authoring, decision board, observatory, domain packs and policy | mixed: decision contracts extract; generic policy remains; host declarations move to YAML; persistence/host concerns move outward |
| `dsl-integration-tests` | external-style integration tests, many coupled to host config | generic public-API tests plus versioned fixtures; host-pack qualification moves to consumers |

There is no shared `semantic-decision-contracts`, `semantic-pack`, `semantic-embedder`, or `repl-contracts` crate today.

### BPMN consumer edges

The active DIR-002 checkout directly declares only the optional embedder edge:

```text
utterance-engine -> ob-semantic-matcher
git https://github.com/adamtc007/ob-poc-rust
rev ff3f12c7c0dfa4ac9c8a7bc086162fc2bcecb67e
default-features = false, optional = true
```

The semantic-board `main` worktree additionally declares:

```text
utterance-engine -> sem_os_ontology @ dsl rev fa51217ffd2218edea82c175e45ffa11d9eb7cf9
utterance-engine -> sem_os_policy   @ dsl rev fa51217ffd2218edea82c175e45ffa11d9eb7cf9
bpmn-lite-server-designer -> sem_os_policy through workspace dependency
fuzz targets -> sem_os_policy at the same exact rev
```

The committed lock resolves all six shared packages from that DSL revision. The gate in `scripts/check-shared-pin.sh` knows those six package names, but not the separate `ob-semantic-matcher`/future embedder source.

### `ob-poc` consumer edges

`ob-poc` declares shared packages from mutable tag `v0.1.5`, including direct uses from the root application, `dsl-analysis`, `dsl-runtime`, `ob-agentic`, `ob-poc-agent`, `ob-poc-boundary`, `sem_os_client`, `sem_os_mcp`, `sem_os_obpoc_adapter`, `sem_os_postgres`, `dsl-lsp`, `ob-poc-web`, `sem_os_server`, `sem_os_harness`, and `xtask`.

It also consumes BPMN at mutable tag `v0.2.0` for engine/store/FFI/DMN/bus/manifest packages, while `bpmn-lite-server-runner` is pinned separately to rev `f9e48161855e57bcf6e34534c7ef6d2db6d80486`.

This confirms both the three-way DSL pin skew and the undeclared application-to-application edge described by the plan.

### Cargo override state

`/Users/adamtc007/.cargo/config.toml` still contains path patches for all six DSL packages and numerous BPMN packages. The F5 per-repository ruling has not been implemented. `/Users/adamtc007/dev/dsl/.cargo/config.toml` exists but contains only `git-fetch-with-cli = true`; it does not replace the global patches. The DSL config file is gitignored. BPMN does not yet ignore a root `.cargo/config.toml`; `ob-poc` does.

Consequences verified in this phase:

- ordinary DSL locked checks discover the global patches and fail before compilation because Cargo wants to rewrite the lock;
- running Cargo from `/tmp` with isolated config discovery allows the DSL committed lock to compile;
- a clean isolated `ob-poc --locked` check cannot resolve its committed lock and fails before compilation;
- ordinary local metadata can appear healthy only because the developer-global patches alter resolution.

## Public API and persistence surfaces

### BPMN `main`

Direct shared APIs are confined to two areas:

- `sem_os_policy::decision_board`: boards, candidate slices, evidence lanes, disposition inputs, workbooks, slots and transitions across `utterance-engine` and `bpmn-lite-server-designer`;
- `sem_os_ontology::verb_contract::{ActionClass, HarmClass}` in candidate/pack construction and tests.

The semantic board is serialised into corpus/capture records. `PendingProposal` stores a `ProposalWorkbook` in the designer server's pending session state. Board and evidence hashes are therefore compatibility surfaces even though no reviewed `ob-poc` SQL migration persists these shared decision-board types. The BPMN-specific board/serializer hashes use BLAKE3 and remain a separate compatibility layer from shared SHA-256 decision hashes.

### `ob-poc`

Source-file consumer counts at this baseline are broad: `sem_os_core` appears in 111 Rust files, `sem_os_policy` in 81, `dsl_core` in 80, `sem_os_ontology` in 41, `sem_os_types` in 18, and `dsl_types` in 7. These counts include tests and compatibility paths but demonstrate that a flag-day rename is unsafe.

`ob-semantic-matcher` is both inference and host persistence/application behaviour: learning handlers use `FeedbackService`, `PatternLearner`, `PromotionService`, and `Embedder`; binaries populate embeddings; Postgres operations invoke matcher population flows. BPMN uses only `Embedder`, confirming the pure-subset extraction seam.

No `ob-poc` source or migration reference to the shared `SemanticDecisionBoard`, `InferenceEvidence`, or `ProposalWorkbook` was found. `ob-poc` has a distinct execution-workbook contract and hash lineage; it must not be conflated with the mapper proposal workbook.

`dsl-sage` is a workspace member used only by itself/tests. The live application path is `ob-poc-sage`, used by the root and `ob-poc-agent`. This confirms the F2 ruling: no shared REPL crate is justified today.

### Shared public contract inventory

The decision-board module currently exports:

- `BoardHash`, `EvidenceRecordHash`, `CandidateSemanticSlice`, `SemanticDecisionBoard`, and position/identity wrappers;
- phrase, argument, lane and candidate evidence types;
- `InferenceEvidence`, `InferenceDisposition`, `DispositionPolicy`, `DecisionRecord`, and `decide`;
- `ProposalWorkbook`, `WorkbookSlot`, binding/value/requirement types, and `ProposalStatus`.

It is a host-neutral contract candidate but currently lives in the large downstream policy crate. Five decision hash inputs use `Debug` enum formatting. `acp_projection` has an additional unframed `Debug`-dependent hash. Those bytes must remain v1-compatible during extraction; v2 is a separate migration.

## Standalone defects confirmed

1. The DSL root has no README, licence, or changelog. `Cargo.lock` is tracked.
2. Crate descriptions and comments still describe OB-POC-specific ownership.
3. CI contains only `.github/workflows/layering.yml`, which runs `scripts/check-layering.sh`.
4. The only unsafe production code is the raw-pointer lifetime extension in `dsl-core/src/ast.rs:748`.
5. Public reusable APIs still return `anyhow::Result` or `Result<_, String>` in parsers, directory loaders, bundle building, and resolver composition.
6. The absolute `config` symlink is untracked/gitignored and points to `/Users/adamtc007/Developer/ob-poc/rust/config`.
7. Exactly 59 tests are marked ignored for host configuration or workspace-root configuration requirements.
8. Host vocabulary remains in production types/policy even though simple token scans can miss it through casing, generated data, and tests; the source-level inventory in the coordinating plan remains accurate.
9. The shared workspace has no complete bytes-in/typed-artifact/registry API and performs ambient filesystem loading in core crates.

## Baseline command results

### Shared DSL at `edded43`, Rust 1.95

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **FAIL** — formatting drift across DSL, types, SemOS core/policy and integration tests |
| ordinary `cargo check ... --locked` from repo | **FAIL before compilation** — global BPMN/DSL patches require lock update |
| isolated `cargo check ... --locked` | **PASS** — all workspace/all targets/all features; one dead-code warning in a test |
| isolated `cargo test ... --locked` | **FAIL after extensive passes** — `every_catalogue_verb_has_phase7_flavour` reports “real verb catalogue count regressed below baseline” when run without host-root config; the first library reported 295 passed and 17 ignored before later test binaries ran |
| isolated `cargo clippy ... --locked -- -D warnings` | **FAIL** — five `wrong_self_convention`, one `for_kv_map`, one `enum_variant_names` finding |
| isolated `cargo doc ... --locked` | **PASS** — seven package documentation outputs generated |

The isolated commands ran from `/tmp` with explicit manifest/toolchain so Cargo did not discover `/Users/adamtc007/.cargo/config.toml`. Generated `target/` content is ignored; no tracked file changed.

### BPMN semantic-board `main` at `745b4ea`, Rust 1.95

| Command | Result |
|---|---|
| isolated workspace check, all targets/features, locked | **PASS** with pre-existing dead-code warnings in `utterance-engine::capture` |
| formatting check | **FAIL** with broad existing rustfmt drift |
| full tests/Clippy/docs | **Not run** — Phase 0 established the clean consumer compiles; changing consumer contracts is blocked by branch selection and the plan requires stopping rather than spending a misleading gate against an unselected base |

### `ob-poc` at `d76d8be9`, Rust 1.96

| Command | Result |
|---|---|
| isolated workspace check, all targets/features, locked | **FAIL before compilation** — committed lock requires update when developer-global patches are excluded |
| formatting check | **FAIL** with broad existing rustfmt drift |
| full tests/Clippy/docs | **Not run** — the clean locked resolution prerequisite failed; running unlocked would mutate user-owned `Cargo.lock` and invalidate the baseline |

## Runtime/deployment inclusion

- BPMN's root Dockerfile builds `bpmn-lite-server-runner`; the designer/utterance path is a workspace/application surface rather than a separately proven released shared artifact.
- `ob-poc` deploys its root/web/server composition and includes local SemOS, matcher, Sage, and DSL adapter crates.
- BPMN's development compose file also builds an `ob-poc` application image, confirming operational coupling, but this phase introduced no deployment change.

## Phase gate and stop conditions

The Phase 0 ledger is complete enough to name every later move and preserve dirty work. Phase 1 must not begin yet because three owner decisions are required:

1. **BPMN base:** choose semantic-board `main` (`745b4ea`, recommended) or provide an integration strategy for DIR-002 `ddd143e`.
2. **Licence:** provide the authoritative licence for `/Users/adamtc007/dev/dsl`; the implementation must not invent one.
3. **Version/MSRV policy:** decide whether repository tag `v0.1.7` corresponds to Cargo `0.1.7` or another scheme, and settle a shared supported Rust version across 1.95/1.96 consumers.

Until those rulings land, no Phase 1 metadata, safety, contracts, embedding, pack, SemOS, consumer, release, or deployment edits are authorised by the plan's own stop conditions.

### Subsequent owner resolutions

The owner resolved all three stop conditions later on 5 August 2026:

1. use semantic-board `main` at `745b4ea` as the BPMN remediation base;
2. license the shared workspace under MIT;
3. release the shared crates as `0.2.0` with MSRV Rust 1.95.

These resolutions do not alter the historical Phase 0 observations; they authorised Phase 1.
