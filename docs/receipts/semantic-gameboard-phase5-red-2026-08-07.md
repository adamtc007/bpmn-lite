# Semantic Gameboard Phase 5 red receipt

Date: 2026-08-07

Phase: 5 — replace top-one hand-off with game-aware disposition

Baseline commit: `1f4a130757e2e7d749b2f5e0313e2b3a9ff7178e`

Branch: `codex/bpmn-gameboard-refactor` (no upstream)

Status: RED. This receipt describes Phase 5 entry failures only. Gate 4 remains green.
No model training, weight generation or corpus regeneration was performed.

## Commands used for the red inventory

```text
rg -n "Phase 5|Gate 5|top-one|disposition" \
  docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md \
  docs/todo/EOP-VS-BPMN-GAMEBOARD-001.md
sed -n '749,822p' docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md
rg -n "enum .*Disposition|ProposeMove|ClarifyMoves|ProposalWorkbook" \
  utterance-engine/src bpmn-lite-server-designer/src \
  /Users/adamtc007/dev/dsl/crates/semantic-decision-contracts/src
sed -n '1,620p' utterance-engine/src/policy.rs
sed -n '3040,4515p' bpmn-lite-server-designer/src/rest.rs
sed -n '300,620p' bpmn-lite-server-designer/src/proposal.rs
sed -n '1120,1515p' \
  /Users/adamtc007/dev/dsl/crates/semantic-decision-contracts/src/lib.rs
```

## Reproduced red conditions

1. `utterance-engine::policy::decide` still selects from the top scalar score and a
   top-two separation margin. It does not decide over the complete position-bound
   `MoveEvidence` set or calibrated move probabilities.
2. The served `ProposalDisposition` vocabulary is the legacy six-way shape
   (`Candidate`, `Ambiguous`, `MissingArguments`, `Compound`, `OutOfScope`,
   `EscalateToSage`). It cannot represent the ten governed Phase 5 interactions.
3. Clarification compares only ranks one and two, and only one reciprocal contrast.
   There is no information-gain calculation over move, anchor and argument dimensions,
   and no path can surface a correct third-ranked move.
4. `ProposalWorkbook` stores candidate, board and graph identities, but not the selected
   `LegalMoveId`, `DesignStateId` or `MoveSetHash`. It can therefore prove candidate
   continuity but not the complete position-bound move continuity required by Gate 5.
5. The server serializes a position and stages a compiler-admitted graph candidate, but
   the workbook contract does not retain the canonical `GraphDeltaPreview` identity as
   part of its stale-safe state machine.
6. The initial utterance route returns before a typed attempt is appended for at least:
   unknown graph anchor, DAG reconstruction failure, IR projection failure, board/
   position/evidence construction failure, and internal serialization failures.
   Unknown-session and transport/storage failures are not position-bound attempts and
   remain honest technical responses.
7. Workbook answer validation and ratification re-stage/admission refusals can return
   before a terminal receipt/audit is persisted. Proposal-not-found is not reconstructible
   as a position-bound attempt from the current ephemeral store.
8. `Candidate` with suggestions/workbooks disabled records no terminal typed outcome;
   a successful exact interpretation likewise has no attempt receipt until a later
   terminal workbook transition.
9. There is no governed clarification-answer state, no correction request that links
   `correction_of`, and no preview/ratification/compiler-admission path for correction.
10. Sage receives rendered legacy disposition text rather than one closed typed packet
    containing the receipt, governed explanations and disclosure-filtered options.
11. No Phase 5 disposition/workbook operation-tape fuzzer exists. The Phase 4 target
    covers history and belief states but not clarification, answer, feedback, preview,
    revision drift and ratification as one reference-modelled state machine.
12. There is no independently receipted gold-in-top-three or clarification-success
    fixture set. Existing evaluation evidence therefore cannot satisfy Gate 5.

## Required red assertions

The coherent implementation must turn the following into executable passing tests:

- candidate permutation leaves the semantic disposition unchanged;
- removing evidence never creates proposal authority;
- hidden/off-board moves never enter clarification or feedback;
- rank three is reachable through one governed clarification when it maximizes expected
  information gain;
- every clarification field derives from admitted contrasts or argument schemas;
- workbook construction fails closed unless move ID, position ID, graph revision,
  move-set hash and candidate all match one current legal move;
- any graph, pack, focus, history or policy drift expires rather than rebases a workbook;
- hostile answers and compiler refusals do not mutate the graph and do append a typed
  terminal attempt/audit where a position-bound attempt exists;
- rejected/applied/corrected histories remain append-only and a correction is itself
  previewed, human-ratified and compiler-admitted;
- the public-facade state-machine target reaches every disposition and every attempt
  outcome with replayable minimized tapes.

## Authority and scope constraints retained

- The graph remains authoritative design state.
- Legal moves and admission remain compiler/verifier-owned.
- Evidence, belief and models cannot authorize a move.
- Human ratification remains mandatory; no automatic apply path may be added.
- Domain wording and recovery policy remain admitted-pack data.
- Generic shared contracts remain domain-neutral.
- Phase 5 will not retrain a model or regenerate unrelated corpora.
- All pre-existing/concurrent worktree changes recorded by Phase 0 remain protected.
