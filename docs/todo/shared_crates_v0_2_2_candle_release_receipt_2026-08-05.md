# Shared crates v0.2.2 — Candle compatibility release receipt

**Date:** 5 August 2026

**Status:** released and consumed; external image promotion remains held

**Shared release:** `v0.2.2` / `a38eefe1e8d039bd8b52e52477ffd58ba39c3058`

## Outcome

The shared DSL/SemOS workspace now uses Candle 0.9.2. This removes the
future-incompatible Apple `block 0.1.6` dependency and replaces Candle's old
`metal` path with maintained `objc2` bindings. The workspace was released as
v0.2.2, and both real consumers now pin its exact immutable commit.

No public Rust API, pack schema, canonical identity, hash schema, tokenizer,
weights, model revision, embedding dimension, or query/target convention was
changed. This is a dependency-runtime patch release, not a semantic-contract
release.

## Repository ledger

| Repository | Branch | Start | End |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `refactor/sem-os-pack-policy` | `586431f81e2bb9101578af5167b8a35335f5a09e` (`v0.2.1`) | `a38eefe1e8d039bd8b52e52477ffd58ba39c3058` (`v0.2.2`) |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-policy-consumer` | `ccc14fa37d7c6abdd3c1f621577e848835c36892` | `d36a17794441e40a1e71cd1c89b265f897769e37` |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/bpmn-semantic-pack` | `d598d7e3c0eda7bac1e1379af2d635bca7bfeca2` | `18905e7c871f75d190cbd83a1d202706e5d7ae6b` |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `3fc978bc43d6d59eca1af6fc4c9ba4dc9583e3f4` | this receipt commit |

All implementation commits and the annotated v0.2.2 tag were pushed before
this receipt was written.

## Dependency change

Old shared embedding path:

```text
semantic-embedder 0.2.1
  -> candle 0.8.4
  -> metal 0.27/0.29
  -> block 0.1.6
```

New shared embedding path:

```text
semantic-embedder 0.2.2
  -> candle 0.9.2
  -> objc2-metal 0.3.2
  -> objc2/block2 0.6.x
```

`cargo tree` over the shared workspace and both exact-remote consumer graphs
finds no `block 0.1.6` package. The consumer lockfiles contain nine shared
package records at the v0.2.2 Git source and contain neither the old revision
nor a machine-local path.

## Numerical compatibility

A side-by-side harness loaded the same pinned BGE bundle from revision
`5c38ec7c405ec4b44b94cc5a9bb96e735b38267a` through the v0.2.1 and v0.2.2
implementations. It compared query and target embeddings for three different
texts (six vectors total).

The vectors were not byte-identical. The largest observed absolute
per-component difference was `1.50e-7`; individual probes ranged from
`1.02e-7` to `1.49e-7`. Model identity, dimensions, tokenizer, weights, and
normalisation semantics were unchanged. The release therefore records honest
floating-point equivalence rather than claiming byte equivalence.

The shared pinned-model test passed six tests plus its doctest with the same
bundle.

## Shared-workspace qualification

The following locked Rust 1.95 gates passed:

- `cargo fmt --all -- --check`;
- workspace tests with all targets and all features;
- strict workspace Clippy with `-D warnings`;
- rustdoc with `-D warnings`;
- `dsl-core` and `semantic-embedder` feature-matrix checks;
- layering, dependency-direction, and domain-neutrality scripts;
- publishable-package and leaf-package dry runs;
- pinned-bundle semantic-embedder tests; and
- `cargo deny check advisories bans licenses sources`.

The deny policy now explicitly admits ISC, Zlib, and CDLA permissive
transitive licenses and grants MPL-2.0 only to `option-ext@0.2.0`. Two
unmaintained-only transitive advisories are ignored by exact ID with reasons:
`RUSTSEC-2024-0436` (`paste`) and `RUSTSEC-2025-0119`
(`number_prefix`). Vulnerability, yanked-crate, unknown-Git-source, and license
enforcement remain active. The pristine v0.2.1 graph failed the same deny gate,
so this closes a pre-existing CI-policy defect as well as qualifying the new
graph.

## Consumer qualification

### BPMN semantic mapper

All commands used the committed exact-remote lock graph, bypassing the ignored
developer path patches:

- full locked workspace/all-target/all-feature check — pass;
- `utterance-engine` — 66 passed, 0 failed, 4 declared model-dependent
  ignores; candidate inventory, shared-contract compatibility, and doctest
  also passed;
- `bpmn-lite-server-designer` — 57 passed, 0 failed, 1 declared trained-bundle
  ignore; and
- old shared revision references — zero.

### `ob-poc`

- full locked workspace/all-target/all-feature check — pass;
- strict locked workspace Clippy on Rust 1.95 — pass;
- `ob-semantic-matcher` — 26 passed, 0 failed, 12 environment-dependent
  ignores, plus 9 public-API doctests passed;
- final database-backed full library run — 1,818 passed, 0 failed, 214
  declared ignores;
- an earlier aggregate run observed one PostgreSQL advisory-lock timing failure;
  that test passed 5/5 immediate isolated reruns before the clean final
  aggregate; and
- old shared revision references — zero.

The transient timing result is recorded as a pre-existing nondeterministic
database-test issue, not hidden and not attributed to the embedding runtime.

## Rollback

Rollback requires only repinning both consumers to shared v0.2.1 commit
`586431f81e2bb9101578af5167b8a35335f5a09e` and regenerating their lockfiles
from outside the developer patch configuration. No database, pack, identity,
wire, or canonical-hash migration is required. Phase 8 already proved
application artifact rollback and persisted semantic replay at that revision.

## Carry-overs

1. BPMN's application-owned `utterance-engine` cross-encoder probe still uses
   Candle 0.8.4 directly. It coexists with the shared embedder's 0.9.2 stack but
   does not reintroduce `block 0.1.6`. Upgrade it only with an admitted trained
   bundle and a cross-encoder score/latency compatibility receipt. **Owner:**
   BPMN model-serving owner. **Target:** next admitted bundle release.
2. Release-candidate container images and SBOMs in the Phase 8 receipt still
   label shared v0.2.1. Rebuild both images from consumer commits `18905e7c...`
   and `d36a1779...` before any external promotion. **Owner:** release/platform
   owner. **Target:** Gate 8 external-shadow entry.
3. `ob-poc` still has a broad pre-existing formatting baseline and a flaky
   PostgreSQL advisory-lock timing test. Neither is caused by v0.2.2. **Owner:**
   `ob-poc` maintainers. **Target:** repository hygiene/test-stability tranche.
4. The two exact unmaintained advisory exceptions must be removed when
   `tokenizers`, `hf-hub`, or Candle eliminate the upstream paths. **Owner:**
   shared dependency maintainer. **Target:** each dependency refresh.

## Worktree protection

The pre-existing `ob-poc/.cargo/config.toml.example` modification was not
committed. The coordinating BPMN checkout's unrelated `.DS_Store`, bus runtime,
and model-training artifact changes were not edited or committed. The
side-by-side parity harness was disposable and contained no source of record.
