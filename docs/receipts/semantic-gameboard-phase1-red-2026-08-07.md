# Semantic Gameboard Phase 1 red receipt

**Date:** 2026-08-07
**Phase:** 1 — introduce the design-position and move contracts
**Gate:** RED (expected before implementation)

Phase 0 is green at BPMN-Lite commit
`a8ac2056d6d119e56da63589505a0e87e5f1393c`. This receipt records the
deliberately failing Phase 1 contract boundary; it does not claim later gameboard
phases are implemented.

## Entry commands and state

```text
git branch --show-current
codex/bpmn-gameboard-refactor

git rev-parse HEAD
a8ac2056d6d119e56da63589505a0e87e5f1393c

git -C /Users/adamtc007/dev/dsl branch --show-current
refactor/sem-os-pack-policy

git -C /Users/adamtc007/dev/dsl rev-parse HEAD
a38eefe1e8d039bd8b52e52477ffd58ba39c3058

git -C /Users/adamtc007/dev/dsl rev-list --left-right --count HEAD...@{upstream}
0  0
```

The DSL worktree was clean. BPMN-Lite contained exactly the protected changes listed
in the Phase 0 baseline receipt: the `.DS_Store` files, the existing import-order
change in `bpmn-lite-server-runner/src/bus_runtime.rs`, the user-owned corpus/bundle
outputs, the deleted split manifest, the two untracked normative Gameboard documents,
and the untracked corpus/training outputs. They remain outside Phase 1.

The BPMN-Lite dependency is pinned to the clean DSL entry revision:

```text
semantic-decision-contracts = { git = "https://github.com/adamtc007/dsl", rev = "a38eefe1e8d039bd8b52e52477ffd58ba39c3058" }
```

## Shared-contract baseline

```text
cargo public-api -p semantic-decision-contracts -sss
items: 255
sha256: f9f653ebd0a9ea6b11cfd6976f4a20ad5a6971a60d6c695423894853860865f5
```

The crate is independently buildable, MIT-licensed, has `unsafe_code = forbid` and
inherits `unreachable_pub = deny`. Its normal dependency closure contains only
`hex`, `serde`, `sha2` and `thiserror`; it has no application, server, fuzz-project or
`xtask` dependency.

## Expected failures establishing RED

At this revision:

- there is no versioned `DesignPosition`, explicit `DesignFocus`, `LegalMove`,
  `MoveAttemptReceipt`, correction-link, rule-explanation, feedback-option,
  disclosure, graph-delta, move-evidence or design-belief contract;
- unsuccessful/non-transition interactions cannot all be encoded as typed attempt
  receipts;
- there is no canonical hash closure or strict deserialization gate for those types;
- there are no shared-contract hostile-decode/round-trip fuzz targets, operation-tape
  reference model, governed regression manifest or semantic outcome/disclosure seeds;
- there is no compile-pass facade fixture or compile-fail internal-path/unchecked-
  constructor fixture for the shared release;
- Designer cannot expose a serialized compatibility `DesignPosition`;
- consequently Gate 1 is red even though all Phase 0 behavior remains green.

The implementation must make these conditions green without changing proposal,
ratification, compiler admission, graph mutation or runtime execution semantics.
