# BPMN semantic decision-board programme

**Status:** implementation authority
**BPMN base:** `b5b2844d9352b7f2ff56696ac34ce8e09813af98`
**Shared DSL base:** `a043e7f3d40262b78b367a6c18ac4a937c7498c6`
**ob-poc base:** `d76d8be9842c960e06841a4cc661d03ad44fbe73`
**Candidate schema at base:** 3
**Context schema at base:** 1
**Promotion posture:** shadow only; ratification always mandatory

The expected `dsl_bpmn_programme_v0.4.md` was absent from all three source
repositories. This file is the programme source required by Phase 1. The full
architecture and implementation plan supplied for this session remain the
normative design; this programme records live-base deltas and receipts.

## Absolute invariants

1. The model never creates, widens, or edits the legal candidate board.
2. The model returns finite, board-bound ranking evidence only. It never returns a disposition, DSL string, operation, binding, or ratification decision.
3. The BPMN graph and positional legality oracle remain the authority for what is meaningfully proposable.
4. Unknown anchors, arguments, graph identities, and semantic snapshots fail closed. They never downgrade to whole-graph or default values.
5. Every model-visible semantic field is content-addressed. A semantic change requires a schema/version change and moves the board hash.
6. Training, evaluation, and live serving call the same canonical turn and candidate serializers. No duplicate textualisation is permitted.
7. Binding remains deterministic and downstream of candidate selection. Missing values become workbook slots, not guessed values.
8. A bound plan must pass the existing `apply_production` and `admit` path before it can be previewed.
9. Ratification remains the only utterance-derived mutation door and must recheck graph revision and restage the plan.
10. Real-user capture remains structurally gated. Do not enable, widen, or work around `q9-capture`.
11. `designer-graph` remains utterance/model agnostic. Do not add Candle, tokenization, phrase, or model concepts to it.
12. The existing shared DSL/SemOS crates contain no `DesignerDag`, BPMN compiler, Candle, pgvector, Postgres, Axum, or host event-store dependency.
13. BPMN V1 scores the complete position-legal board unless benchmark evidence and an owner-approved decision introduce retrieval truncation.
14. The SLM never generates textual DSL. Typed `Operation` values are the authority; any text form is a deterministic view.
15. Outside the explicitly listed Phase 0 BPMN pack files and compatibility pin/fixture, do not edit `ob-poc`.
16. Authoring, service invocation, runtime operations, and infrastructure control are separate semantic planes.
17. Structural BPMN elements cannot be registered as successful no-op executable verbs.
18. Do not introduce `utterance-mapper-core` or a parallel ontology/policy/workbook crate.
19. BPMN depends on an immutable shared revision in committed manifests; local patches are development-only.

## Ownership matrix

| Concern | Owner |
|---|---|
| Verb identity, arguments, phrases, classes, pre/postconditions, effects | `sem_os_ontology::VerbContractBody` plus additive ontology shapes |
| State nodes and verb-labelled transitions | `sem_os_ontology::StateGraphDefBody` |
| Domain-pack identity, ownership and surface hash | `sem_os_policy::DomainPackManifest` |
| Resolved position, constraints and action surface | `sem_os_policy::GroundedActionSurface` |
| Semantic board, finite evidence and deterministic disposition | `sem_os_policy` |
| Generic workbook values and transition safety | shared DSL/SemOS types; host owns storage |
| BPMN graph legality and typed operation materialisation | `DesignerDag`, `PositionalLegality`, BPMN adapter |
| Retrieval, tokenization and Candle | BPMN `utterance-engine` |

`dsl-manifest` is the low-level BPMN invocation schema. It is not shared
`dsl-core`, and neither is a compatibility alias for the other.

## Cross-repository compatibility matrix

| Consumer | Shared revision | State |
|---|---|---|
| BPMN base | intended local patches at `a043e7f`; all unused | red baseline |
| ob-poc base | released shared crates consumed by its Rust workspace | compatibility baseline |
| BPMN mapper worktree | Phase 2 shared worktree during development | temporary only |

The shared revision must be committed before the BPMN consumer pin is frozen.
No push is performed by this programme without owner approval.

## Phase receipts

### Phase 0 — pack truth

- RED: six invocation DAG verbs versus eight manifest verbs.
- RED: authoring importer emits `bpmn:timer-wait` and
  `bpmn:message-wait`; the latter lacks an execution-plan representation.
- RED: `ob-poc` exposes four structural no-op plugin verbs.
- GREEN: commit `be0aa48` adds checked-artifact pack validation, removes the
  two stale invocation verbs, and preserves timer/message waits as first-class
  execution-plan nodes. The compiler/authoring/Designer gate is 257 tests green.
- GREEN: the isolated `ob-poc` pack gate reports 761 registered plugin verbs,
  761 declared plugin verbs, zero missing registrations and zero dead-code
  candidates after structural no-op removal. Commit `342fdd37` splits
  infrastructure control from the operations pack, removes all four structural
  no-op verbs and validates the stateless workspace root against the Phase 2
  shared revision. Its forced pack hash is
  `4511bf90e2f8a3355aeb7d32e1023fb7fb73843b55c81c72c006a1ab63ab40d8`.

### Phase 1 — executable baseline

- GREEN: position-specific legality, off-board refusal, no pre-ratification
  mutation, drift refusal and missing-value refusal already have permanent
  tests on `b5b2844`.
- GREEN: `docs/receipts/bpmn-candidate-coverage-v3.json` enumerates all 26
  candidates and truthfully marks every semantic contract absent before Phase 3.
- RED: the future coverage validator must refuse this intentionally incomplete
  inventory until the Phase 3 semantic registry supplies all boardable entries.

### Phase 2 — shared deterministic contracts

- GREEN: shared commit `fa51217` adds content-addressed semantic boards,
  complete finite evidence, a versioned disposition policy, decision records,
  typed proposal workbooks and a closed transition table to `sem_os_policy`.
- GREEN: board/evidence/policy deserialization reconstructs and verifies content
  hashes; all 49 workbook status pairs are covered; stateless domain packs no
  longer invent transitions merely to satisfy validation.
- GREEN: shared workspace check, 7 contract tests, 10 domain-pack unit tests,
  37 documentation tests, changed-package Clippy with `-D warnings`, layering
  guard and the `ob-poc` consumer compile/test all pass.
- KNOWN BASELINE: full dependency Clippy fails in unchanged `dsl-core` and the
  ignored domain-pack tests hard-code an absent repository-local `config`
  directory. The executable `ob-poc` pack check is green and is the relevant
  cross-repository receipt.
- RELEASE BOUNDARY: `fa51217` is a local isolated-worktree commit. It is not yet
  an immutable remote revision/tag, so Phase 3 must not activate a local path or
  unpinned dependency.

### Phase 3 onward

Receipts are appended here as each phase reaches green. A phase is not marked
complete solely because it compiles.

## Fuzz assurance amendment

Phase 9 includes the ten findings from the 4 August fuzz review. The immediate
blocking controls are:

- shard nightly target execution and require one completed receipt per target;
- commit the historical XML crash under the consumed regression directory and
  fail a zero-regression production gate;
- add controlled-clock authority/job-claim state machines;
- add PostgreSQL crash-cut, native/Wasm differential and resource-budget lanes;
- persist every project corpus/artifact and use reproducible locked fuzz builds.

Mapper fuzz targets are added only after the nightly schedule is made complete.
