# BPMN semantic mapper carry-over register

**Opened:** 4 August 2026

**Implementation branch:** `feature/bpmn-semantic-decision-board`

**Implementation tip at handoff:** `eb1b48b3e599a8eb929ec0d6139d07cf40249a02`

**Current production posture:** shadow; human ratification is mandatory

**Purpose:** record work that is deliberately outside the completed mapper
implementation or still needs independent evidence before promotion.

## Status boundary

The deterministic mapper implementation is complete through Phase 10. The
semantic board, evidence cascade, typed proposal workbook, audit record,
fail-closed rollout controls, property tests, fuzz targets and CI wiring are in
place. The items below do not invalidate that implementation result. They do
prevent an enterprise-release claim or promotion beyond shadow.

The following review findings are closed by the implementation and must not be
reopened as generic carry-over work:

- nightly fuzz execution is a per-target matrix with completion receipts;
- the permanent regression gate contains and replays the governed
  `F8-COMPILER-001` reproducer;
- compiler and server fuzz projects participate in discovery, caching and
  artifact handling;
- quiet-phase recovery inspection fails closed;
- the mapper has four bounded fuzz targets.

The "reproducible fuzz lockfiles" claim from the original handoff did NOT
hold on a machine with local-dev `~/.cargo/config.toml` patches (a normal,
deliberate cross-repo dev setup) — `cargo xtask fuzz regress` was red there.
Fixed 2026-08-05 via a scratch `CARGO_HOME` in the fuzz runner
(`xtask/src/fuzz.rs::neutral_cargo_home`); re-verified byte-stable across two
independent runs. See the programme document's Remediation section (R3).

## Carry-over summary

| ID | Priority | Area | State | Promotion impact |
|---|---|---|---|---|
| CO-01 | P0 | branch integration and release ordering | ready for owner action | blocks integration |
| CO-02 | P0 | v3 model training and bundle admission | externally blocked | blocks `suggest` |
| CO-03 | P0 | independent quality evaluation and thresholds | evidence absent | blocks `suggest` |
| CO-04 | P1 | production model latency and memory qualification | depends on CO-02 | blocks model promotion |
| CO-05 | P1 | seven binder-unrepresentable BPMN actions | design/engine work absent | limits legal boards |
| CO-06 | P0 | durable execution-authority model fuzzing | not started in this tranche | blocks enterprise authority claim |
| CO-07 | P0 | PostgreSQL crash/chaos qualification | not started in this tranche | blocks multi-replica claim |
| CO-08 | P1 | native/Wasm differential and resource budgets | not started in this tranche | blocks runtime equivalence claim |
| CO-09 | P1 | corpus/coverage/admission governance | partially implemented | weakens long-term fuzz assurance |
| CO-10 | P2 | historical workspace gate cleanup | pre-existing baseline | blocks whole-workspace clean claim |
| CO-11 | P1 | shadow evidence and rollout decision | awaits real traffic and owners | keeps rollout at `shadow` |
| CO-12 | P2 | isolated `ob-poc` generated edits | unrelated/uncommitted | must not contaminate integration |

## CO-01 — Integrate the three repository branches in dependency order

Updated 2026-08-05 after a six-agent review found `ob-poc`'s handoff commit
unbuildable (see the programme document's Remediation section, R1) and
reworked it. Current published branches:

1. shared DSL/SemOS: `feature/sem-os-decision-board` at `edded43` — the
   tagged `v0.1.6` (`fa51217`) plus an additive test-only commit
   (missing Phase 2 red receipts, R5) NOT yet included in that tag. Decide
   whether to cut `v0.1.7` before integrating, or integrate at `fa51217` and
   land `edded43`'s tests separately.
2. `ob-poc`: `fix/bpmn-pack-truth` at `d2afc0c4` (force-pushed 2026-08-05,
   superseding `342fdd37`) — scope-narrowed to the authorized yaml/registry
   surgery only; the stateless constellation-map-root fix is a separate,
   still-open, surfaced fork (see R1 and finding P7), not resolved by this
   commit.
3. `bpmn-lite`: `feature/bpmn-semantic-decision-board` at `f4fa613` (was
   `eb1b48b` at original handoff; nine remediation commits since, closing
   R2–R9 and part of R11/R12 — see the programme document's Remediation
   section for the full list).

No pull requests or merges were created. Integrate the shared contract first
(after the `v0.1.6`/`v0.1.7` decision above), then the pack-truth change,
then the BPMN mapper. Do not replace the reviewed shared revision with an
unreviewed moving branch.

**Owner:** repository maintainers.

**Done when:** all three commits are reachable from their intended protected
branches, dependency resolution still selects the reviewed shared revision,
and the mapper contract, pack, regression and server gates pass on the merged
heads.

## CO-02 — Train and admit a candidate-conditioned v3 bundle

The v3 serializer, corpus schema, bundle card checks and admission boundary are
implemented. No v3 weights were trained. The host used for the implementation
had Python 3.14 and no compatible PyTorch installation; the existing v2 model
was correctly not relabelled as v3.

Run training in a pinned, supported Python environment and retain the exact
corpus, split manifest, serializer hash, semantic snapshot and producer bundle
hash used. Admission must reject serializer, candidate-contract or snapshot
drift.

**Owner:** model/training owner.

**Done when:** a reproducible v3 bundle passes the committed bundle validator,
loads through the Candle path, has a reviewed bundle card and is immutable by
content hash.

## CO-03 — Produce an independently authored evaluation and ratify thresholds

No independent v3 evaluation set, confusion matrix, ambiguity/NOTA study,
confident-wrong review or owner-approved quality threshold exists. Synthetic
training examples and constructed exact-phrase precision are pipeline evidence,
not promotion evidence.

The evaluation must include at least:

- top-1 and per-candidate results, not only an aggregate score;
- a complete confusion matrix and reviewed high-confidence mistakes;
- ambiguity and NOTA precision/recall;
- full-board versus bounded-retrieval comparison;
- compound utterances, questions and unsupported but coherent BPMN requests;
- multi-instance requests missing ceilings;
- timeout versus non-interrupting-notification near neighbours;
- binding completion and dry-admission/refusal rates.

Keep the evaluation authorship and held-out data independent of training data
generation. Decide thresholds before inspecting the final test result.

**Owner:** product risk, BPMN domain and model-evaluation owners.

**Done when:** the independent report, threshold decision and confident-wrong
review are versioned, reviewed and linked from the shadow report.

## CO-04 — Qualify Candle serving latency and memory

Native semantic-board and serializer measurements exist, but there is no
admitted v3 bundle with which to measure Candle inference. The missing receipt
must cover cold load, warm inference, request memory and the actual legal-board
distribution. Board sizes that cannot occur must not be fabricated.

At minimum record batch sizes 4, 8 and 12 and the largest observed legal board;
include p50, p95 and p99, peak RSS or allocator-backed request deltas, bundle
size and hardware/toolchain identity. Define the production budget before
turning the measurement into a gate.

**Owner:** serving/performance owner.

**Done when:** the admitted v3 bundle meets owner-ratified latency and memory
budgets without changing deterministic disposition semantics.

## CO-05 — Decide and implement the seven unrepresentable BPMN actions

The 26 semantic contracts currently divide into 14 directly supported binders,
five typed-workbook binders and seven deliberately excluded actions:

- create race;
- close parallel region;
- rollback guard;
- call subprocess;
- timer/message race;
- human review with rework;
- durable subprocess production.

They are absent from legal production boards, so the mapper cannot select an
operation that the binder or engine cannot represent. Adding phrases or model
labels before adding typed execution support would violate this invariant.

**Owner:** BPMN compiler/runtime and domain-pack owners.

**Done when:** each action is either explicitly rejected as a product non-goal
or gains a typed contract, binder/workbook representation, dry admission,
execution semantics, audit coverage and tests before entering a legal board.

## CO-06 — Add durable execution-authority state-machine fuzzing

The mapper fuzz work does not close fuzz-review findings FT-03 and FT-04. Add a
compact reference-model schedule fuzzer for transition leases, durable
activations and claimed jobs using the production tokenised APIs.

Required behaviours include claim, renewal, expiry, takeover, release, commit,
lost response, stale completion/failure and same-owner ABA schedules. Use a
controllable clock. A simulated crashed executor must be permanently removed
from the actor set. Compare process revision, current authority, job state,
receipt and journal outcome after every operation, not only at the end.

**Owner:** durable-engine/store owner.

**Done when:** minimized deterministic schedules exercise the production claim
surface and prove stale or crashed actors cannot mutate current authority.

## CO-07 — Run PostgreSQL crash and reconnect qualification

FT-05 remains open. High-throughput libFuzzer targets intentionally use memory
stores; they do not establish PostgreSQL MVCC, fencing or transaction claims.
Replay minimized authority schedules against real PostgreSQL with at least two
executor identities and two pools/connections.

Inject connection loss immediately before and after commit, transaction
rollback, lease expiry/takeover, replica/process termination and restart. Check
head revision, journal, activation, claim and receipt invariants after each cut.

**Owner:** persistence/reliability owner.

**Done when:** the chaos suite is reproducible in CI or a governed qualification
environment and its minimized failure tapes are committed as regressions.

## CO-08 — Generalise native/Wasm differential execution and resource limits

FT-06 and FT-09 remain open. The existing Wasm gate uses one fixed fixture. It
must consume a bounded corpus of portable artifact/snapshot/command packets and
compare native Rust with Wasmtime for canonical transition bytes, typed errors,
final snapshot hash and journal bytes.

Add explicit product limits for accepted artifacts and snapshots, decode
allocation, Wasm fuel, linear memory, transition time and fibre/effect
amplification. Failure must be deterministic and leave the worker pool healthy.

**Owner:** kernel/Wasm and platform-safety owners.

**Done when:** pull requests run a bounded differential corpus, nightly runs the
evolved corpus, and resource-limit breaches produce governed typed failures.

## CO-09 — Finish fuzz corpus and coverage governance

The critical scheduling, regression and lockfile defects are fixed, but the
remaining FT-10 governance work is not complete. Add scheduled corpus merge or
`cmin`, target-level coverage baselines, valid-input/admission rates, semantic
event frequencies and proof that coverage loss is reviewed. Retain per-target
receipts and avoid treating corpus file count as a coverage metric.

**Owner:** fuzz/CI owner.

**Done when:** every target has a governed baseline and trend receipt, corpus
growth is periodically minimized, and material coverage or admission-rate
regression requires explicit review.

## CO-10 — Clean the historical whole-workspace gate baseline

Mapper-changed source passes formatting and changed-package Clippy with
`-D warnings`. Whole-workspace gates still report unrelated historical issues:

- DMN source formatting drift;
- two collapsible-`if` warnings in `bpmn-lite-kernel`;
- a match-like-`matches!` and a too-many-arguments warning in
  `bpmn-lite-compiler`;
- existing rustdoc link warnings.

These were not suppressed or reformatted in the mapper branch because doing so
would mix unrelated code into the implementation review.

**Owner:** affected crate maintainers.

**Done when:** a separate focused change makes workspace formatting, Clippy and
rustdoc policy green on the chosen toolchain.

## CO-11 — Collect shadow evidence and make an explicit rollout decision

Keep `BPMN_MAPPER_ROLLOUT` missing or set to `shadow`. Unknown values also fail
closed to shadow. Shadow records evidence but returns no suggestion and creates
no workbook. `suggest` exposes a suggestion without a workbook; `workbook`
enables staging but still requires explicit human ratification. There is no
auto-apply mode.

Operational evidence must distinguish the actual producer used after bundle or
embedding degradation. Review abstention, ambiguity, compound refusal,
candidate distribution, board size, dry-admission and confident-wrong samples
without storing uncontrolled utterance content.

**Dependencies:** CO-02, CO-03 and CO-04.

**Owner:** product/risk and operations owners.

**Done when:** owners record a signed stage decision with rollback criteria.
`suggest` is the maximum next stage; `workbook` requires a later, separate
decision. Human ratification remains permanent.

## CO-12 — Preserve the `ob-poc` worktree boundary

The isolated `ob-poc-bpmn-pack-truth` worktree contains unrelated generated DSL
edits after commit `342fdd37`. They were deliberately not staged or included in
the pushed pack-truth branch. Their provenance and intended owner were not part
of this programme.

**Owner:** `ob-poc` DSL generation owner.

**Done when:** those edits are independently reviewed, committed or discarded
by their owner; no mapper or pack-truth integration action should absorb them.

## Recommended execution order

1. Integrate the reviewed branches (CO-01) while retaining shadow posture.
2. In parallel, build the v3 bundle and independent evaluation assets
   (CO-02/CO-03), and implement engine authority qualification
   (CO-06/CO-07).
3. Run v3 performance qualification (CO-04) and native/Wasm/resource work
   (CO-08).
4. Decide the seven product/runtime gaps (CO-05) and institutionalise fuzz
   trends/minimisation (CO-09).
5. Clean historical baseline issues separately (CO-10).
6. Use real shadow evidence to make a staged rollout decision (CO-11).

## Release rule

No unavailable evidence cell may be inferred from synthetic data, an older
bundle or a native-only microbenchmark. Until CO-02, CO-03 and CO-04 are closed
and CO-11 records an owner decision, the supported operational state is
`shadow`. Until CO-06, CO-07, CO-08 and CO-09 are closed, do not claim that
multi-replica durable execution authority has been fuzz/model/chaos qualified.
