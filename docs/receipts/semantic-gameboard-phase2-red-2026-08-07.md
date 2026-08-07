# Semantic Gameboard Phase 2 red receipt — 2026-08-07

## Scope and authority baseline

Phase 2 starts on branch `codex/bpmn-gameboard-refactor` at
`060273d479ebf6f73ce4734454bd1cf0c5926f97` (`refactor(api): establish sealed
gameboard capability facade`). The branch has no configured upstream. The admitted
shared contracts are pinned exactly to
`bc547723e6831cdb46fb8028071db3f537129d77` in the workspace and both standalone
fuzz projects.

The authoritative graph remains `DesignerDag`; `PositionalLegality` remains the
candidate legality oracle; `DesignerDag::admit` remains the verifier/lowering
admission boundary; human ratification remains mandatory. No model training or
corpus regeneration is part of this phase.

## Commands and inspected production path

The red audit recorded `git branch --show-current`, `git rev-parse HEAD`, complete
porcelain-v2 status, and inspected the Phase 1 facade plus the production path through:

```text
designer-graph/src/positional.rs
designer-graph/src/board_candidate.rs
designer-graph/src/ops.rs
designer-graph/src/productions.rs
designer-graph/src/schema.rs
utterance-engine/src/bpmn_pack.rs
utterance-engine/src/bpmn_board.rs
bpmn-lite-server-designer/src/proposal.rs
bpmn-lite-server-designer/src/rest.rs
```

The current graph-backed language path reconstructs the `DesignerDag`, computes its
revision/content/history identities, builds the admitted semantic candidate board,
retrieves evidence, applies deterministic disposition, creates a typed workbook,
materializes operations in the server, dry-applies them with `apply_production`, calls
`DesignerDag::admit`, then requires explicit ratification before the same operations
are re-staged, re-admitted and persisted.

## Expected red findings

Gate 2 is red for the following concrete reasons:

1. `DesignPosition::from_semantic_board` creates one compatibility move per semantic
   candidate. Whole-graph candidate deduplication therefore loses the concrete valid
   anchor set and does not produce position-bound pieces.
2. Known graph bindings are not projected into typed `MoveArgument` values. Required
   anchors are represented as ordinary missing legacy arguments.
3. There is no private `game_state` or `legal_moves` kernel and no deterministic
   canonical enumeration API shared by palette/direct-action and language paths.
4. The only complete operation materializer is application-private
   `bpmn-lite-server-designer::proposal::materialize_operations`; it mints `NodeKey`
   values with ambient UUID randomness and cannot be used by the capability crate or
   a hermetic fuzzer.
5. No non-mutating graph-delta preview exists for `DesignerDag` operations. The
   compiler's existing `dsl::AstMutator` mutates the textual DSL AST and is not the
   graph-backed Designer admission route.
6. Fully bound candidates are not dry-applied and passed through the same
   `DesignerDag::admit` boundary during legal-move construction.
7. Requested-but-inapplicable shapes, stable rule codes, governed explanation
   identities and recovery choices are not retained by the Phase 1 projection.
8. Direct graph actions do not resolve an equivalent governed `LegalMoveId`.
9. No compact independent Phase 2 reference model, structured graph/focus/binding
   operation tape, semantic coverage counters or legal-move/preview fuzz receipt
   exists.

## Frozen inventory and API/dependency baseline

The BPMN-Lite fuzz discovery baseline remains 20 targets. The shared contract project
has six targets. Phase 1's affected public-API baseline remains:

```text
utterance-engine default:             349 items, sha256 7f1f36952bd9535172cdf3e1fcb64b36ad765b9b9ab09a667515a95c31ceeaa1
utterance-engine candle-probe:        375 items, sha256 7807458d416bd782cc1f4b006055c98ff054b1b0067e394341c77a22637c045f
utterance-engine embed,candle-probe:  385 items, sha256 840e72e07b7aa510d49f9821abcdbb44fdd259ba9b632df7425a3e3ba0196d5f
utterance-engine q9-capture:          403 items, sha256 f439bed5d7b23c19da737aadc7b5ff1dec085c71b7caa8817b00d30c81d1094c
bpmn-lite-server-designer:              8 items under every checked feature
```

The exact full hashes are owned by
`scripts/baselines/semantic-gameboard-public-api-v1.json`. Capability dependency
direction is still application-inward: `utterance-engine` depends on shared contracts,
the compiler and `designer-graph`; the server depends on `utterance-engine`. No
capability crate depends on the server, fuzz projects or `xtask`.

## Protected pre-existing work

The pre-existing `.DS_Store` files, runner import-order edit, corpus and bundle
outputs, deleted split manifest, untracked normative documents, untracked v3 corpora
and training logs remain user/concurrent work. They are not part of Phase 2 and must
not be staged, formatted, regenerated or deleted.

## Red decision

Gate 2 is intentionally red. Implementation may proceed only through private or
`pub(crate)` mechanisms and a reviewed named adapter facade. If concrete enumeration
and compiler admission disagree, preview and apply deltas differ, or deterministic
reconstruction changes identities, Phase 2 must stop and report rather than weaken
the authority boundary.
