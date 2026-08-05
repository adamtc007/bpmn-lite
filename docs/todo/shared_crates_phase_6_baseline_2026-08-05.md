# Shared-crates remediation Phase 6 baseline

**Date:** 5 August 2026
**Status:** pre-edit ownership and compatibility receipt

## Repository ledger

| Repository | Branch | Starting HEAD | Upstream | Pre-existing dirty state |
|---|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `feat/semantic-embedder` | `5ac7da7a513744e907ca110484c3a6a9472ae985` | `origin/feat/semantic-embedder` | clean |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/semantic-embedder` | `2665c06ad42ef51a54e42c7739546edfc6ccbf49` | none | clean; ignored local `.cargo/config.toml` present |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-embedder-adapter` | `333975b7c453758f5fabfdba76b2a0875df5da05` | none | pre-existing `M .cargo/config.toml.example`; ignored root `.cargo/config.toml` present |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `ddd143e8258b17593ab6282742fa84e5795cdb30` | not recorded | concurrent DIR-002/model work and programme documents; preserved |

Other worktrees remain `/Users/adamtc007/dev/dsl-sem-os-decision-board` at `edded438f07303fd954ec2a814bf3302f30e449d` and `/Users/adamtc007/Developer/ob-poc-bpmn-pack-truth` at `d2afc0c49d8b2b6cea8fb83f95474c17f0d4b639`. Neither is selected.

Shared and BPMN use Rust/Cargo 1.95.0. The active ob-poc checkout uses Rust/Cargo 1.96.1 from its `rust-toolchain.toml`; the shared MSRV remains 1.95.

## Forensic evidence

`cargo metadata --locked --no-deps` reports one package with a dependency named `dsl-sage`: `dsl-sage` itself. That edge is the crate's dev-only self-dependency used to expose `test-util` to its own integration tests. Repository search outside the package finds only:

- the workspace-member entry in `rust/Cargo.toml`;
- its `rust/Cargo.lock` package entry;
- the generated `audits/surface/dsl-sage.txt` API snapshot;
- historical documentation/evidence references.

The live implementation is separate: the root application depends on `ob-poc-sage`, and the application owns its inline REPL V2 routes, types, sessions, persistence, personas and UI. `ob-poc-sage` has no dependency on `dsl-sage`.

The BPMN designer's local keyword classifier currently fabricates application-specific authority in two places:

1. retry suggestions always target `create-cbu` even though no selected node is supplied;
2. an `unknown verb` phrase without a verb identity invents `ob-poc:cbu.create`, while an unqualified import invents the `ob-poc` domain.

The route is named `/api/dsl/sage/utter` and its handler is named `sage_utterance_gate`, despite being a local substring classifier rather than the shared Sage runtime. The route is retained for compatibility; the implementation is renamed and documented accurately.

## Baseline checks

- DSL: `cargo check --workspace --all-targets --all-features --locked` passed at `5ac7da7`.
- ob-poc: `cargo check -p dsl-sage --all-features --locked` was blocked before compilation because the ignored local path-patch configuration makes the committed lockfile differ from the exact dependency graph. This is the known Phase 1 development-override condition, not a source failure.
- BPMN: `cargo check -p bpmn-lite-server-designer --all-targets --all-features --locked` was blocked for the same ignored local path-patch/lock mismatch.
- Exact-revision focused checks will be run with each ignored root `.cargo/config.toml` temporarily disabled and then restored.

The Phase 4 receipt already records green full shared gates, green focused ob-poc adapter gates and green BPMN workspace check/test gates at these starting commits. Phase 6 will re-run checks proportionate to the files changed.

## Compatibility risks and controls

1. A caller may rely on the fabricated retry node. The replacement accepts an optional exact selected node and otherwise returns a non-action response rather than constructing an invalid macro request.
2. A caller may rely on the fabricated ob-poc diagnostic verb. The replacement accepts optional diagnostic context and otherwise asks for the exact unresolved identity.
3. The existing request remains backward-compatible because added fields are optional; response field names and shapes are unchanged.
4. Explicit cross-domain verbs remain possible when the caller names them. Only implicit ob-poc defaults are removed.
5. The ob-poc orphan deletion has no consumer compatibility risk according to Cargo metadata. Git history remains the recovery path.
6. The coordinating worktree and its unrelated dirty files will not be staged or committed as part of either code repository commit.
