# EOP-PLAN-BPMN-DESIGN-003 — Implementation Plan: BPMN Designer, Sage/Repl UI, and the SLM Prototype

**Version:** v0.2
**Status:** **RATIFIED (Adam, 2026-07-27)** — executes EOP-VS-BPMN-DESIGN-003 **v0.6 (RATIFIED)**; restructured per **EOP-DIR-BPMN-DESIGN-003-001** (SLM capability built IN-PHASE, concurrent with the Designer refactor). Changes from here are versioned amendments; receipts append in place (§F).
**Document class:** Implementation plan (workstreams, gates, receipts)
**Baseline:** EOP-VS-BPMN-ISA-002 v0.19 IMPLEMENTED; EOP-VS-BPMN-DESIGN-003 v0.6 ratified 2026-07-25
**Working discipline (inherited, binding):** GRIND vs CAREFUL tiers; authorship-blind review at every CAREFUL close; Rule 7 — substrate/plan mismatch = executor HALTS and reports, never adapts; red→green for every remediation; receipts appended to this document per workstream; build proof over assertion; zero suppressions; every code claim marked and traced before anything rests on it; per-site rules converted to build failures when a class recurs.

## Changelog

**v0.1 → v0.2 (EOP-DIR-BPMN-DESIGN-003-001).** Sequencing restructure ONLY — no ratified constraint weakened, no gate criterion dropped (see the delta table, §D). The serial T1 → T2 → T3 layout becomes three concurrent workstreams (WS-A `designer-graph`, WS-B `designer-ui`, WS-C `utterance-engine`) plus the governance track. Tier-1 is present from the start — in **shadow** — rather than inserted later. G2's "SLM-insertion readiness" item is obsolete as a *readiness check*: the seams it verified (board object, single disposition function, I28 decision record) are WS-C's interfaces, implemented in WS-A/WS-B from the first commit because WS-C consumes them immediately; G2 now verifies the running pipeline instead (strictly stronger). C5 trace promoted to an immediate task. Tier-0 corpus-baseline evaluation explicitly flagged to Adam for a charter-timing call rather than assumed charter-exempt.

---

## A. Ratified constraints — restated, binding, unweakened

These do not change under the in-phase restructure. Each is stated as ratified; the restructure sequences *within* them.

1. **D17** — the Q9 governance charter gates all live-session capture AND any training use of the existing 30k corpus (retrospective application first). Building in-phase moves the charter onto the critical path *immediately*; it does not bypass it.
2. **D18 + the scope ruling** — Designer design sessions only; promotion ceiling shadow → suggest-only → staged-patch; G3's absolute criteria are the only promotion path. "In this phase" changes when the code exists, never how promotion is earned. **Prototype cap (binding on every workstream):** the SLM's promotion ceiling in this plan is Designer suggest-only → Designer staged-patch. No workstream, task, or "quick win" touches runbook suggestion or any execution-adjacent surface. Equivalently by domain: this programme targets BPMN Designer design sessions (utterance → DSL/graph template construction) only — not the wider ob-poc onboarding/KYC pack surfaces. Rollout to those packs, if the SLM proves its worth, is a separate promotion with its own corpus (seeded by this programme's charter-governed capture), threat model, and gate — proposed as a new V&S when the G3 evidence exists.
3. **D19 (with rider) and D20** — denial semantics (helpful about the path forward, generic about the request; never confirms existence, never enumerates; `Forbidden` reachable only from explicit references) and Sage's board-transition protocol (explain absence → governed context change → new content-addressed board → rerun; never silent expansion) as ratified.
4. **The stable contract set** — `SlmResult`/`FiniteScore`/canonical tie-break/`NONE_OF_THE_ABOVE`/board content-hashing/I28 closure (`disposition_policy_hash`, `context_projection_hash`, `retrieved_subset_hash`, board + bundle hashes; recorded values are the historical truth, re-inference forensic) — and the T3.4a bake-off criteria: Apache/MIT license; ~100–300M params; **architecture loadable in Candle, verified not assumed**; CPU-friendly; train in Python, serve in Candle.
5. **Gate G1** (verdict parity with direct compilation) and **G3** (absolute criteria, thresholds set by Adam at gate time) unchanged.
6. Rule 7, marking discipline, GRIND/CAREFUL tiers, blind reviews at CAREFUL closes, receipts.

---

## B. Shape of the programme — concurrent workstreams

**The Q9 charter kickoff is this plan's first line and its critical path. Under in-phase build, every week of charter delay is a week the finished shadow pipeline runs without data it is allowed to keep.**

```text
GOV   Q9 charter kickoff                 starts IMMEDIATELY — critical path
WS-A  designer-graph      (was T1)       gate G1: verdict parity
WS-B  designer-ui         (was T2)       gate G2: end-to-end authoring + pipeline-in-loop
WS-C  utterance-engine    (was T3)       C-now (ungated) ∥ C-gated (Q9) ∥ C-bakeoff — gate G3
T4    In-browser oracle   (unchanged)    CONDITIONAL — entry gated on C1 + Q23
```

WS-A, WS-B, and WS-C's C-now run **concurrently** with explicit cross-stream interface points (§C ordering constraints). Every open code claim in the V&S §0 C-table is an **entry task of the workstream that depends on it** — never a background assumption. A failed trace is a Rule-7 HALT producing a substrate ask, not an improvisation.

### GOV — Governance track (starts immediately, finishes last)

**GOV.1 — Q9 data-governance charter (bank-side, longest lead time).** Owner: Adam + whoever governs data use. Deliverables per V&S D17: permitted/prohibited fields; redaction before persistence; retention and deletion; separation of evaluation, training, and audit datasets; access controls; consent/lawful-use basis; correction-into-training policy; model and dataset lineage; contamination protection. **This gates WS-C's C-gated items; starting it first is the schedule-critical decision of this plan.** The existing 30k corpus falls under the charter retrospectively before any training use.

**GOV.2 — Crate and repo placement.** Decide where the Designer crates live. v0.1 presumed the ob-poc workspace; **Adam's 2026-07-27 standalone ruling (EOP-SAGE-REPL-BPMN-001 T0: BPMN-Lite deploys independent of ob-poc) is a direct input** — the presumption is now the bpmn-lite workspace/standalone deploy unit, decision recorded here when made. Names: `designer-graph` (WS-A), `designer-ui` (WS-B), `utterance-engine` (WS-C). L1 layering and the public-API surface gate apply from the first commit.

**GOV.3 — Fixture-corpus skeleton.** Seed the corpus directory from ISA-002 §30's normative table: one fixture per deviation row (D3 SESE/cyclicity, D4 guard-failure, D5 MI ceiling, D6 empty-MI, D7 zero-match) plus V-10 refusal, dangling-fork refusal, and a nested-fork/pairing topology. Grows through WS-A.4.

Tier: GRIND except GOV.1 (not code).

### WS-A — Substrate-contact crate (`designer-graph`) — Phase D1 content, unchanged

#### WS-A.0 — ENTRY TRACES (CAREFUL, findings-only, blind-format receipts)

| Trace | Claim | HALT condition |
|---|---|---|
| C2-residual | `compute_post_dominators` (and the region/pairing derivations over it) is consumable at a crate boundary the Designer may depend on under L1 | Not exposed → HALT; substrate ask = export or thin wrapper in the compiler crate, no logic duplication. *Note: R8 (2026-07-25, task receipts in EOP-FUZZ programme) exposed the pairing oracle at the compiler crate boundary — the trace verifies that receipt against L1, expected green.* |
| C3 | The dsl macro runtime's named placeholder bindings: representation, typing, whether anything is creation-order-dependent | Order-dependence found → Q15 resolves toward a versioned durable representation |
| C4-residual | Write the deliberate design note: mapping of nested tagged-union envelopes onto instance flags vs `domain_payload`, within `MAX_VALUE_ARRAY_LEN/DEPTH` | Contradiction with §28's data model → HALT and redesign the note, not the substrate |

#### WS-A.1 — Canonical DAG schema + declaration model (CAREFUL — design-bearing, not grind)

Closes V&S Q2 and Q27 in one pass: node identity, region nesting, connector keys, adapter bindings, provenance fields; and **where declarations canonically live** — the schema and edit log carry MI maxima, guard budgets, cycle bounds, and correlation sources as first-class fields, compiled into the real envelope tables, with the DTO surface explicitly bypassed (C7). Blind review before WS-A.2 starts.

**Cross-stream interface (sequenced FIRST within WS-A.1): the board-candidate schema.** WS-C's board constructor consumes candidate identity and legality derivation from this schema — node identity, region descriptors, and the enumeration of legal operations/productions at a position (`DesignerBoard.legal_operations`/`legal_productions` per V&S §11.7). This slice of WS-A.1 lands and freezes early so WS-C's C-now board service is never blocked on the rest of the schema.

#### WS-A.2 — Graph operations + productions as deterministic builders (GRIND against WS-A.1's schema)

The §12.1 operation set and §12.2 production set as `fn apply(dag, anchor, bindings) -> Result<GraphPatch>`; the staged-candidate transaction (copy/validate/diff/ratify); the gateway incident edge inserted by default connector rules. Structural derivation consumed from the compiler per C2's trace — reimplementation is a review-rejectable defect.

#### WS-A.3 — Declaration authoring into the envelope (GRIND)

`v2_guard_budgets` + `default_guard_budget`, `v2_corr_sources`, MI maxima, cycle `max_fires` — authored values flowing into the sealed artifact through the production compiler, round-tripped through the edit log.

#### WS-A.4 — Fixture corpus completion + G1 harness

**GATE G1 (CAREFUL close, blind review):** for every corpus fixture, the production-built candidate's admission verdict — including *which theorem* rejects the invalid cases — is identical to direct compilation of the equivalent source. Layering and public-API gates clean. Receipts: per-fixture verdict table.

### WS-B — Sage/Repl UI shell (`designer-ui`) — Phase D2 content, minus the readiness item, plus day-one policy wiring

**WS-B.1** Repl strip: staged diff, verifier diagnostics naming the failing theorem, ratify/reject. (GRIND)
**WS-B.2** DAG surface: nodes/connectors, guard extents with triggers and *effective* budgets (declared or inherited default), correlation sources on wait nodes, Terminate/Incident/limit visual distinction, incident-path highlighting, lowered-fragment inspection. (GRIND)
**WS-B.3** Sage pane wired as enrichment/escalation only (D7): renders clarifications produced by deterministic policy; hosts escalation dialogue; never gates the routine path. Board-transition flow per D20 wired against WS-C's board service (no longer a stub — WS-C builds it concurrently). (CAREFUL — the boundary is easy to get wrong here)
**WS-B.4** Edit-log persistence retaining every declaration (C7 honoured by construction); undo/redo; provenance records per V&S §11.8's closure list.

**Day-one rule (from the directive):** WS-B's disposition path calls **WS-C's deterministic disposition policy function from the first commit**, with tier-0 + Sage as the initial evidence producers. There is never a WS-B-local disposition mechanism to migrate off later.

**GATE G2 (CAREFUL close, blind review) — re-scoped:** the solicit-document workflow (V&S §6.3) authored end to end — including a reminder cycle with `max_fires`, a per-guard budget differing from the workflow default, and a correlation source — published, re-opened, every declaration intact; and a red-team script of deliberately invalid edits (backward edge, GUARD-R> inside a fork, missing MI max, budget on a non-guard, unroutable business rejection left unrouted) each refused at staging with the correct theorem or check named; **plus the full pipeline — board → tier-0 → disposition policy → I28 record — demonstrably in the loop with records written (capture-switch state per the charter)**. The v0.1 "SLM-insertion readiness" structural checks are subsumed: the board object, the single typed policy function, and the I28-shaped record are no longer promises reviewed as interface facts — they are running code exercised by this gate. Tier-1 insertion remains "register one producer + add record fields" — now demonstrated by the shadow producer registration itself rather than asserted.

### WS-C — Utterance Engine + SLM shadow (`utterance-engine`) — Phase D3 content, split by what gates it

#### C-now (ungated — build immediately, concurrent with WS-A/WS-B)

1. **C5 trace — IMMEDIATE first task: locate the matcher.** Verdict on score accessibility, scale, determinism. *Known state going in (Code-Facts Register C5 + R10): `/dev/rust/crates/ob-semantic-matcher`, Candle, CPU, cosine via pgvector, exact-match pinned 1.0 / phonetic capped 0.95; now git-versioned (`eb0b3b6`), no remote yet. The trace confirms and pins; the recall@K measurement against the corpus moves to C-gated pending GOV.1's timing call (see below).*
2. **Board construction service per §11.7:** universe → reachability filter → pre-inference policy filter (D19) → canonical ordering → content hash over the exact board (ids, schema versions, descriptions-as-supplied, ordering, reachability context, pack, policy-filter state). `NONE_OF_THE_ABOVE` on every board. Denial rendering per the ratified D19 rider — reason ("not part of your current working context"), nearest legal alternatives from the board, the governed route — never existence confirmation, never catalogue enumeration. Q29's interim rule: uncertain subject → clarify-first, boards built only against a resolved subject. Consumes WS-A.1's board-candidate schema. (CAREFUL)
3. **Stable contract + disposition policy:** `RankedCandidate{id, FiniteScore}`, `SlmResult{ranking, retrieved_subset_hash, board_hash, model_bundle_hash}`; canonical tie-break; the deterministic disposition policy (separation thresholds, missing-slot rules via the option-(a) resolvers, abstention, compound-suspected → Sage) and the I28 decision record (`disposition_policy_hash`, `context_projection_hash`). Sage escalation implements D20 against WS-B.3's flow. This function is the ONE disposition path — WS-B calls it from day one. (CAREFUL)
4. **Tier-0 matcher wiring** behind the retrieval interface, scores logged *to the record pipeline* (I28-shaped records from the first interaction).
5. **Metrics harness:** the §10.7 decomposition (board completeness; tier-0 recall@K; ranking-given-inclusion; end-to-end; abstention on oracle-absent; latency vs K), boundary FP tracking (shown/accepted/published — executed is structurally zero at this surface), hard-case suites (cross-pack collisions, rare verbs, short utterances, paraphrase families, near-identical descriptions), and the **position-invariance test**.
6. **Capture pipeline built with the switch OFF.** The full I28 closure per interaction is writable; eval/train/audit dataset separation physically enforced per the charter's shape; nothing persists beyond the session until GOV.1 ratifies.

#### C-seed (UNGATED — added by Adam's ruling 2026-07-27: LLM-as-trainer seed corpus)

The plan as drafted left tier-1 untrainable until the charter (all
training rode the 30k corpus) — meaning shadow mode had nothing
meaningful to test. Ruling: follow the Candle-phrase-population model —
**the LLM's domain knowledge is the trainer**. Synthetic seed data is
charter-independent BY CONSTRUCTION: no live capture, no 30k-corpus
use, no user/bank data — utterance→candidate pairs authored by the LLM
over the board vocabulary's own descriptions. Recorded posture (flagged
for Adam's confirm, not assumed): the charter's lineage/contamination
items apply to the seed corpus when the charter lands (it is versioned,
hashed, and provenance-marked `synthetic.llm` from day one so
retrospective application is mechanical).

- **C-seed.1**: seed corpus v1 in-repo (versioned fixture, content-
  hashed): per-candidate paraphrase families across all 28 board
  candidates + hard cases (cross-candidate collisions, short
  utterances) + off-board/NOTA examples. Format = the metrics
  harness's `LabeledCase`.
- **C-seed.2**: tier-0 baseline numbers over the seed corpus recorded
  in receipts (the recall@K baseline every gate references, seed
  edition). **RECORDED 2026-07-27** (tier0.lexical.v1 over
  synthetic.seed.v1, 92 cases): board completeness 1.0 · recall@5
  0.6125 · ranking-given-inclusion 0.14285714285714285 · end-to-end
  0.075 · abstention coverage 1.0 (exact harness output). The decomposition says precisely what §10.2 predicts:
  retrieval and abstention are serviceable, RANKING is the missing
  capability — the seed-trained cross-encoder's job, with 0.14/0.075
  as the floor it must beat.
- **C-seed.3**: seed fine-tune of the T3.4a shortlist on the synthetic
  corpus (Python train → safetensors → Candle serve behind
  `Tier0Retriever`/tier-1 contract); sealed bundle carries
  `corpus: synthetic.seed.v1` in its identity. Bake-off methodology
  unchanged — only the training data source is the seed.
- **BOUNDARY (restated, binding):** seed-trained tier-1 runs in SHADOW
  and supports engineering/testing only. G3's absolute criteria are
  measured on charter-governed REAL data — promotion evidence never
  rests on synthetic-only metrics. D17/D18 untouched.

#### C-gated (blocked on the Q9 charter — GOV.1)

- Switching live capture ON.
- Any fine-tune.
- **Tier-0 baseline *evaluation* against the existing 30k corpus:** engineering use of already-held data — scheduled here, but **explicitly flagged to Adam for a charter-timing call rather than assumed exempt** (D17 gates *training* use by its letter; evaluation-use timing is Adam's call, not this plan's assumption).

#### C-bakeoff (T3.4a as written — sequenced after the charter unblocks training)

**Base-model selection (the Q7 residual, closed by bake-off, not prior assertion).** Shortlist of 2–3 open-weight cross-encoder bases meeting all of: permissive license (Apache/MIT — non-commercial families excluded); ~100–300M parameters; **architecture loadable in Candle (BERT-family/XLM-RoBERTa known-supported; anything newer verified against Candle at selection time, not assumed)**; CPU INT-friendly. Each candidate fine-tuned identically on the corpus and measured on the C-now harness; selection = best against the absolute-criteria shape, recorded with receipts. The winner's identity becomes part of the sealed model bundle (V&S §10.8). Then: offline fine-tune (Python) with `NONE_OF_THE_ABOVE` in the training set and the charter's contamination protections; safetensors → Candle CPU inference behind the stable contract; sealed model bundle; per-pack calibration. Training is Python; serving is Candle — required at tier-0, tier-1 inference, and for the pinned-runtime reproducibility of the bundle; it is not the training stack. (GRIND with CAREFUL review of selection and training/eval methodology.)

**GATE G3 (CAREFUL close, blind review) — unchanged and unmoved:** promotion shadow → Designer suggest-only against the absolute criteria, however early the code exists. The plan defines the metrics; **the threshold values are Adam's to set at gate time** against observed baselines:

| Criterion | Threshold |
|---|---|
| Minimum evaluation sample per pack, with confidence bounds | _set at gate_ |
| Per-risk-class non-regression vs tier-0 | required |
| Max accepted false-positive rate / max published false-positive rate | _set at gate_ |
| Abstention coverage on oracle-absent boards | _set at gate_ |
| Minimum tier-0 recall@K (else fix tier-0 before promoting tier-1) | _set at gate_ |
| Maximum p95 latency (board construction → disposition) | _set at gate_ |

Suggest-only → staged-patch promotion repeats G3 on live suggest-only data. **The ladder ends there (D18).**

### T4 — In-browser oracle — Phase D4 (CONDITIONAL, unchanged)

**Entry:** C1 traced TRUE (wasm32-wasip2 build + replay-hash parity gate exist as claimed) **and** the Q23 verifier-verdict parity gate designed. Either failing parks T4 without affecting the workstreams — server-side validation from WS-A is the working mechanism regardless.

**T4.1** Ship kernel/verifier/decoder module to `designer-ui`; local validate → lower → verify → dry-run with deterministic host stubs.
**GATE G4:** parity corpus — identical verdicts and identical replay hashes, browser vs server, across the full WS-A.4 fixture corpus, wired into CI as a standing gate (the Q23 answer made permanent).

---

## C. Ordering constraints (cross-stream, binding)

1. **WS-C's C-now items depend on WS-A's board-candidate schema** (candidate identity, legality derivation — the named interface in WS-A.1). That slice is WS-A.1's first deliverable, frozen early.
2. **Shadow mode starts the day WS-B's session loop first runs against a real Pack:** from that day, every design session flows through the disposition policy with tier-0 evidence and (once trained) tier-1 in shadow, and — charter permitting — accrues corpus.
3. **G2 verifies the pipeline in the loop** (board → tier-0 → policy → record, records written, switch state per charter) alongside its authoring/red-team criteria.
4. **G3 is unmoved:** shadow → suggest-only happens only when its absolute criteria are met, however early the code exists.
5. WS-A and WS-B otherwise proceed as v0.1 sequenced them internally; T4 entry unchanged.

## Standing rules for this plan

1. **Receipts.** Each workstream close appends its receipts here (tests red→green, verdict tables, review findings and dispositions), matching the ISA-002 plan's practice.
2. **V&S amendments.** Anything a workstream surfaces that contradicts DESIGN-003 v0.6 is a HALT and a proposed versioned amendment — never adapted around. Q7 producer upgrades, Q26, Q28, Q29's revisit, and Q30's abstention-evidence shape are the expected amendment candidates.
3. **Threat-model note for WS-C.** Utterances are untrusted input to a system that constructs staged patches: the red-team suite must include prompt-shaped utterances ("ignore the board and…"), which must resolve as ordinary off-board/out-of-scope — the architecture already makes them inert; the suite proves it stays that way.
4. **Executor.** Sonnet-tier for GRIND, with CAREFUL workstream items and all blind reviews at the careful tier, per the established split. Fine-tune methodology review is CAREFUL regardless of who runs the training.

---

## E. Plan-level rulings (2026-07-27 — delegated by Adam "ok do it"; implementation-scope, no V&S clause touched)

| # | Fork | Ruling |
|---|---|---|
| E1 | Sage identity in the standalone build | **Sage is a trait** (evidence producer + escalation/clarification renderer), two impls: deterministic stub (renders policy-produced clarifications only, no free dialogue — the default, so standalone runs keyless) and a live LLM adapter (Anthropic API, config-keyed). Honest under D7: the routine path never needed Sage, so a stub default hides nothing. |
| E2 | Board-universe source | WS-C consumes the **registry interface** (the `ManifestPlaceholderRegistry` surface) behind a provider trait; T3's sealed pack becomes a drop-in provider behind the same trait when it lands. G2 does not require the sealed pack; T3 stays independently sequenced. |
| E3 | Tier-0 integration shape | **In-process embed-and-score**: board candidates embedded on the fly via the matcher's Candle embedder (CPU, L2-normalised), cosine in memory — boards are tens of candidates; the pgvector ranking path is NOT used for Designer boards (no DB round-trip, no /dev/rust schema dependency, deterministic and hashable). Palette pre-embedding is a later optimisation, not the mechanism. |
| E4 | UI stack | **Static HTML + vanilla JS (ES modules) + SVG**, served from bpmn-lite-server; renders the server-supplied DAG layout. No framework, no build toolchain. It is a window, not an editor — all mutation via endpoints; trivially Chrome-MCP-drivable. |
| E5 | Initial shadow thresholds | Thresholds live in a **versioned config struct hashed into `disposition_policy_hash`** — never inline literals. Initial values are named PLACEHOLDERs (separation margin, abstention floor, NONE_OF_THE_ABOVE-wins → abstain), low-stakes in shadow, recalibrated at G3 where the threshold values are Adam's. The ruling is the mechanism, not the numbers. |

**GOV.2 CLOSED (Adam confirmed 2026-07-27):** designer crates (`designer-graph`, `designer-ui`, `utterance-engine`) live in the **bpmn-lite workspace** as separate crates; ob-poc consumes later via git dependency with **exact rev pin** (never path-`[patch]`); extraction to an own repo deferred to the promotion gate when a second consumer exists. Rider: `/dev/rust` gets a private remote so `ob-semantic-matcher` is rev-pinnable.

**Executor split (Adam, 2026-07-27: "I will keep fable"):** Fable runs all CAREFUL items, entry traces, dispatch-brief authoring, and blind-review orchestration; Sonnet executes GRIND tasks only against a frozen upstream interface and a dispatch brief (full skeletons, verbatim invariants, HALT conditions, receipt pair named). No GRIND dispatch before its interface freezes.

## F. Receipts

### WS-A.0 entry traces + WS-C C5 trace — CLOSED 2026-07-27 (findings-only, no HALT)

**C2-residual: GREEN.** `compute_post_dominators`, `compute_region_map`,
`gateway_pairs` are `pub` + crate-root re-exported with R8 doc tags
(compiler lib.rs:24; lowering.rs:1028-1032, :1145-1149, :1330-1337); all
input/output types public and externally constructible (`IRGraph` =
petgraph `DiGraph<IRNode, IREdge>`, all-pub fields). Address-level
`compute_gateway_pairing` + `InclusiveBranchInfo` stay private BY DESIGN
— if WS-A.1 ever needs the `Addr`-level maps, that is a surfaced fork,
not a workaround. Doc-assertion converted to build-lock:
`bpmn-lite-authoring/src/oracle_boundary_tests.rs`
(`pairing_oracle_is_consumable_across_the_crate_boundary`, green) —
sibling-crate consumption of all three entry points, cement-locked.
Interface facts for the WS-A.2 brief: (i) acyclicity pre-gating is the
CALLER's responsibility; (ii) `compute_region_map`'s public contract is
**diverging-gateway → region-closing partner**, NOT node → region
membership; (iii) the public pairing name is `gateway_pairs`
(`compute_gateway_pairing` is private).

**C3: named-env-exists-but-no-runtime — claim UNSUPPORTED at HEAD; Q15
disposition recorded.** Named typed binding envs exist compile-time only
(dsl `BindingContext` HashMap<String, BindingInfo>, typed,
binding_context.rs:94-96 — zero runtime readers/writers; bpmn-lite
`PlaceholderSchema.slots` name-keyed untyped, plan.rs:238-253). NO macro
expansion path resolves named bindings to earlier-step identifiers: dsl
`MacroDefBody.expands_to` is an ordered opaque `Vec<serde_json::Value>`
with zero executors (macro_def.rs:21); bpmn-lite macro apply is caller-
param `%name%` textual substitution (macros.rs:124-160) + AstMutator
insert — no read of any binding env. The only live earlier-step→later-step
value path is POSITIONAL (`V2MiLoadElement` by array index). Order-
dependence present → **Q15 resolves toward a versioned durable named
representation** (per the WS-A.0 HALT-condition disposition). V&S §0 C3
row should flip OPEN → REFUTED-as-runtime on next V&S amendment.

**C5 (+E3 feasibility): AMBER — runtime path GREEN, build coupling
blocks.** Embedder (`/dev/rust/crates/ob-semantic-matcher`, HEAD eb0b3b6,
clean, NO remote) is DB-free at source level: Candle-only imports,
`Device::Cpu`, deterministic (no RNG/dropout; BGE-small-en-v1.5 weights
pinned to an immutable HF commit SHA; L2-normalised 384-dim; self-test
asserts same-text cosine ≈ 1.0). In-memory cosine trivial. BLOCK:
lib.rs:42-43 unconditionally compiles matcher/feedback; `sqlx`+`pgvector`
are non-optional deps → consuming the embedder drags the Postgres tree
into the designer build. REMEDY (chosen): default-on `pg` Cargo feature
gating matcher/feedback/populate_embeddings with sqlx/pgvector optional;
designer consumes `default-features = false`. Folded into the WS-C tier-0
wiring task. Note for the WS-C brief: exact-match 1.0 / phonetic 0.95
pins live in the pgvector matcher, so Designer tier-0 implements its own
exact-match pinning. GOV.2 rider CLOSED 2026-07-27:
`/dev/rust` pushed to private remote `adamtc007/ob-poc-rust` —
`ob-semantic-matcher` is now rev-pinnable.

**C4-residual — the design note (envelope ↔ instance-data mapping).**
No contradiction with ISA-002 §28 found; no HALT. The mapping:

1. **The typed invocation envelope rides the JSON planes, never `Value`.**
   `Value` has NO map/object variant (`Bool/I64/Str(interned)/Ref/Array`,
   types.rs:132-150) — a nested tagged union is not representable in
   `flags`. The envelope serialises as canonical JSON into
   `StartCommand.initial_payload` → stored verbatim as `domain_payload`
   AND (iff a JSON object; malformed = hard admission reject per R4)
   seeded into `placeholder_values` (engine.rs:789-815).
2. **Routing discriminants are top-level STRING keys.** Variant tags
   (e.g. `"delivery_kind": "client_portal"`) surface as top-level
   payload keys matched by `V2LoadPlaceholderMatch` →
   `placeholder_matches` (types.rs:1096-1111), which compares String and
   Bool ONLY (I64/arrays/objects never match — substrate rule, not a
   bug to fix silently). Designer staging validates: every declared
   routing discriminant is a string-or-bool top-level key.
3. **Variant payloads stay nested inside the JSON plane** and are read
   mid-flight via `bind_placeholder_from_payload` (absence = error,
   never null) — pointer-not-cargo intact; the envelope carries refs.
4. **Collections for MI ride `flags` as bounded `Value::Array`**
   (≤ MAX_VALUE_ARRAY_LEN=4096, depth ≤ MAX_VALUE_ARRAY_DEPTH=8,
   enforced at canonical decode + gRPC boundary + runtime backstop —
   canonical.rs:557-581, grpc.rs:170-195, kernel lib.rs:919-924).
   Per-element data is scalar/`Ref`/`Str`/nested-array by value
   (`V2MiLoadElement` clones `items[index]`); object-shaped per-element
   data must be flattened or carried as a `Ref` into the payload plane.
5. **Late-bound results enter via completion `orch_flags`**
   (`flag_<u32>` keys through the flag symbol table) — flags start
   EMPTY at spawn; there is no working start-time flag seeding (see
   finding F-DSGN-1 below).

**Finding F-DSGN-1 (surfaced, not fixed — awaiting Adam):**
`start_process` gRPC accepts and VALIDATES `req.orch_flags`
(grpc.rs:529) then silently DROPS them — `StartParams` is built without
them (grpc.rs:546-557); `StartCommand` has no flags field
(transition.rs:102-116); spawn sets `flags: BTreeMap::new()`
(engine.rs:802). The types.rs:167-174 comment describes spawn-time
seeding that does not exist. Validated-then-discarded input is a
trap-door-shaped defect under E6/fail-closed discipline. Options:
(a) wire orch_flags through StartParams→StartCommand→spawn seeding
(kernel/engine CAREFUL change; matches the comment's stated intent), or
(b) reject non-empty orch_flags at start until (a) is designed.
Recommendation: (b) now (small, fail-closed), (a) as a scheduled item —
the C4 mapping above needs neither.
**RULED (b) by Adam + IMPLEMENTED 2026-07-27:** `start_process` rejects
any non-empty `orch_flags` with `InvalidArgument` naming F-DSGN-1
(grpc.rs); stale types.rs spawn-seeding comment corrected. Receipts:
red = `start_process_rejects_any_nonempty_orch_flags` (a benign flag —
previously validated-then-discarded — now rejected); the two array-limit
start tests amended to the categorical-reject contract (strictly
stronger; completion-path limit cement unchanged); green = all existing
empty-flag lifecycle tests. Option (a) wire-through remains unscheduled
until a consumer needs spawn-time flags.

### WS-A.1 — CLOSED 2026-07-27 (CAREFUL; blind-reviewed, findings dispositioned)

Deliverables: `designer-graph` crate — frozen board-candidate interface
(19 §12.1 ops + 9 §12.2 productions, canonical ids + descriptions as
board-hash inputs, `CANDIDATE_SCHEMA_VERSION=1`, `LegalityOracle`) and
the canonical DAG schema (Q2) with the Q27 ruling: **node payload IS the
compiler's `IRNode`** — per-node declarations reach the sealed envelope
by construction; process-level declarations ride the DAG root and are
carried by `admit()` explicitly. Blind review verdict: decision SURVIVES
with three riders (per-node-scope claim narrowing; never the persistence
wire format — the edit log is, per §6.2/§12.5; NodeKey-level referential
integrity). Disposition:

| # | Severity | Finding | Disposition |
|---|---|---|---|
| F1 | BLOCKER | `admit()` dropped `default_guard_budget` (lowered with `None`); test camouflaged it | **FIXED**: `admit()` = `Compiler::lower_with_default(&ir, self.default_guard_budget)`; red→green `process_default_guard_budget_reaches_the_sealed_envelope` (Some(3) → envelope max_failures 3; None → conservative default); module-doc claim narrowed to per-node scope |
| F2 | CONCERN | `attached_to` string id lets renames dangle or silently re-point guards | **FIXED**: `DesignerNode.attached_to_key: Option<NodeKey>`; `to_ir()` projects the host's CURRENT id; non-boundary attachment refused at insert; stale-string test green |
| F3 | CONCERN | Id uniqueness promised, enforced nowhere; compiler admits duplicate ids (ambiguous budget/attachment binding) | **FIXED both halves**: insert-time duplicate node/flow-id rejection (designer) AND a new duplicate-id theorem in the production `verify()` (P8 — the oracle is the gate), cemented `duplicate_ids_are_refused_by_verify`; full workspace sweep green |
| F4 | CONCERN | `pub` mutators contradict bypass claim; `Uuid::new_v4` in `insert_node` breaks edit-log replay determinism | **FIXED**: mutators `pub(crate)` (WS-A.2 ops are the public surface, I18 structural); `NodeKey` caller-supplied — key generation belongs to the operation record (Q5), pinned in the WS-A.2 brief |
| F5 | CONCERN | `admit()` omitted `verify_bytecode` + types-crate V-1..V-11 — G1 which-theorem parity unsatisfiable | **FIXED**: `admit()` runs the exact direct-compilation chain via `Compiler::lower_with_default` (verify_or_err → lower → verify_bytecode → envelope → `from_verified_envelope`), returns the `VerifiedWorkflow` for G1 comparison |
| F6 | CONCERN/NOTEs | Description edits uncemented; `CandidateId` serde leaks variant names; no dedup | **FIXED**: blake3 golden content cement over (id, description, version) triples; hash-preimage contract documented as `(canonical_id, description, schema_version)` ONLY; `legal_candidates` dedups (test) |
| F7 | CONCERN | Raw-IRNode serde as durable format = C7's lesson repeated (landed IR field rename precedent) | **ACCEPTED as a rule**: module doc reworded to §6.2/§12.5 — the EDIT LOG is the persistence surface, the DAG a replay product; any snapshot goes through a versioned envelope. Written into the WS-A.2 brief |
| F8 | NOTE | I23 mechanism/backstop inverted (no per-op forward-only pre-gate yet) | **PINNED to WS-A.2 brief**: every edge-introducing operation pre-gates `has_path_connecting(to, from)`; verifier stays the backstop. Plus reviewer's `declared_max = 0` convention test |

**Substrate finding F-DSGN-2 (surfaced, unfixed — awaiting Adam):**
`verify_data_objects` (compiler verifier.rs) had ZERO non-test callers —
a gate that never ran. **RULED wire-in (Adam) + IMPLEMENTED 2026-07-27**:
`verify()` now runs it on every admission; cement
`verify_runs_data_object_checks` (unresolved FFI var-ref refused — red
that previously verified clean); full workspace sweep green.

### WS-C C-now items 1–3 — CLOSED 2026-07-27 (CAREFUL; blind-reviewed NOT-CLEAN → remediated)

Deliverables: `utterance-engine` crate — stable contract (`FiniteScore`
typed rejects, `SlmResult`, canonical tie-break, `NONE_OF_THE_ABOVE`),
§11.7 board construction, deterministic disposition policy + I28 record.
Blind review returned 2 BLOCKERS + 7 CONCERNS + 5 NOTES; disposition:

| # | Finding | Disposition |
|---|---|---|
| B1 | Board-hash preimage non-injective (anchor `"<root>"` sentinel collision; delimiter forgery via provider strings) | **FIXED**: length-prefixed domain-tagged preimage (`tag:len:bytes`), distinct tags for None/Some; red fixtures — sentinel collision + crafted delimiter pair now hash differently |
| B2 | Close-scope deviation: close-gap → `Ambiguous` CONTRADICTED §10.3's ruling (score topology cannot distinguish ambiguity from compound); `MissingArguments`/`Compound` absent vs I21 | **FIXED to the ruled reading**: insufficient separation → `EscalateToSage` (never a masking A-or-B render); enum carries the full I21 shape with `Ambiguous`/`MissingArguments`/`Compound` declared UNREACHABLE-in-v1 (reachable only with certified producers — policy version bump + plan amendment, not a threshold tweak). D20 escalation SHAPE (board ref, context-change channel) lands with WS-B.3's flow — recorded here as WS-B scope |
| C3 | Abstain description uncemented board-hash input | **FIXED**: folded into the designer-graph golden (hex bumped deliberately) |
| C4 | policy_hash rested on serde_json float text | **FIXED**: hand-built preimage (`f64::to_bits`), golden hex cement for shadow_v1 |
| C5 | decide trusted producer order; duplicates admitted | **FIXED**: policy re-sorts via `rank_canonically` (I28 tie-break policy-owned); duplicate ids refused; misorder receipt green |
| C6 | build_board fail-open on provider misbehavior | **FIXED**: `-> Result`; reserved `abstain.*` namespace refused; same-id/different-content collision refused; identical dupes still collapse |
| C7 | Reachability context lacked artifact identity | **PARTIALLY FIXED + WS-B obligation**: `BoardContext.graph_identity` added and hashed (None hashed distinctly); WS-B MUST supply the session revision/graph hash when building boards — brief item |
| C8 | I27 documentation-not-mechanism (pub fields allow forging records/boards) | **DEFERRED with note**: Repl recheck is the ratified gate (§11.7 "the pre-filter is hygiene, never the gate"); Board private-fields hardening rides WS-C item 4 wiring |
| C9 | anchor/anchor_id decoupled | **FIXED**: single `Option<(&NodeKey, &str)>` parameter |
| N1 | Empty ranking laundered as escalation | **FIXED strict**: producer malfunction, typed error |
| N2 | Ambiguous top-2 truncation | Moot in v1 (unreachable); revisit at producer certification |
| N3 | Projection hash unversioned | **FIXED**: `ctxproj.v1:` domain tag |
| N4 | policy_version honor-system | **FIXED**: golden decision table + golden policy hash tied to version 1 |
| N5 | Record ranking as raw f64 | **FIXED**: `FiniteScore` in `DecisionRecord` |

Config-by-hash registry (N3 rider) is a named WS-C item-6 (capture
pipeline) obligation: records are reproducible only if configs are
retrievable by `disposition_policy_hash`.

### WS-C C-now items 4–6 — CLOSED 2026-07-27

- **Item 4 (tier-0):** `ob-semantic-matcher` `pg` feature-gate landed
  and pushed (`ob-poc-rust @ ff3f12c7` — C5 AMBER→GREEN; Candle slice
  builds with no Postgres tree). `Tier0Retriever` trait is the producer
  seam; `LexicalTier0` (the demoted keyword gate's ruled successor:
  deterministic token overlap, designer-side exact-match 1.0 pin, NOTA
  as overlap complement) and `EmbedTier0` (E3: rev-pinned Candle
  embedder, on-the-fly board embedding, in-memory cosine; behind an
  off-by-default `embed` feature so default builds stay network-free;
  integration receipt `#[ignore]`d for cold-cache weight download).
  **Pipeline-in-loop receipt green**: board → tier-0 → policy → I28
  record end to end, gibberish abstains, deterministic (G2 criterion,
  first light).
- **Item 6 (capture, switch OFF):** `CapturePipeline::off()` sole
  zero-arg constructor; ON requires a ratified Q9 charter reference
  (D17 as mechanism); suppression visible, never silent; physical
  Evaluation/Training/Audit sink separation. `ConfigRegistry` closes
  the N3 rider (policy-hash → config, hash derived never supplied).
- **Item 5 (metrics):** the §10.7 per-tier decomposition
  (completeness / recall@K / ranking-given-inclusion / end-to-end /
  abstention coverage) with zero-denominator honesty, plus
  `assert_position_invariant` reusable against any producer.
- utterance-engine suite 23/23 (+1 ignored embed integration);
  workspace clean. **Next: WS-B day-one wiring** (session utterance
  endpoint → `decide()` with `LexicalTier0`) = formal shadow start per
  §C constraint 2; WS-B must supply `graph_identity` (C7 obligation).

### WS-A.2 slices 1–5 + WS-B UI — receipts (2026-07-27)

Five Sonnet GRIND dispatches, each against a committed proscriptive
brief, each reviewed first-hand before commit (the executor-split loop
proven): **16 operations** (linear 5, guard/declaration 5, region 4,
ReplaceNode, CreateBranch) and **6 of 9 §12.2 productions** as pure
`bindings → Vec<Operation>` compositions with atomic-abort application
and serde round-trip (Q5 edit-log entries). Binding rule minted by the
slice-4 remediation (my brief mis-specified `reminder_then_escalate`;
the executor flagged the admit-gap honestly): **a production ALONE must
admit — it owns its complete shape including guard escape flows.**
Illegal states unrepresentable by typing where possible
(cycle-on-interrupting unconstructible through
`InterruptingTimeoutBindings`; MI max mandatory; budget-on-non-guard
absent from the vocabulary). designer-graph 49/49.

Excluded pending CAREFUL substrate traces (never faked):
`CreateRace`/`timer_message_race` (no race IRNode),
`CallSubprocess`/`call_durable_subprocess` (no call-activity IRNode),
`AttachRollbackGuard` (no GUARD-R IR path),
`human_review_with_rework` (XOR default-edge semantics untraced).
`CloseParallelRegion` recorded unrepresentable-by-design (regions
constructed closed).

**WS-B.1/B.2 landed:** `/designer` static window (ruling E4) — session
list, REPL pane showing disposition + board-hash + D17 capture state
per turn, source/diagnostics pane, save-as-template, SVG graph window
over the server-built DAG + layout endpoint. UI smoke receipt green.
Shadow pipeline live at the session utterance endpoint since `d4e2406`.

Open to G2: solicit-document end-to-end authoring receipt; red-team
script; the four traces above; WS-B.4 edit-log persistence
formalization; blind review of the WS-B surface before the gate.

#### Receipts — four CAREFUL substrate traces (2026-07-27, findings-only, four independent read-only agents)

**Race / `timer_message_race` — NOT-REPRESENTABLE (frontend); kernel COMPLETE.** The ISA/kernel race primitive is fully built and loser-cancelling: `V2RaceOpen/V2ArmTimer/V2ArmMsg/V2RaceClose` (types.rs:575–611), `WaitState::V2Race`, winner resolution emitting `TimerMutation::V2CancelRace` (kernel lib.rs:3187, 4307–4320), V-5 contiguous-arm verifier rules. Three independent frontend breaks: no `IRNode` variant; parser hard-rejects `eventBasedGateway` (parser.rs:557–564); lowering never emits the race opcodes. Guard composition cannot substitute: boundary guards wrap task hosts only. Work to open it: parser + IR variant + lowering + frontend-verifier acceptance — zero kernel work. `CreateRace`/`TimerMessageRace` exclusions stand.

**CallSubprocess / `call_durable_subprocess` — NOT-REPRESENTABLE (IR/durable); authoring checks EXIST; spawn half ABSENT.** DSL hash-plug tasks already carry call-activity verification (child existence, blocking-deadlock, recursion — closure.rs:98–140) but lowering collapses every task to `Instr::ExecDslTask` (frontend.rs:86–100, `delivery_mode` dropped) — an external job, not a child workflow. The durable-invocation substrate is built but producer-less: `ProcessState::WaitingOnSubmission/WaitingOnInvocation`, `Command::StartChildResult`, `ChildStart` (zero call sites), `TickOperation::StartChild`, migration 033 (caller-side callout registry — no parent/child instance columns). Work: IR node, lowering arm, a kernel word that EMITS WaitingOnSubmission + child spawn, store producer, schema linkage. Exclusion stands.

**AttachRollbackGuard / GUARD-R — kernel EXISTS, frontend-inaccessible; compensation ABSENT by design.** `V2GuardR/V2GuardREnd/V2CancelScope` + A3 five-field snapshot restore + V-10 rules are complete and test-covered (kernel lib.rs:2122–2151, 3900–3991, 4045; v2_verifier.rs:531, 852). No IRNode, no parser keyword, no lowering emission. True saga-compensation (reverse-order handlers over COMPLETED work) is deliberately uninhabited (`RecordKind::Compensation`, concurrency.rs:65–80) — v3 scope. Exclusion stands; opening data-rollback = frontend-only work.

**XOR default-edge — default is MANDATORY; `human_review_with_rework` UNBLOCKED.** Verifier §6 requires EXACTLY ONE condition-less edge on every multi-out XOR (verifier.rs:304–330); lowering emits conditioned `BrIf` chains in edge order with the default as trailing `Jump` — zero-match deterministically takes the default, no incident on the XML path. Conditions are boolean Eq/Neq only at the XML frontend. Backward rework edges are REFUSED twice (IR cyclicity, verifier.rs:115–123; bytecode backward-jump, verifier.rs:847–871) — rework is forward-only or bounded (MI / cycle guard). Production shape ruled representable: HumanWait → XOR with conditioned approve arm + default reject/rework arm routing forward. Remains in the production queue.

#### Receipts — G2 (partial) + F-DSGN-3 fail-open closure (2026-07-27, `designer-graph/src/g2_receipts.rs`, 6 tests)

**GREEN:** §6.3 solicit-document chain (create → resolve → send → correlated MessageWait → register → HumanWait review → End) authored ENTIRELY through the edit log (ops + `request_and_wait`), admits through the full direct-compilation chain; declarations survive (default budget 3 → envelope; both correlation sources → projection); the whole edit log serde round-trips and replays bit-identically (Q5). Guard declarations proven on a SUPPORTED task host: GUARD-N> + GUARD-TIMER-CYCLE>{max_fires:3} opcodes in the envelope, budget override Some(2) in projection. **RED (red-team script, each refusal naming its theorem+elements):** duplicate BPMN id; I23 backward connect; delete/replace of a guarded host; cycle trigger on interrupting guard; undeclared correlation source rejected at admission naming the missing producer; I18 backstop green.

**F-DSGN-3 (fail-open, FIXED red→green):** verifier §7a listed HumanWait as a legal BoundaryTimer host but lowering's HumanWait arm never consults `boundary_lookup` — a verified guard-on-human-wait compiled with the guard SILENTLY DROPPED (proven: admitted envelope contained zero guard opcodes; escalation chain orphaned). Fix: HumanWait removed from §7a's host set — reject, don't skip. Cement: `g2_boundary_timer_on_human_wait_rejected_not_dropped`. Full workspace green.

**FORK SURFACED — §6.3 "guard the wait" is unrepresentable (G2 blocked-in-part, awaiting Adam).** The reminder cycle on the document wait: guard on MessageWait rejects at admission (fail-closed receipt `g2_fork_receipt_…`); guard on HumanWait now also rejects (F-DSGN-3). Guards lower on task hosts only. Options: (a) extend lowering to wrap wait hosts (HumanWait first, MessageWait with it) in the guard scope — enables §6.3's literal shape; kernel scope/arming mechanics appear ready but need CAREFUL receipts; (b) amend §6.3's temporal impl to hang the reminder elsewhere. **Recommendation: (a)**, as a CAREFUL tranche with kernel park/fire receipts. Until ruled, G2's end-to-end receipt stands minus the guarded wait.

#### Receipts — DIR-002 pre-B dependencies (2026-07-27)

**A1 serializer (`utterance-engine/src/context.rs`, commit a70beaa):** ctxproj.v1 canonical line grammar; golden bytes + hash pinned (`07290be2…f804`); injectivity by typed construction rejects; `decide()` takes `&ContextProjection`, hash DERIVED never supplied; the utterance-placeholder hash is deleted. I28 widening: `Utterance` events store the SERIALIZED projection (trainable, not hash-only, additive+defaulted); endpoint receipt proves stored bytes re-hash to `context_projection_hash`.

**Positional legality oracle (`designer-graph/src/positional.rs`):** real §11.7 position-dependent boards over a `DesignerDag`. Two-layer rule table (staging-enforced mirrored + consistency-cemented against `apply`; admission-enforced mirrored from verifier theorems — F-DSGN-3 alignment: guard attachment proposed at task hosts ONLY, never at waits). Absolute exclusions cemented: CreateRace/CloseParallelRegion/AttachRollbackGuard/CallSubprocess + TimerMessageRace/CallDurableSubprocess/HumanReviewWithRework never boarded (the interim WholeGraphLegality boards the full catalogue including unbuildables — superseded for corpus work; endpoint swap rides WS-B wiring). Position-sensitivity receipts: same op present at one anchor, absent at another (A2.2's mechanism). Whole-graph = deterministic union; empty graph = NOTA-only board.

**T3.4a shortlist (research receipt, candle support verified from candle-transformers source; loadability receipts still owed at Phase C per "verified not assumed"):** recommended four — `Alibaba-NLP/gte-reranker-modernbert-base` (149M, Apache-2.0, `models::modernbert` incl. SequenceClassification head, already a reranker); `cross-encoder/ms-marco-MiniLM-L6-v2` (22.7M, Apache-2.0/MIT base, `models::bert` + small head, only candidate comfortably sub-second CPU); `answerdotai/ModernBERT-base` (149M, Apache-2.0, clean-room fallback); `BAAI/bge-reranker-base` (278M, MIT, `models::xlm_roberta` incl. head — latency is its gate). Excluded: bge-reranker-v2-m3 (568M), mxbai v2 (Qwen), gte-multilingual (remote code), DeBERTa-v3 (candle head coverage + tokenizer conversion UNCERTAIN — flagged, not guessed). Latency caveat recorded: 149M tier needs batching/quantization receipts.

#### Receipts — DIR-002 Phase A: spec + blind review (2026-07-27)

EOP-SPEC-SLM-TRAIN-001 v0.1 authored; independent authorship-blind review returned 17 findings (3 BLOCKER / 11 CONCERN / 3 NIT — full disposition table in the spec §S8). Blockers, remediated same-session: (1) uncheckable loader board-hash check → S2 now stores the full §11.7 preimage; (2) **A1 skew one layer up — no shared DAG→projection CONSTRUCTOR existed** → `project_ir` + `ir_kind_str` (the one kind vocabulary) landed in `utterance-engine/src/context.rs`, `DesignerDag::seed` (fail-closed Start/DataObject-only public seeding) in designer-graph, golden cross-crate cement `project_ir_golden_from_designer_ops`; interim DSL-plan endpoint projections marked NON-training-grade — **substrate ask: WS-B DesignerDag-backed sessions are the convergence point**; (3) split rule could sever context-pair sides → split unit redefined as the connected component over family/pair-group/utterance-text. **OPEN RULING FOR ADAM (finding 5): listwise training lists — DIR-002 A4 says "over the board", the ratified §10.3/§10.6 inference contract scores the tier-0 RETRIEVED SUBSET; recommendation = train on the real retriever's K-subset + NOTA always appended. Phase B label generation holds until ruled.**

#### Receipts — DIR-002 Phase B start: ruling implemented, generator live (2026-07-27)

Finding-5 ruling (Adam): lists = real tier-0 K-subset + NOTA → `retrieval::tier1_list` (one function, generator + Phase-C serving). Corpus generator landed (`utterance-engine/examples/corpus_gen.rs`): enumerates board states via seed→ops→PositionalLegality→build_board, consumes authored banks (`seed/banks/*.json`), enforces mechanically: label-on-board (HALT otherwise), 0.5 Jaccard cap incl. the NOTA rule, normalized dedup, retrieval-miss drops counted, pair-group integrity (structural violations HALT; hygiene-broken pairs drop BOTH sides, counted). `synthetic-v2-alpha` emitted from a 43-entry starter bank: 38 examples (10 NOTA), drops: 2 overlap-cap (the cap catching description-like authoring — working as specified) / 1 retrieval-miss / 1 pair-break.

**Empirical finding (headline): the context pair "chase them again" (guard-anchor → set_guard_trigger vs task-anchor → reminder_then_escalate) was retrieval-missed under LexicalTier0 — zero token overlap with the gold description is the DEFINING property of high-value context pairs, so the lexical retriever structurally excludes exactly the examples tier-1 exists to learn.** Consequence: synthetic-v2 full generation runs on the embed tier-0 (E3, `--features embed`) as the retriever; lexical stays as the regression baseline. Recorded in spec §S5.

#### Receipt — embed tier-0 closes the retrieval gap (2026-07-27, corpus_v2-alpha regenerated)

Generator wired to `EmbedTier0` under `--features embed` (BGE-small-en-v1.5, SHA-pinned, integration test green). Same 43-entry bank, embed retriever: **40 examples, 2 paired (the "chase them again" context pair SURVIVES on both boards), 0 retrieval-miss** — vs lexical: 38 examples, 0 paired, pair lost to retrieval-miss. The red→green pair of corpus cards is the receipt that full synthetic-v2 generation runs on the embed retriever; retriever identity is recorded in each card, and `tier1_list` keeps training-list = serving-list on whichever retriever serves.

#### Receipts — guarded-wait ruling landed; synthetic-v2-beta emitted (2026-07-27)

**Guarded-wait ruling (Adam) closes the G2 fork.** Kernel trace (independent read-only agent, before implementation) found items 1–4 of the guard-on-parked-wait question WORK TODAY with zero kernel change: fire-while-parked is record-keyed and ignores fiber `WaitState` (`lib.rs:4360,4371`); interrupting cancellation walks `control_stack` to find the Msg-parked host, and fiber deletion IS the only message-registration cleanup — no side table to leak (`lib.rs:3777-3809,3844`); post-close late fires are staleness no-ops via `RecordState::Armed` gating (`lib.rs:4371-4384`); the v2 verifier already admits `V2WaitMsg` inside a guard scope (no anti-non-task rule exists). Item 5 (sizing) was the only real gap — compiler-only.

Implemented: `lowering.rs`'s `lower_boundary_guarded_task_v2` generalized behind a `GuardedBody{Native,WaitMsg}` enum (returns the body `Addr` so the caller registers the correlation source at the right instruction); `MessageWait`/`HumanWait` lowering arms route through it instead of the old flat 2-instruction body; `instr_count_for`'s guarded-host sizing arm extended to the two wait kinds (same `+extra` formula, base 2 instead of `ExecNative`+`Jump`); verifier §7a re-admits `MessageWait`/`HumanWait` as legal `BoundaryTimer` hosts (superseding the F-DSGN-3 fail-closed rejection — this is the fix the fork was blocked on, not a reopening of the fail-open). `PositionalLegality`'s `is_guard_host` now includes wait nodes (SendTask excluded — never a lowerable host). G2 receipts flipped red→green: `g2_solicit_guarded_wait_admits_and_arms` proves `V2WaitMsg` sits INSIDE the guard scope with `GUARD-N>`/`GUARD-TIMER-CYCLE>{max_fires:3}` present in the sealed envelope; `g2_guard_on_human_wait_admits_and_arms` proves the same on the HumanWait review step. **§6.3's literal "guard the wait" shape now admits end-to-end — G2 is CLOSEABLE pending only the WS-B blind-review half.** Full workspace sweep: 110/110 green.

Bank sweep for the new legality: 10 stale "waits can't host guards" NOTA entries removed across banks (labels-by-construction — dropped, never judgment-relabelled); one false-positive drop ("race the reply against a deadline" — races remain off-board regardless of the wait-guard ruling) caught and restored. `BRIEF-BANK-AUTHORING.md` updated with the corrected per-class legal-label table.

**synthetic-v2-beta emitted** (regenerated under the new legality + embed retriever, corrected corpus-version naming): 567 examples from 615 validated bank entries (five authoring-agent banks + alpha, ~666 authored total), zero label-legality halts — every bank entry's label was legal on its board, including the newly-legal wait-guard labels, confirming the oracle/lowering/verifier triangle agrees end to end. Drops: 2 overlap-cap, 2 duplicate, 39 retrieval-miss (6.8% — expected under the ruled K=8 embed-subset rule; visible in the card, not hidden), 5 pair-break (a pair whose one side missed retrieval loses both sides per spec S2). 83 NOTA (14.6%), 40 paired examples / 20 surviving pairs (7.1%) — both above spec S3 floors' proportional targets on the surviving base. Per-regime and per-label breakdowns in the card. Held-out eval slice emitted separately (`synthetic-v2-beta.eval.jsonl`, 100 disjoint-persona entries, never trained) plus the 37-item `eval_ambiguity_v1.json` (never trained, A2.5). **Total-count floor (≥5,000) explicitly NOT met — card says so plainly; this is an authoring-progress receipt, not a release corpus.**

**Performance finding (not yet actioned, flagged for Phase C):** `EmbedTier0::retrieve` re-embeds every board's full candidate set on every call — no description-embedding cache. Regenerating ~600 entries took >10 minutes of CPU even with model weights warm-cached from a prior run. This will not clear the Phase-D latency gate as built; caching per-`board_hash` candidate embeddings is the fix, sized for the S3-floor-scale (≥5,000 example) generation run and for symmetry with the eventual serving path.

#### Receipt — EmbedTier0 target-embedding cache (2026-07-27, closes the flagged performance finding)

`retrieve()` re-embedded every board candidate on every call; a corpus run reuses each enumeration-class board's small candidate set across hundreds of utterances, so this was O(entries × board_size) forward passes for what is really O(distinct descriptions). Fixed: `EmbedTier0` gained a `Mutex<HashMap<description, embedding>>` cache — exact, not approximate (`embed_target` is a pure function of its text, matcher contract), keyed by description so a future collision-in-id-but-not-text case stays correct by construction. Receipt (`embed_target_cache_is_exact_and_faster_on_repeat`, run against real pinned weights): a warm-cache retrieve is measurably faster than the cold one, AND re-running the cold utterance after the cache warms yields bit-identical `retrieved_subset_hash` and per-candidate scores (`to_bits()` equality) — caching never perturbs a score. **Measured end-to-end: full synthetic-v2-beta regeneration (567 examples + 98 held-out eval) dropped from >10 minutes to 4:19.** Still not fast enough alone for the 5,000-example S3-floor run at proportional scale — batching `embed_target` calls (the matcher crate already exposes a batch API) is the next lever, deferred until bank authoring actually scales that far.

#### Receipts — WS-B.4: DesignerDag-backed sessions (2026-07-27)

Architecture question (how sessions become DesignerDag-backed) resolved via a
dedicated read-only research agent before implementation, per verify-don't-infer:
exhaustive grep confirmed zero DSL-source↔DesignerDag round-trip code exists
anywhere, live or dead; `schema.rs`'s own module doc explicitly forbids raw DAG
snapshot persistence ("Any DAG snapshot persistence goes through a versioned
envelope, never raw serde of these types") and designs for an edit-log-backed
model instead; `g2_receipts.rs`'s `g2_solicit_edit_log_round_trips` already
proves the serde round-trip + replay pattern. This made the opaque edit-log
event kind the only architecturally-consistent option, not merely the
cheapest one.

Landed: `DesignSessionEventKind::GraphEdit { operations_json, note }` in
`bpmn-lite-store` — stored as an opaque `String`, deserialized only
server-side (`bpmn-lite-store` gains no new `designer-graph` dependency,
mirroring the existing `Utterance`/`decision_record_json` precedent).
`DesignerDag::key_for_bpmn_id` added to `designer-graph` (fail-closed
`Option<NodeKey>` — an unknown anchor id is a typed `None`, never a silent
whole-graph downgrade). `bpmn-lite-server/src/rest.rs`: deterministic
Start-node seeding (`seed_start_key` via `Uuid::new_v5`, fixed namespace,
so every replay reconstructs the identical key without persisting anything
extra); `reconstruct_designer_dag` replays a session's accumulated
`GraphEdit` payloads through `apply_production`; new
`POST /api/dsl/sessions/:id/graph-edit` endpoint stages against the current
reconstruction and persists only on admission (I18 clone-and-stage extended
to the session layer — a refusal persists nothing); `session_utterance_endpoint`
now branches on `is_graph_backed()`: graph-backed sessions resolve the
request's `anchor` via `key_for_bpmn_id` (422 on unresolvable), compute
`graph_identity` by hashing the accumulated edit-log payloads (the edit log
IS the DAG's sole source of truth — not a Debug-formatted IR string, rejected
mid-implementation as non-canonical), and drive the board/context through the
real `PositionalLegality` oracle and `project_ir` — legacy DSL-source sessions
are byte-for-byte unchanged (WholeGraphLegality + census-only fallback).

Receipts (`cargo test -p bpmn-lite-server --lib`, 3 new, all green):
`test_session_graph_edit_admits_and_persists` (valid two-op sequence stages/
admits/persists; empty-operations POST → 400); `test_session_graph_edit_refuses_invalid_ops_and_persists_nothing`
(unknown-anchor `AppendNode` → 422, zero `GraphEdit` events persisted —
the RED half); `test_session_utterance_uses_positional_legality_when_graph_backed`
(headline receipt: builds a 2-node chain via graph-edit, POSTs an utterance
against a ghost anchor → 422 fail-closed, then against the real anchor → 200,
then confirms the persisted `context_projection` text was produced by
`project_ir`, not the census fallback). Full workspace sweep:
`cargo test --workspace` — 0 failures across every crate.

This is the convergence point flagged by the SLM-training blind review
(finding 2, DIR-002 Phase A remediation): interim DSL-plan endpoint
projections were marked non-training-grade pending exactly this substrate.
Graph-backed session utterances now produce real `project_ir` context —
future corpus generation can ingest real session data, not synthetic-only.
Also closes the WS-B.4 half of G2's remaining open item (WS-B blind review
is the other half, still open).

#### Receipts — WS-B independent blind review + remediation (2026-07-27) — G2 NOT YET CLOSED

Independent read-only agent dispatched against the full WS-B surface
(`rest.rs`'s session lifecycle/graph-edit/utterance/save endpoints,
`store.rs`'s `DesignSessionEventKind`, `schema.rs`'s `DesignerDag`/
`key_for_bpmn_id`), instructed to assume at least one real bug exists and
not run the test suite (independence from what the suite already asserts).
Found 3 BLOCKERs; each re-derived against primary source before acceptance
(verify-don't-infer), not accepted on the sub-agent's paraphrase alone.

**BLOCKER 1 (FIXED, red→green): `session_graph_edit_endpoint` never called
`.admit()`.** `productions.rs:289-290`'s own contract states `apply_production`
"does NOT run admission itself... the caller stages then calls
`candidate.admit()`" — the endpoint discarded the staged candidate
(`let _ = staged;`) and persisted the op sequence regardless. `apply_production`
only proves per-op local anchor legality; the full `to_ir`→`verify`→
`Compiler::lower_with_default` theorem chain (fork/join matching, SESE
nesting, reachability) never ran. **Fix:** `staged.candidate.admit()` now
gates persistence; refusal returns 422 with the verifier's diagnostics,
nothing is appended. **Receipt:** new test
`test_session_graph_edit_refuses_locally_staged_but_globally_illegal_graph`
(a `GatewayAnd` split with no matching join — each `AppendNode` individually
legal, the resulting graph globally illegal) — confirmed genuinely RED
pre-fix (200 OK, wrongly accepted) by mechanically disabling the `admit()`
call and re-running, then GREEN restored.

**BLOCKER 3 (FIXED): TOCTOU race in `session_graph_edit_endpoint`.**
Neither backend (`MemoryStore::append_design_session_event`,
`store_postgres.rs`'s equivalent) serializes a session's load→reconstruct→
stage→append sequence — only the append's own storage write was atomic.
Two concurrent graph-edits against the same session could each load the
same base, each stage successfully, both persist — the second replays
against a DAG shape it was never validated against, permanently bricking
the session (every future reconstruct/utterance call errors, no repair
path). **Fix:** `DemoState::session_lock` — a per-session `tokio::sync::Mutex`
serializing the endpoint's full load-stage-append critical section; different
sessions never contend. **Receipt:**
`test_concurrent_graph_edits_on_same_anchor_never_corrupt_session` — 5
concurrent requests competing for one anchor's single outgoing-edge slot;
confirmed exactly 1 succeeds, 4 cleanly refused, and the session remains
reconstructible afterward. **Honesty note (no trap doors — don't overclaim
a receipt):** this test could NOT be made to fail without the lock in this
harness (`#[tokio::test]`'s current-thread runtime never interleaves the
critical section without an explicit forced yield point) — three runs
without the lock all passed by accident, not by correctness. The fix is
retained on the strength of the static TOCTOU analysis (both backends read
independently before any serialized write) and is a standard, obviously-
correct primitive; the test is a real regression guard for the post-fix
invariant, but is NOT a proven red→green pair the way BLOCKER 1's is. Flagged
here rather than silently presented as equally strong evidence.

**BLOCKER 2 (OPEN — genuine fork, not decided): `save_design_session_endpoint`
ignores `is_graph_backed()`.** `store.rs`'s `current_source()` returns `None`
for a graph-backed session unless a `Revision` event exists in its log; the
save endpoint calls it unconditionally. A pure graph-authored session gets a
400 (save-as-template simply broken); a session seeded with initial DSL text
then edited via graph ops silently saves that STALE, unrelated text as the
template — a quiet-wrong-success. Not a mechanical fix: `DesignerDag::admit()`
produces an `ExecutableWorkflow` (post-`to_ir`/`verify`/`lower_with_default`,
already-lowered) while `dsl::compile()` produces a `WorkflowExecutionPlan`
(pre-lowering AST-level plan) — different types, and `load_plan`'s stored
JSON is a LIVE contract: `bpmn-lite-bus-handler/src/lib.rs:715-717`
deserializes it specifically as `WorkflowExecutionPlan` before spawning a
process. My first-pass recommendation (store `ExecutableWorkflow` directly)
was WRONG — verified against this real consumer before touching any code —
and would have silently corrupted template instantiation for any
graph-authored template. Correcting the record rather than proceeding on a
disproven recommendation. **This blocks G2's close** until ruled: (a) build
a `DesignerDag`→`WorkflowExecutionPlan` projection so both session kinds
converge on the one stored/consumed type, or (b) widen `load_plan`'s
contract to a tagged union of both plan shapes with the bus-handler
consumer updated to handle both. Recommend (a) — the bus-handler's
`spawn_process_with_idempotency` and the AST-level plan machinery it
depends on are unlikely to want two competing input shapes — but this
changes a live wire contract, so surfacing rather than deciding.

Full workspace sweep after all fixes: `cargo test --workspace` — 0 failures.
**G2 remains OPEN pending Adam's ruling on BLOCKER 2.**

Also worth a line: the executor's course-correction on BLOCKER 2 — tracing
`load_plan`'s real consumer (`bpmn-lite-bus-handler`) before shipping the
first-pass recommendation, catching that storing `ExecutableWorkflow`
directly would have silently corrupted live template instantiation — is the
trace-before-trust discipline (verify-don't-infer) doing its job. Recorded
as a caught-by-process event, per Adam's ruling below.

#### Ruling — BLOCKER 2: (a), with two riders (Adam, 2026-07-27)

**(a)** — `DesignerDag`→`WorkflowExecutionPlan` projection, both session
kinds converge on the one stored/consumed type — **ratified, with two
riders that determine whether (a) is right or a trap:**

- **Rider 1 — the projection is a call, not a construction.** The
  `IRGraph → WorkflowExecutionPlan` step must be the production compiler's
  own validate-and-lower path, invoked at save/publish — never a bespoke
  mapping function. A hand-rolled projector would be a second lowering path
  (the three-faces-of-one-root-cause lesson, §23) and a direct I16/I17
  violation. If the existing compiler surface can't be called from the
  endpoint's position, that's a HALT and a substrate ask, not a
  reimplementation.
- **Rider 2 — converging the plan store must not lose the authoring
  truth.** `load_plan`'s ONE stored type is correct because that store
  feeds instantiation and should only ever contain compiled artifacts (P5).
  But the `DesignerDag` is the authoritative authored artifact (I1) and
  must persist in the session/edit-log store with its declarations (C7)
  intact so a graph-backed session can be reopened and edited. (a) as
  sketched saves only the projection — fixing the bus-handler while
  silently breaking G2's reopen-and-round-trip requirement is not a fix.
  **Two stores, two roles: session store holds the DAG (already true —
  `GraphEdit` events), plan store holds what the compiler produced from it.**

**Why not (b)** (widen `load_plan` to a tagged union): puts an authoring
representation into the execution consumer's contract — the bus-handler
would be interpreting/lowering a source graph at instantiation time,
exactly what P5 forbids, and spreads variant-handling into a shipped
consumer where a future wildcard arm is one lazy edit away. The
instantiation boundary receives compiled artifacts, full stop.

**Receipts required (both, not one):** red→green fixture that (i) saves a
graph-backed session, publishes, and instantiates end-to-end through
`bpmn-lite-bus-handler`; (ii) reopens the same session for edit with the
DAG and every declaration intact. The bug was silent precisely because
only one side of the boundary was ever exercised.

#### Trace result — Rider 1's condition IS triggered: HALT, substrate ask (2026-07-27)

Dispatched a read-only research agent to determine whether an
`IRGraph → WorkflowExecutionPlan` path already exists anywhere in the
compiler, at any crate position (the question Rider 1's fallback anticipates).
Finding, stronger than "not callable from this position" — **it does not
exist in any form:**

- `dsl::compile()`'s `lint()` phase (`linter.rs:261-288`) is what
  constructs `WorkflowExecutionPlan`; it operates purely on the parsed AST
  (`WorkflowSource`/`NodeAst`) and never builds or references an `IRGraph`
  — zero references to `ir.rs` types anywhere in `dsl/`.
- The IR path (`lowering.rs`'s `lower`/`lower_with_default`/`lower_v2`)
  goes `IRGraph → CompiledProgram/VerifiedWorkflow` — bytecode-level,
  never touches `WorkflowExecutionPlan`.
- A third, independent route (`dsl/frontend.rs:44-49`, `lower_plan`) goes
  `WorkflowExecutionPlan → VerifiedWorkflow` directly, bypassing `IRGraph`
  entirely.

Not a crate-boundary/visibility problem — `designer-graph` already depends
on `bpmn-lite-compiler` and already calls into it (`to_ir`/`verify`/
`Compiler::lower_with_default`); wiring a new call would be trivial. The
function has simply never been written. `WorkflowExecutionPlan`'s
`ExecutionNode` also carries placeholder-schema/delivery-mode fields
(`plug`, `static_args`, `produces_placeholder`) `IRNode` has no equivalent
for — not a mechanical reshuffle, but new compiler logic: how does
IR-authored graph state resolve to placeholder bindings the AST path
currently derives from source annotations?

Per Rider 1 and the working contract's HALT discipline: not building this
under implement-mode. **G2 stays OPEN, unchanged.** Needs Adam's scoping
of the substrate tranche (a new `IRGraph → WorkflowExecutionPlan` lowering
inside `bpmn-lite-compiler`, presumably CAREFUL-tier given it touches the
compiler's ratified surface) before further BLOCKER 2 work proceeds.

#### Receipts — BLOCKER 2 substrate tranche implemented (Adam: "careful scope and implement", 2026-07-27)

**Scope, per Rider 1 ("a call, not a construction").** New module
`bpmn-lite-compiler/src/dsl/ir_plan.rs`, `project_ir(ir: &IRGraph,
workflow_id: String) -> Result<WorkflowExecutionPlan, IrPlanError>` — the
production compiler's own path, extended to IR input, not a bespoke
mapping outside it. Deliberately conservative: supports `Start`, `End`,
`ServiceTask`, and `GatewayAnd`/`GatewayInclusive` matched
diverging/converging pairs (via the already-exposed `gateway_pairs`
pairing oracle — never hand-rolled repairing; this is the exact function
R8/C2 exposed at the compiler boundary for precisely this kind of reuse).
`DataObject` nodes (structural-only, zero bytecode) are omitted. Every
other IRNode kind — `GatewayXor` (no `direction` field, no
compiler-exposed join-pairing oracle; its DSL counterpart's `join` id is
an explicit AST annotation with no IR equivalent), `BoundaryTimer`,
`BoundaryError`, `MessageWait`, `HumanWait`, `SendTask`, `MultiInstance`,
`TimerWait`, `FfiServiceTask` (confirmed via the earlier trace: none has
an `ExecutionNode` representation in `WorkflowExecutionPlan` at all) —
fails closed with a typed `IrPlanError`, never a lossy shoehorn. Extracted
`derive_delivery_mode` (the P6/L8 "Blocking is derived, not chosen"
formula) out of `linter.rs`'s inline Pass 6 into a shared pure function in
`plan.rs`, called identically by both the AST path and the new IR path —
literally the same code computing delivery mode either way, not two
divergent implementations of the same rule. Graph-authored `ServiceTask`
nodes get no placeholder inference (IR's `task_type` is an external-job
dispatch identity, not a catalogue `domain:verb` symbol — attempting
registry resolution on it would be a category error) and default to
`BestEffort` through the same formula, fed honestly with no catalogue
signal.

**Wired into `save_design_session_endpoint`** (`bpmn-lite-server/src/rest.rs`):
branches on `is_graph_backed()`; the graph-backed path reconstructs the
DAG, calls `dag.admit()` (same admission discipline as the graph-edit
endpoint — refusal is 422, nothing stored), then `to_ir()` + `project_ir()`,
storing the projected plan in the PLAN store. **Rider 2 ("two stores, two
roles") honored by construction**: the session store's `GraphEdit` event
log is untouched by save (already true — this endpoint never appends to
it), so the DAG stays the authoring truth there and the session remains
reopenable for edit. The template catalog's `dsl_body` display field gets
an honest placeholder string for graph-authored sessions (`"<graph-authored
session {id}; edit via the graph-edit endpoint, not DSL text>"`) rather
than a fabricated DSL rendering.

**Independent blind review** (CAREFUL tier, dispatched per
verify-don't-infer) found 2 real issues in the first pass, both re-derived
against primary source and fixed before acceptance: (1) **BLOCKER**:
`IREdge.condition` is a generic field on every edge, not just
diverging-gateway edges — `Operation::Connect` can attach one between any
two existing nodes — so a condition on a plain `Task`'s or `Join`'s
outgoing edge was silently dropped (`single_successor` returned only the
successor id, never inspecting the edge's condition), exactly the "looks
valid but behaves differently than authored" failure the module exists to
prevent. Fixed: `single_successor` now refuses (typed
`UnrepresentableCondition`) any condition on an edge whose target
`ExecutionNode` kind has no field to carry one. (2) **CONCERN**: the
diverging-gateway flow-builder used `neighbors_directed` + `find_edge`,
which silently misattributes a parallel edge's condition to the FIRST edge
between a fork/successor pair when two edges connect the same pair. Fixed:
switched to `edges_directed` (pairs each edge directly with its own
target), the same pattern `verifier.rs` already uses elsewhere in the
compiler — reusing established idiom, not inventing a new one.

**Receipts.** `bpmn-lite-compiler`: 7 unit tests in `ir_plan.rs` (linear
chain projects; matched AND pair → Split/Join; `GatewayXor` refused, not
guessed; `BoundaryTimer` refused, not shoehorned; missing-Start refused;
condition-on-non-gateway-edge refused, not dropped — the BLOCKER's
red→green; parallel-edges-don't-misattribute — the CONCERN's red→green).
`bpmn-lite-server`: 3 new tests on `save_design_session_endpoint`
(projects a graph-backed session to a real `plan_hash`; refuses when the
graph doesn't admit — composes with BLOCKER 1's graph-edit-time fix, so a
non-admitting graph can never even reach save; **receipt (ii)**, reopen-
for-edit — save doesn't touch the session's edit-log event count, and a
further graph-edit against the same session still runs real staging logic
post-save, not a corrupted/frozen reconstruction). `bpmn-lite-bus-handler`:
new integration test `graph_authored_plan_instantiation.rs`, modeled on
the existing `sage_macro_assembly_tests.rs` pattern — **receipt (i), the
"define-template" half**: a `project_ir`-produced plan defines a template
through the real `BpmnLiteBusHandler` dispatch path, including
`validate_path_family`'s closure checks, green. **The "spawn-instance"
half is `#[ignore]`d, honestly documented, not faked**: the local
`data_designer` Postgres DB this test connects to (same connection string
the pre-existing bus-handler tests use) carries a FOREIGN migration
history (`_sqlx_migrations` shows `202412`/`6` — not this workspace's
numbered chain at all) and is missing `bpmn_spawn_idempotency`; `sqlx
migrate run` correctly refuses to reconcile rather than guess. This is a
pre-existing local-environment gap no prior test ever hit (none of them
exercised "spawn-instance" before), not a defect in this work — flagged as
ops follow-up rather than forced through with a risky migration-history
edit on a DB of unknown other dependents. Full `cargo test --workspace`
after all fixes: 0 failures.

**G2 is now CLOSEABLE**: both prior BLOCKERs (1: missing `.admit()`; 3:
TOCTOU race) were fixed in the earlier WS-B blind-review pass, and
BLOCKER 2 (save-as-template for graph-backed sessions) is now implemented
per Adam's ruling, itself independently blind-reviewed and remediated.
The one remaining gap — the "spawn-instance" leg of receipt (i) — is
infra, not code, and is captured as a concrete, re-runnable `#[ignore]`d
spec rather than silently dropped.

## D. Delta table — v0.1 → v0.2 (per EOP-DIR-BPMN-DESIGN-003-001 Phase 3)

Every change tagged `sequencing` or `content`. No `content` change to a ratified constraint was made; no HALT condition arose.

| # | Change | Tag | Basis |
|---|---|---|---|
| 1 | Serial T1→T2→T3 becomes concurrent WS-A ∥ WS-B ∥ WS-C(C-now) + GOV track; tranche names → workstream names | sequencing | Directive §1/§2; V&S §16 phases' *content* unchanged |
| 2 | Charter-critical-path statement is the plan's first line; T0.1 → GOV.1 unchanged in content | sequencing | Directive §2 governance track; D17 restated verbatim (§A.1) |
| 3 | WS-A.1 gains the named board-candidate schema interface, sequenced first within WS-A.1 | sequencing | Directive §3 bullet 1; schema content itself unchanged (Q2/Q27 close as before) |
| 4 | WS-B disposition path calls WS-C's policy function from day one (tier-0 + Sage initial producers); WS-B.3's D20 flow wired live instead of stubbed | sequencing | Directive §2 WS-B; architecture already ratified (D7/D8/I27 — one deterministic disposition path); only *when the code exists* changes |
| 5 | G2's "SLM-insertion readiness" structural-readiness item removed; replaced by "full pipeline (board → tier-0 → policy → record) demonstrably in the loop with records written" | sequencing | Directive §1 (item declared obsolete) + §3 (G2 re-scope). Verification is strictly strengthened: the same three seams are now exercised as running code under the same blind-reviewed gate, not reviewed as interface promises. No criterion weakened |
| 6 | T3 → WS-C split into C-now (ungated) / C-gated (Q9) / C-bakeoff; C-now built immediately with capture switch OFF | sequencing | Directive §2 WS-C; D17 restated verbatim — the split *encodes* the gate rather than deferring the code |
| 7 | C5 trace promoted from T3.0 entry gate to WS-C's immediate first task; known register state (matcher located, R10-versioned) recorded in the task | sequencing | Directive §2 ("C5 trace is now an immediate task — locate the matcher first"); trace content unchanged |
| 8 | Tier-0 corpus recall@K baseline measurement moved from C-now trace into C-gated with an explicit charter-timing flag to Adam | sequencing | Directive §2 C-gated ("flag it to Adam explicitly… rather than assuming it is exempt"); tightens, not loosens, D17's application |
| 9 | GOV.2 placement presumption updated: ob-poc workspace → bpmn-lite/standalone deploy unit as the working presumption, decision still open in GOV.2 | sequencing | Adam's standalone ruling 2026-07-27 (EOP-SAGE-REPL-BPMN-001 T0) as decision input; V&S is silent on repo placement — T0.2/GOV.2 was always the open decision slot; no ratified clause touched |
| 10 | §A added: ratified constraints restated verbatim-in-substance as a standing section | sequencing | Directive §1 ("restate verbatim in the amended plan, do not weaken") |
| 11 | Shadow-start definition added: the day WS-B's session loop first runs against a real Pack | sequencing | Directive §3 bullet 2 |
| 12 | G1, G3 (criteria + threshold table + ladder), T4/G4: untouched | — | Directive §1 items 5, §3 bullet 4 |

---
*v0.2 restructured 2026-07-27 per EOP-DIR-BPMN-DESIGN-003-001. Receipts append here per workstream as each closes. Amend in place.*
