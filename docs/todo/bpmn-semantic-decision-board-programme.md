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

### Phase 3 — BPMN semantic profile

- GREEN: shared release `v0.1.6` / revision
  `fa51217ffd2218edea82c175e45ffa11d9eb7cf9` is pinned in the workspace and
  locked for all shared DSL/SemOS packages.
- GREEN: `bpmn_pack` exhaustively maps all 26 Designer candidates into shared
  semantic contracts with typed arguments, governed phrases, examples,
  contrasts, risk/action classes and deterministic binder capability.
- GREEN: `build_bpmn_semantic_board` uses `PositionalLegality`, refuses
  mismatched anchors, filters policy before model visibility, excludes
  unrepresentable candidates and delegates canonical hashing to shared SemOS.
- GREEN: cold dependency resolution, 59 Designer tests, 41 utterance tests and
  changed-package Clippy pass. Detailed evidence is in
  `docs/receipts/bpmn-semantic-board-phase3.md`.
- KNOWN BASELINE: the exact Clippy gate without `--no-deps` reaches two
  unchanged `bpmn-lite-compiler` lints. No lint suppression was introduced.

### Phase 4 — graph-backed serving cutover

- GREEN: the graph-backed endpoint now constructs and serves exactly one shared
  semantic board; legacy DSL-source sessions remain explicitly labelled
  `legacy_thin_v1`.
- GREEN: response hashes change with resolved position, unknown anchors remain
  HTTP 422, policy-hidden candidates never enter model input, and old thin-board
  evidence is rejected by board hash.
- GREEN: consented capture retains the full semantic board and the existing
  proposal/ratification suite remains unchanged and green.

### Phase 5 — exact evidence and full-board retrieval

- GREEN: one versioned semantic candidate serializer feeds lexical, embedding
  and Candle retrieval.
- GREEN: Unicode-normalized governed exact matching expands collisions and
  indexes only the filtered live board. The current profile has 52 normalized
  phrase keys, all unique; collision rate is 0.000000.
- GREEN: semantic evidence is rejected unless it covers the complete legal
  board exactly once. Candle has a dedicated full-board serving entry point;
  the K=12 helper remains compatibility/evaluation-only.
- GREEN: evidence trace records serializer identity, lanes, bundle identities,
  exact collision set and the full-board fact. Detailed evidence is in
  `docs/receipts/bpmn-semantic-serving-phase4-5.md`.

### Phase 6 — candidate-conditioned model and corpus v3

- GREEN (structure): serving/training share one bounded pair serializer; v3
  corpus generation reports 3,301 full-board records and zero retrieval misses.
- GREEN (admission): legacy/mismatched bundle cards fail before model load and
  serving degrades to an honestly identified tier-0 producer.
- GREEN (data controls): family/pair leakage and all-NOTA-training splits are
  refused; bundle cards close over every required schema, serializer, file,
  budget, calibration and split identity.
- BLOCKED (bundle): no compatible PyTorch environment is installed (the host
  Python is 3.14), no v3 weights were trained, and independent promotion
  evidence/threshold ratification remains an owner decision. Existing v2
  weights are intentionally not relabelled. See
  `docs/receipts/bpmn-candidate-pair-phase6.md`.

### Phase 7 — resumable proposal workbook

- GREEN: terminal binding is replaced by a shared typed workbook created from
  the selected board contract; positional arguments are now declared rather
  than held as undeclared server state.
- GREEN: the answers endpoint applies batches atomically, supports partial
  completion, dry-stages only complete workbooks and preserves inference as a
  separate response fact.
- GREEN: direct needs-input ratification, invalid/unknown/duplicate answers,
  answer-time drift and restart reuse all fail closed. Request-and-wait with a
  later data-reference answer dry-admits without pre-ratification mutation.
- GREEN: ratify/reject remain one-shot and graph mutation remains behind
  ratification. Detailed evidence is in
  `docs/receipts/bpmn-proposal-workbook-phase7.md`.
- GREEN (Phase 0 closure): all server-runner plan/graph/stack projections now
  exhaustively represent the first-class message-wait node; the all-feature
  workspace check no longer fails on that earlier integration gap.

### Phase 8 — ambiguity, abstention, compound boundary and audit closure

- GREEN: policy v2 derives clarification only from reciprocal live-board
  contrasts; legacy/undiscriminated close scores still escalate.
- GREEN: impossible-position abstention, hidden-candidate refusal and strict
  two-span compound fixtures are permanent. Compound evidence creates no
  workbook and compound execution remains explicitly deferred.
- GREEN: decision records contain a resolvable board dump and independently
  identified action-span producer; their hash moves with every recorded
  dependency tested.
- GREEN: append-only proposal audits retain workbook state, slot provenance,
  bound plan, dry-run diagnostics/hash, decision-record identity and event
  linkage through rejection, expiry or ratifying graph edit.
- GREEN: development capture retains the expanded closure only after explicit
  consent; the default live-user capture path remains structurally absent.
- RECEIPT: `docs/receipts/bpmn-disposition-audit-phase8.md`.

### Phase 9 — property/fuzz, CI and performance

- GREEN (mapper): eight property families cover the required canonical,
  collision, ordering, state-machine, parser and typed-answer boundaries.
- GREEN (fuzz): four bounded mapper targets bring discovery from the corrected
  baseline of 15 to 19. Each seed corpus completed 1,000 runs without a crash;
  the per-target nightly budget remains 1,200 seconds in independent matrix
  jobs.
- GREEN (reproducibility): all seven fuzz projects now carry cargo-fuzz-resolved
  locks. Runs use a neutral working directory and fail if a lockfile changes;
  repeat regression replay was byte-stable and executed F8-COMPILER-001.
- GREEN (CI): production gates explicitly cover mapper contracts, serializer
  identity, hermetic bundle refusal, proposal tests, discovered regressions and
  a generated performance receipt.
- GREEN (named features): workspace build, serial test and documentation gates
  pass with `postgres,database,embed,candle-probe`; model-dependent tests remain
  explicit ignores and ordinary tests cannot initiate a model load.
- BASELINE EXCEPTIONS: full-workspace formatting and `-D warnings` Clippy stop
  in unchanged DMN/kernel/compiler sources. Changed files pass rustfmt and no
  suppression or unrelated rewrite was introduced.
- RECEIPTS: `docs/receipts/bpmn-mapper-phase9.md` and
  `docs/receipts/bpmn-mapper-performance-2026-08-04.md`.

### Phase 10 — shadow rollout

- GREEN: `BPMN_MAPPER_ROLLOUT` has exactly three conservative stages:
  `shadow`, `suggest` and `workbook`; missing or unknown input is shadow.
- GREEN: graph-backed evidence and the actual producer identity are always
  recorded. Suggestions and workbooks are independently gated, legacy thin
  boards remain legacy-session-only, ratification stays mandatory and no
  auto-apply state exists.
- GREEN: permanent tests prove shadow serves neither a suggestion nor a
  workbook, suggest serves no workbook, and the existing workbook/ratification
  suite remains green.
- DECISION: remain shadow. Independent v3 metrics, a confusion matrix,
  confident-wrong review, Candle latency/memory and owner thresholds are
  absent; synthetic corpus facts are reported separately and do not authorize
  promotion.
- RECEIPT: `docs/receipts/bpmn-mapper-shadow-report-2026-08-04.md`.

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

### Critical gate remediation

- GREEN: FT-01 is closed with a manifest-discovered, per-target nightly matrix,
  a full time envelope for every target and an aggregation job that requires
  every discovered completion receipt.
- GREEN: FT-02 is closed with a minimized F8-COMPILER-001 input in the consumed
  regression tree, hash-governed manifest validation, unconditional production
  replay and runner-level rejection of an empty corpus.
- GREEN: FT-07's compiler/server corpus and artifact persistence omission is
  closed by per-target paths derived from discovery.
- GREEN (FT-10 lock discipline): every selected project receives an explicit
  locked metadata preflight; cargo-fuzz runs from a neutral directory and fail
  if the lock changes. All seven fuzz lockfiles are resolved by cargo-fuzz and
  a repeat regression run was byte-stable.
- CORRECTION: current discovery is 15 targets; the source review's table also
  totals 15 despite stating 16. The old workload was 300 minutes in a
  180-minute job, so the verdict is unchanged.
- RECEIPT: `docs/receipts/fuzz-critical-gates-2026-08-04.md`.
- GREEN (mapper tranche): semantic-board decode, phrase-index collision,
  workbook-transition and deterministic binding-extraction targets are added.
- REMAINING: authority models, PostgreSQL crash cuts, native/Wasm corpus
  differential execution, resource limits and fuzz telemetry remain open.
