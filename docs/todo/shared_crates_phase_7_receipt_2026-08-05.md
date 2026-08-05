# Shared-crates remediation Phase 7 receipt

**Date:** 5 August 2026
**Gate:** Phase 7 — consumer cutover and dependency narrowing
**Result:** complete; release and consumer branches pushed; no deployment

## Outcome

The shared DSL/SemOS workspace is now a standalone, MIT-licensed Rust capability
release. Its domain behavior comes from typed YAML semantic packs; its public
Rust crates provide parsing, admission, registry, policy, decision-contract and
embedding APIs without an application reverse dependency.

Both BPMN and `ob-poc` build and test from exact Git revisions. Neither consumer
uses a mutable branch, tag dependency or developer path in its committed Cargo
graph. Locked `ob-poc` metadata contains exactly these two application-owned Git
sources:

```text
git+https://github.com/adamtc007/dsl?rev=586431f81e2bb9101578af5167b8a35335f5a09e
git+https://github.com/adamtc007/bpmn-lite?rev=de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a
```

Phase 7 stops at source release and consumer qualification. No application was
deployed and no database migration was performed.

## Repository and commit ledger

| Repository | Branch | Starting HEAD | Ending implementation HEAD | Published |
|---|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `refactor/sem-os-pack-policy` | `f0e2552a4153fb23c6910b82015d35860dab6739` | `586431f81e2bb9101578af5167b8a35335f5a09e` | branch and `v0.2.1` tag pushed |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/bpmn-semantic-pack` | `cc924da0b7f795f7a11aa5866d27a212712c1e62` | `de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a` | branch pushed |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-policy-consumer` | `ec0ba7ddfe4100520a151c58ab9edbef11d45437` | `e51dda11a02f4993e8023b934c4fd1df4293b983` | branch pushed |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `355b73dd364b20f922ae0e1f5a16c6bf232ff7e2` | unchanged before this receipt | receipt branch pushed separately |

Phase commits, in dependency order:

1. shared `0d44808` — move host qualification to consumers;
2. shared `9b76c95` — remove stale host-checkout suites;
3. shared `60bb00d` — close the initial release changelog;
4. shared `f2c81aa` — align release guards with admitted pack policy;
5. shared `586431f` — qualify version `0.2.1`;
6. BPMN `58e4f6e`, `5038ce1`, `de48b8c` — advance and qualify exact shared pins;
7. `ob-poc` `9f3bf889` — replace mutable/mixed pins and adapt public APIs;
8. `ob-poc` `a20cd1d2` — make YAML-to-DSL projections deterministic;
9. `ob-poc` `77cfb96e`, `e51dda11` — close the exact shared/BPMN revision graph.

## Release identity

The qualified release is:

| Property | Value |
|---|---|
| Tag | annotated `v0.2.1` |
| Commit | `586431f81e2bb9101578af5167b8a35335f5a09e` |
| Git-archive SHA-256 | `289b4bb6f727e2cc727603ecfc605de2df0e89e08534b96326989e57f79ab3f6` |
| Workspace crate version | `0.2.1` |
| Licence | MIT |
| MSRV | Rust 1.95 |
| Delivery | exact Git revision; crates were packaged and publish-dry-run, not uploaded |

`v0.2.0` remains an immutable tag at `f2c81aabe3939f695d7126c2b7d5fd1380617374`
with archive SHA-256
`f549d13fc9dcbd5e4b7552a3b08ff6ccb102f9e4919d11cc6e8a647f0293f0e5`.
It is superseded and must not be selected: final CI qualification found its
existing eight-port `CoreServiceImpl::new` rejected by the repository's
warnings-as-errors Clippy policy. Version `0.2.1` documents that intentional
dependency-injection boundary with a narrow lint allowance; no runtime or wire
behavior changed.

## Source and public API ledger

No Phase 7 public Rust module was created, moved, deleted or compatibility
re-exported. The compatibility re-exports retained by earlier extraction phases
remain available for the promised transition period. Phase 7 made these
ownership changes instead:

- removed nine shared tests that reached into host checkouts and one stale
  nested host-specific test module;
- narrowed BPMN designer's direct shared edge to
  `semantic-decision-contracts`; pack compilation and optional embedding stay
  behind `utterance-engine`'s owned boundary;
- removed BPMN's obsolete `ob-semantic-matcher` route to the historical shared
  checkout and removed direct `sem_os_policy` use where it existed only to
  reach extracted APIs;
- changed all `ob-poc` shared dependencies from the mutable `v0.1.5` tag/mixed
  revisions to one exact `v0.2.1` commit;
- made the YAML verb source authoritative by emitting an explicit domain slot,
  canonicalizing map order, pruning only generator-owned stale files, and
  proving repeated generation is byte-identical;
- added generated `kyc/dsl-kyc.dsl` and
  `kyc/dsl-kyc-obligation.dsl`, and deleted five stale generated projections:
  `control.dsl`, `kyc/board.dsl`, `kyc/ubo-registry.dsl`, `ownership.dsl`, and
  `ubo.dsl`.

The generator produced 145 files from 1,253 verbs, including 202 Pattern-D
bindings. A second run had identical hashes and pruned zero files.

## Dependency graph receipt

Before Phase 7:

```text
bpmn-lite designer
  -> shared decision/pack/policy APIs at pre-release revisions
  -> ob-semantic-matcher from a historical shared checkout

ob-poc
  -> six DSL/SemOS crates via mutable tag v0.1.5
  -> newer extracted crates and BPMN packages via different exact revisions
```

After Phase 7:

```text
bpmn-lite designer @ de48b8c
  -> semantic-decision-contracts @ dsl 586431f
  -> utterance-engine
       -> semantic-pack @ dsl 586431f
       -> optional semantic-embedder @ dsl 586431f

ob-poc @ e51dda11
  -> shared DSL/SemOS family @ dsl 586431f
  -> BPMN integration family @ bpmn-lite de48b8c
       -> shared DSL/SemOS family @ dsl 586431f
```

`cargo metadata --locked` proves one DSL Git source and one BPMN Git source.
`cargo tree -p bpmn-lite-server-designer --edges normal --locked` contains no
SQLx or pgvector edge. Shared dependency, layering and domain-neutrality guards
all pass; there is no application reverse edge from shared crates.

## Verification receipt

Ignored root `.cargo/config.toml` development patches were temporarily disabled
for exact-revision gates and restored after every command.

### Shared DSL/SemOS `v0.2.1`

| Command / gate | Outcome |
|---|---|
| `cargo check --workspace --all-targets --all-features` | pass |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 27 suites; 914 passed, 0 failed, 1 ignored |
| `scripts/check-dependencies.sh` | pass |
| `scripts/check-layering.sh` | pass |
| `scripts/check-domain-neutral.sh` | pass |
| `scripts/check-packages.sh` | all nine publishable packages packaged; three leaf publish dry-runs passed |

### BPMN consumer `de48b8c`

| Command / gate | Outcome |
|---|---|
| `cargo check --workspace --all-targets --all-features` | pass |
| `scripts/check-shared-pin.sh` | all nine shared packages resolve from `586431f`; no unused patch fallback |
| `cargo test --workspace --all-targets --all-features --locked` | 92 suites; 1,336 passed, 0 failed, 6 ignored |
| designer dependency-tree SQLx/pgvector assertion | pass — neither dependency is present |

### `ob-poc` consumer `e51dda11`

| Command / gate | Outcome |
|---|---|
| exact `cargo metadata --locked` Git-source assertion | pass — one DSL revision and one BPMN revision |
| `cargo check --workspace --all-targets --all-features` | pass |
| `cargo test --workspace --all-targets --all-features --locked` | 146 suites; 4,043 passed, 0 failed, 398 ignored |
| Clippy for changed library packages with all features/targets and `-D warnings` | pass |
| `cargo clippy -p ob-poc-boundary --lib --all-features --locked -- -D warnings` | pass |
| semantic receipt regeneration | pass — only 15 compiler-version labels changed; source and artifact hashes were unchanged |
| YAML-to-DSL double generation | pass — byte-identical output |

## Compatibility and hashes

- BPMN candidate, disposition, board, evidence, workbook, replay and persistent
  identity behavior remains covered by its full locked suite.
- `ob-poc` authorisation, command resolution, cross-workspace rules and SemOS
  discovery remain covered by its full locked suite. Stale fixtures were
  corrected to the admitted YAML policy rather than preserving contradictory
  Rust-era expectations.
- All 15 `ob-poc` semantic pack source hashes and artifact hashes remained
  stable when the compiler provenance label advanced from `0.2.0` to `0.2.1`.
- No serialization schema, canonicalization version, UUID namespace, database
  schema, model serializer or model bundle changed.
- Model inference was not rerun because no embedder code, weights, serializer or
  bundle changed. The Phase 4 bit-for-bit native comparison remains applicable.

## Deployment, shadow and rollback

No application image was built, promoted or deployed. Shadow inference,
production SBOM comparison and operational rollback belong to Phase 8 and were
not silently expanded into this source-cutover session.

Source rollback anchors are shared `f0e2552`, BPMN `cc924da`, and `ob-poc`
`ec0ba7dd`. Compatibility re-exports remain present, and the phase introduced no
database migration, so source rollback is a Git pin/revert operation. A deployed
rollback was not exercised because there was no deployment; Phase 8 must test it
against release-candidate artifacts before promotion.

## Known carry-overs

| Carry-over | Owner | Target |
|---|---|---|
| Build application release candidates, SBOMs, dependency receipts, shadow comparison and exercise rollback | deployment owners | Phase 8 |
| Remove compatibility re-exports only after both consumers have completed their deprecation window | shared-crate owners | Phase 9 |
| Repository-wide `ob-poc` Clippy remains red in unrelated code (`derivable_impl`, test-module ordering, `iter_kv_map`, and `await_holding_lock`) | `ob-poc` maintainers | separate hygiene tranche |
| `block 0.1.6` emits Cargo's future-incompatibility warning | `ob-poc` dependency owner | dependency refresh before the next Rust upgrade |
| BPMN `utterance-engine::CapturePipeline` has pre-existing test/dead-code warnings | BPMN mapper owner | mapper hygiene tranche |
| `v0.2.0` is a superseded immutable candidate and must not be consumed | shared release owner | retain for audit; select `v0.2.1` |
| Generated DSL remains large; CI should retain deterministic regeneration/drift checks | `ob-poc` DSL owner | permanent gate |

## Concurrent-work protection

The coordinating checkout's pre-existing `.DS_Store`,
`bpmn-lite-server-runner/src/bus_runtime.rs`, training evaluation/card/manifest
changes and `docs/.DS_Store` were not modified, staged or reverted. The
`ob-poc` `.cargo/config.toml.example` change was also left untouched. Ignored
development patch configs were restored. No unrelated work was included in any
Phase 7 commit.

## Gate decision

Gate 7 is green. Both applications compile and test against the same immutable
shared release, dependency direction is clean, compatibility hashes are stable,
and all observed differences are configuration-truth corrections covered by
tests. Proceeding to Phase 8 requires an explicit deployment instruction.
