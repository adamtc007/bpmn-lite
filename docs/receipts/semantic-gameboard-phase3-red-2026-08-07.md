# Semantic Gameboard Phase 3 red receipt — 2026-08-07

## Scope and entry state

Phase 3 starts on branch `codex/bpmn-gameboard-refactor` at
`ece6b59168a90333ca680adb16b8360beba9c867` (`feat(designer): enumerate explain
and preview concrete legal graph moves`). The branch has no upstream. Gate 2 is green
and the exact shared-contract pin is
`bc547723e6831cdb46fb8028071db3f537129d77`.

The authoritative `DesignerDag`, concrete legal move set, compiler preview/admission,
workbook state machine and explicit human ratification boundary are frozen inputs to
this phase. Evidence may rank or explain only those moves. This phase will not train
weights, regenerate corpora or make evidence authoritative.

The pre-existing `.DS_Store` files, runner import-order edit, corpus/bundle outputs,
deleted split manifest, untracked normative documents, untracked v3 corpora and
training logs remain protected concurrent work.

## Commands and traced path

The audit recorded branch, HEAD and complete worktree status after the Phase 2 commit,
then inspected:

```text
docs/todo/EOP-VS-BPMN-GAMEBOARD-001.md §6 and §7
docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md Phase 3 and Gate 3
bpmn-lite-server-designer/src/rest.rs::retrieve_utterance_evidence
utterance-engine/src/contract.rs
utterance-engine/src/exact.rs
utterance-engine/src/retrieval.rs
utterance-engine/src/trained_ranker.rs::Tier1Ranker::rank_full_board
utterance-engine/src/policy.rs
utterance-engine/src/bpmn_board.rs
utterance-engine/src/bpmn_pack.rs
utterance-engine/config/bpmn-semantic-pack.yaml
utterance-engine/config/bpmn-semantic-pack.lock
semantic-decision-contracts shared evidence and MoveEvidence contracts
```

The live graph-backed path constructs the admitted semantic board and concrete design
position, but evidence still runs against the candidate-level semantic board. The
server chooses exactly one producer (`Candle`, else embedding, else lexical), calls
`finalize_semantic_evidence`, then sends one scalar score per semantic candidate to
the existing deterministic disposition policy. Workbook creation later projects that
candidate selection back onto the concrete position.

## Expected red findings

Gate 3 is red for the following concrete reasons:

1. `DesignerState::retrieve_utterance_evidence` is the explicitly documented exclusive
   `Candle -> embedding -> lexical` fallback chain. Available producers do not
   contribute simultaneous lanes.
2. `SlmResult` contains one final scalar per semantic candidate. `EvidenceTrace`
   records only a global set of lane names and bundle identities; it does not record
   raw, normalized and fused lane values per concrete legal move.
3. The shared `CandidateEvidence`/`InferenceEvidence` and gameboard `MoveEvidence`
   contracts are not materialized in live BPMN serving.
4. The current shared `EvidenceLane` enum has only governed-exact, deterministic-
   grammar, lexical, embedding and Candle variants. It cannot represent typed
   argument, graph-local, structural-completion, history, abstention or correction
   evidence. Widening this generic contract must occur in the shared DSL repository;
   BPMN vocabulary must not be introduced there.
5. Multiple concrete moves can share one semantic candidate. The current evidence
   record cannot distinguish their anchors, bindings or move identities, so it cannot
   give every legal move exactly one complete vector.
6. Exact evidence currently mutates final candidate scores directly. There is no
   versioned fusion policy, weight identity, deterministic rule dominance or canonical
   probability record.
7. Typed extraction and binding remain conflated in the server proposal helpers.
   Extracted duration/count/node/data values are not first-class evidence proposals
   with provenance before workbook validation.
8. The current context projection supplies useful graph text to the learned ranker but
   there is no per-move graph-locality producer. Focus/anchor compatibility therefore
   is not recorded as its own deterministic lane.
9. The admitted YAML pack owns phrases, arguments, applicability and contrasts, but it
   has no admitted feature declarations, fusion weights, governed rule-explanation
   resources or recovery links. Current pack admission consequently cannot reject
   unknown feature kinds, invalid weights, dangling feedback references or
   contradictory deterministic gates.
10. Governed explanation construction uses pack applicability text, but the generic
    message/rule mapping and recovery policy are still assembled in Rust rather than
    admitted as pack resources.
11. Rejection/correction facts are not accepted as explicit evidence inputs and there
    is no cement proving they can alter evidence without altering legality. Phase 4
    will persist history; Phase 3 still needs a pure bounded prior-attempt input and
    lane behavior.
12. No evidence-fusion or semantic-pack-admission fuzz target exists. Duplicate,
    missing, reordered, non-finite and extreme lanes, candidate permutation,
    canonical-equivalent input and irrelevant-history metamorphisms are not covered.

## Frozen assurance baseline

The BPMN-Lite fuzz inventory is 22 targets; the two Phase 2 targets and their named
seeds are green. The governed regression manifest contains one case. The affected
public API baseline is:

```text
utterance-engine default:             385 items, sha256 b702df64451b81ab90e40e6109e3f1ebedd00504b8c0eba76e26a30366ac064e
utterance-engine candle-probe:        411 items, sha256 4b7ba8574ee5001ddc3f05d0dc224b5e696ea92b5eb695cb4783a9909daef4f5
utterance-engine embed,candle-probe:  421 items, sha256 089f786621b4d692373aaf75c29d443ee57932884811f988cdec4e9c6b3298a3
utterance-engine q9-capture:          439 items, sha256 ce02b0c7016e090cf81c0d4284a7f95948d60b096c185b19096c8c3fe89b3686
bpmn-lite-server-designer:              8 items under every checked feature
```

Dependency direction is unchanged: shared contracts and semantic-pack are outward
dependencies of `utterance-engine`; the application server depends inward on the
capability. No producer, fusion implementation, generator or fuzz support type may
enter the public surface.

## Red decision and dependency condition

Gate 3 is intentionally red. Implementation must start by extending the generic
shared evidence-lane contract without BPMN or `ob-poc` vocabulary, then pin the exact
reviewed shared commit. Because BPMN-Lite consumes the shared repository by immutable
Git revision, a new shared commit must be reachable before the BPMN pin can verify;
that push is an external state change and requires explicit authorization if existing
authorization does not cover this new commit.

Implementation must stop rather than weaken the design if evidence changes the legal
move set, a producer can omit a current move, pack resources require host hard-coding,
non-finite or duplicate lanes can be admitted, or a test/fuzzer needs implementation
visibility.
