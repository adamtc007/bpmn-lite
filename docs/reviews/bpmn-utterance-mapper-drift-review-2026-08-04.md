# BPMN utterance-mapper drift review

**Reviewed base:** `b5b2844d9352b7f2ff56696ac34ce8e09813af98`
**Architecture source:** `bpmn_utterance_mapper_reviewed_architecture.md`
**Implementation source:** `zed_bpmn_utterance_mapper_implementation_plan.md`
**Fuzz amendment:** `bpmn_fuzz_testing_review.md`
**Review date:** 4 August 2026

## Decision

The architecture and implementation plan remain directionally correct, but their
source receipts describe an older checkout (`6e4de6d`,
`feat/lease-remediation-phase0`). The live implementation base is the later
DIR-002 training branch at `b5b2844`. None of the architectural gaps has been
silently closed by that branch, so the phase ordering remains valid.

## Confirmed current facts

- `designer-graph` now exposes 19 operation candidates and 7 production
  candidates at candidate schema version 3. Two duplicate-semantics productions
  described by older DIR-002 material have already been retired. The reviewed
  documents' 19/7 count is current.
- Graph-backed serving still builds the thin `utterance_engine::board::Board` and
  supplies `EmptyUniverse`; candidate-specific semantic contracts and a real
  semantic pack identity are absent.
- Production serving still relies on `retrieval::TIER1_K = 12` and the model pair
  still consists of generic context plus the thin candidate description.
- The shared `/dev/dsl` patches are still reported as unused by Cargo. BPMN does
  not currently consume `sem_os_ontology` or `sem_os_policy`.
- `proposal.rs` remains a terminal deterministic binder. Missing fields produce
  `MissingArguments`; there is no typed resumable workbook or answers endpoint.
- The invocation DAG contains six real handler-backed verbs while the checked-in
  manifest contains eight. `message-wait` and `timer-wait` remain stale manifest
  entries with no bus-handler route.
- Timer waits are first-class in the DSL execution plan. Message waits are
  first-class in compiler IR and bytecode (`V2WaitMsg`) but remain unsupported by
  the DSL execution-plan projection, which explains the lingering fake service
  task in the authoring importer.
- `ob-poc` still registers the four structural BPMN constructs as successful
  no-op `SemOsVerbOp`s, and its BPMN pack/domain/DAG/constellation seed files
  retain all pack-truth defects described by the forensic amendment.
- The fuzz review remains current at `b5b2844`: nightly still runs every target
  sequentially for 1,200 seconds inside one 180-minute job, compiler/server
  corpora and artifacts are still omitted, and every consumed regression
  directory is empty. The native/Wasm gate is still the single replay fixture.

## Baseline receipts

| Repository | Base | Result |
|---|---|---|
| `bpmn-lite` | `b5b2844` | `designer-graph`: 59 passed; `utterance-engine`: 30 passed; `bpmn-lite-server-designer`: 38 passed |
| `/dev/dsl` | `a043e7f` | `cargo check --workspace` passed; workspace tests have one pre-existing failure: `dsl-core/tests/verb_flavour_catalogue.rs::every_catalogue_verb_has_phase7_flavour` |
| `ob-poc` | `d76d8be9` | clean source baseline; narrow pack/registry gates are recorded in the phase programme |

The unrelated modified BPMN source/generated files in the owner's original
worktree were not copied into this worktree.

## Required amendments to the plan

1. Phase 0 must add the missing first-class message-wait execution-plan shape;
   removing the fake service verb without it would regress authoring fidelity.
2. Phase 6 must migrate from the already-landed candidate schema version 3 and
   preserve the DIR-002 corpus provenance. Existing generated bundle files must
   be regenerated, never edited.
3. Phase 9 incorporates every finding from the fuzz review. In particular,
   mapper fuzz targets cannot be added to the currently impossible sequential
   nightly schedule. Nightly sharding and a non-empty committed regression gate
   are prerequisites, not follow-up work.
4. The fuzz review's durable-authority, PostgreSQL, job-claim, native/Wasm and
   resource-budget lanes are execution-engine assurance work. They remain
   required release gates but are not allowed to leak authority concerns into
   the utterance mapper's types.
5. The reviewed command spelling `cargo xtask ...` is not installed on this
   checkout; repository gates use `cargo run -p xtask -- ...`. This is command
   drift only and does not change the findings.

## Conclusion

There is no architectural drift that invalidates the target design. There is
implementation drift in the form of additional DIR-002 model work and a more
precise 26-candidate catalogue. The plan is therefore executed from the current
SHAs with those amendments and with shadow-only promotion as the conservative
default until the owner ratifies thresholds.

## Implementation-time reconciliation

Live target discovery during FT-01 remediation found 15 fuzz targets, matching
the review's table but not its stated total of 16. Consequently the old nightly
schedule declared 300 minutes rather than 320; the critical conclusion is
unchanged because the job timeout was 180 minutes. The replacement workflow is
discovery-driven and therefore does not hard-code this count. See
`docs/receipts/fuzz-critical-gates-2026-08-04.md`.
