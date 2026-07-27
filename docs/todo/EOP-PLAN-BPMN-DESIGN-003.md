# EOP-PLAN-BPMN-DESIGN-003 — Implementation Plan: BPMN Designer, Sage/Repl UI, and the SLM Prototype

**Version:** v0.2
**Status:** DRAFT — for review; executes EOP-VS-BPMN-DESIGN-003 **v0.6 (RATIFIED)**; restructured per **EOP-DIR-BPMN-DESIGN-003-001** (Adam, 2026-07-27: SLM capability built IN-PHASE, concurrent with the Designer refactor, not a later tranche)
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
exact-match pinning. GOV.2 rider stands: `/dev/rust` still needs a remote
before anything can rev-pin it.

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
