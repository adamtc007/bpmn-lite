# EOP-VS-BPMN-DESIGN-003 — Sage, Repl and the BPMN-Lite Runtime

**Version:** v0.7
**Status:** **RATIFIED** (v0.6 Adam 2026-07-25; amendment v0.7 Adam 2026-08-11) — v0.7 adds §20 (gameboard turn model, loop unrolling, content identity, parameter manifest). v0.6 rulings 1–4 confirmed (Q7 provisional option (a); D19 with the helpful-denial rider below; D20; the R1-7 board sub-decisions). This document is the framework for implementation; EOP-PLAN-BPMN-DESIGN-003 decomposes it. Changes from here are versioned amendments.
**Document class:** Version & Strategy
**Scope:** BPMN-Lite runtime philosophy, Business DSL boundary, BPMN Designer, Sage/Repl interaction, and constrained language-model insertion
**Baseline:** EOP-VS-BPMN-ISA-002 **v0.19 — IMPLEMENTED** (all plan gates V1–V8 closed; destructive cutover landed 2026-07-24). Every substrate claim in this document is aligned to that text; where this document paraphrases it, ISA-002 governs.
**Related documents:** EOP-VS-BPMN-ISA-002 (normative baseline); EOP-PLAN-BPMN-ISA-002 (receipts); EOP-VS-DSL-LANGUAGE-DESIGN-004; SemOS DAG architecture v0.5

---

## Changelog

**v0.6 → v0.7 — AMENDMENT (§20).** Four items ratified 2026-08-11, all appended as §20 rather than rewritten in place, per the ISA-002 amendment pattern. (1) **Gameboard turn model** supersedes the v0.6 proposal vocabulary — `DesignPosition`/`LegalMove`/`GameDisposition`/`MoveAttemptOutcome` with receipts; three implementation properties ratified as invariants I29–I31 (legal moves are compiler-proved, history cannot affect legality, one authority shot per staged proposal). (2) **Loop unrolling** ratified as the lowering strategy (D21) with a capped total unrolled size (I32), deterministic per-copy identity (I33), and the audit position stated (D22); `AstMutator` retirement becomes a migration, sequential MI stays out of scope. (3) **Content-derived graph identity** added alongside route-derived (D23, I34) with the loosened staleness semantics stated rather than discovered. (4) **Parameter manifest** derived by the linter's unresolved-reference walk, sealed with the template, typed by three slot kinds — scalar / collection / element-scoped (I35, I36). New open question Q31 (LSP protocol surface). §20.6 records two untraced figures the author supplied to a research directive, corrected.

**v0.5 → v0.6 — RATIFICATION.** Rulings 1–4 confirmed by Adam. One rider on ruling 2 (D19), applied in place: the denial should be *helpful about the path forward while generic about the request* — "this cannot be executed because it is not part of your current working context", optionally offering the nearest legal alternatives from the board and the governed route (context change per D20, or an access request through governance). It still never confirms the requested operation exists and never enumerates what the user cannot do.

**v0.4 → v0.5** — review R2 dispositioned (consistency-and-contract pass; all findings accepted; two contract rulings made). Sage-first / SLM-selects wording propagated out of §9.1, §10.2, §11.1, §11.8, §12.2, §13.1, D7, D9, I14 — the §10.6 canonical chain is now the only architecture in the document. The tier-1 contract is narrowed to what a cross-encoder actually produces (scalar `FiniteScore`; no span/slot promise until a certified producer exists — R2-2, ruled provisional option (a): ranker-only with deterministic slot resolvers, multi-peak compound cases escalate to Sage). Replay now closes over the disposition (`disposition_policy_hash`, `context_projection_hash`, `retrieved_subset_hash`, canonical tie-break — R2-3). The pre-inference policy filter's consequence is ruled (R2-4, D19): NL requests for forbidden operations resolve as off-board with a **generic** denial; `Forbidden` is reachable only from explicit references. Sage's escalation protocol for off-board/wrong-pack is ratified (D20): explain absence → propose governed context change → rebuild a new content-addressed board → rerun; never silent board expansion. Refinements: explicit `NONE_OF_THE_ABOVE` abstention candidate; canonical-order rationale corrected to reproducibility (position-invariance is a *test*); absolute promotion criteria added to the D3 gate. New: I28, D19/D20; Q7/Q8/Q30 re-scoped.

## Review R2 Disposition (v0.4 → v0.5)

| # | Finding | Disposition |
|---|---|---|
| R2-1 | Sage-first / SLM-selects wording survives in eight locations, contradicting D8/I27 | **ACCEPTED — blocker.** All named locations corrected; the canonical chain replaces every architectural diagram; D7/D9/I14 rewritten per the review's recommended wording. |
| R2-2 | A conventional cross-encoder emits a scalar score, not span/slot evidence; the contract promised what no chosen architecture produces | **ACCEPTED — blocker, with a contract ruling.** Stable contract narrowed to the review's provisional form (`FiniteScore`, subset/board/bundle hashes). Q7's fork is now explicit — (a) ranker-only + deterministic resolvers, (b) multi-task heads, (c) separate extractor — **ruled provisionally (a)**: deterministic slot resolvers over boarded schemas, and multi-peak/compound cases escalate to Sage rather than pretending score topology distinguishes ambiguity from compound intent. Slot/span evidence enters the contract only as the output of a separately identified, separately evaluated producer. |
| R2-3 | Recorded scores alone do not reproduce the disposition | **ACCEPTED — blocker.** `disposition_policy_hash`, `context_projection_hash`, `retrieved_subset_hash` added; `FiniteScore` rejects NaN/Infinity (the model layer inherits the canonical encoder's rule); equal scores tie-break on canonical candidate ID; §11.8 provenance carries the full closure. |
| R2-4 | Pre-inference filtering makes `Forbidden` unreachable for NL requests; the text promised both | **ACCEPTED — blocker, ruled (D19).** The concealment is **intentional**: a natural-language request for an operation the user cannot invoke resolves as off-board/weak, and the denial is *generic* — "not available in your current context" — never confirming the operation exists. `Forbidden` remains reachable where an operation is referenced **explicitly** (DSL text, a manual selection, a staged patch naming it): Repl's recheck then returns `Forbidden` because an explicit artifact, not a model inference, put the operation in play. A denial-recognition model head is rejected — it would re-expose the forbidden catalogue to the model, defeating the filter's purpose. |
| R2-r1 | Explicit null/abstention hypothesis | **ACCEPTED.** Provisional: an explicit `NONE_OF_THE_ABOVE` candidate on every board, trained on off-board examples, calibrated per pack; an open-set/OOD head remains a Q8 alternative if calibration proves fragile. |
| R2-r2 | Canonical order is reproducibility, not bias prevention | **ACCEPTED.** Rationale corrected; position-invariance of an independently-scored cross-encoder becomes a standing test — order-sensitivity is a batching/model defect, not something ordering "prevents". |
| R2-r3 | "Beats tier-0" is insufficient for promotion | **ACCEPTED.** Absolute criteria added to the D3 gate: minimum sample sizes with confidence bounds, per-risk-class non-regression, maximum accepted/published FP rates, required abstention coverage, minimum tier-0 recall@K, maximum latency. |
| R2-r4 | Sage cannot fix wrong-pack while pinned to the same board | **ACCEPTED — ratified as D20.** Sage's permitted escalation: explain absence → propose a governed context/pack/subject change → a **new** content-addressed board is built → inference reruns. Never silent expansion of the existing board. Closes Q30's protocol; residual is the abstention-evidence shape. |

**v0.3 → v0.4** — adversarial review R1 dispositioned (all findings accepted; two sub-decisions ratified inside F7; two new open questions). The load-bearing changes: the SLM fast path no longer runs behind Sage (§10.6 routing corrected — Sage is enrichment and escalation, never a mandatory upstream dependency, and tier-1 always receives the raw utterance); the D3 safety claim corrected from "zero unsafe false-positive executions by construction" to "zero direct model-authorised executions by construction", with semantic false positives measured at proposal, ratification, publication, and execution boundaries; Q9 (data governance) converted from an open question into a **Phase D3 entry gate**; the tier-1 output contract narrowed to ranking + evidence with dispositions issued by deterministic policy (§10.3); tier-0 oracle recall and board completeness added as load-bearing metrics (§10.7); the model artifact widened to a sealed bundle with recorded-scores-as-truth replay stance (§10.8); the candidate board given a content-addressed construction contract (§11.7); promotion scoped by surface and risk class, prototype capped at Designer suggest-only/staged-patch (§16 D3); explicit ratification boundary added (§18). New: I26/I27, Q29/Q30, D17/D18.

## Review R1 Disposition (v0.3 → v0.4)

| # | Finding | Disposition |
|---|---|---|
| R1-1 | Sage-first sequence defeats the SLM fast path; SLM must retain raw-utterance access | **ACCEPTED — blocker.** §9.1/§10.6 rewritten to the corrected routing. The finding also catches a hallucination-laundering hazard (an invented Sage mention becoming an apparently established SLM input) now guarded by I26. This was a genuine contradiction between §10.5's cost claim and the pipeline as drawn. |
| R1-2 | "Zero unsafe false-positive executions by construction" indefensible; contradicts §13.5 | **ACCEPTED — blocker.** Reworded per the review; boundary-tracked FP metrics adopted; accepted-and-published FP rate is the safety-relevant metric. Recorded as an instance of the standing slogan-vs-specification failure pattern — the claim named what the architecture prevents (direct model-authorised execution) with words that promised more. |
| R1-3 | Live capture before data governance is the wrong dependency order | **ACCEPTED — blocker.** Q9 is now a D3 entry gate with the review's charter items as its deliverable list. Corpus-only work (the existing 30k utterances) may proceed under the same charter applied retrospectively before any training use. |
| R1-4 | Tier-1 contract conflates ranking with extraction/disposition | **ACCEPTED.** `SlmResult`/`RankedCandidate` adopted; `ProposalDisposition` is issued by the Utterance Engine's deterministic disposition policy over model evidence — consistent with P9/I16, which this document should have applied to its own model layer. Entity resolution constrained to board selection or evidenced spans (I27). |
| R1-5 | Tier-0 recall is load-bearing | **ACCEPTED.** Metric decomposition and hard-case suites adopted (§10.7). |
| R1-6 | Model artifact must be a full sealed bundle; replay consumes recorded scores | **ACCEPTED.** §10.8 added. The replay stance mirrors ISA-002's own canonical-form-drift risk (R1 there): floating-point re-inference across hardware is the model layer's version of nondeterministic serialisation, so decision-time records are the historical truth and re-inference is forensic. |
| R1-7 | Board construction underspecified | **ACCEPTED, with two sub-decisions ratified here:** (i) policy-forbidden candidates are filtered **before** inference (disclosure minimisation; Repl rechecks everything regardless — defense in depth, not a substitute); (ii) candidate ordering is normalised to a deterministic canonical order before inference (position-bias prevention), with the order recorded in the board hash. Uncertain subject resolution and wrong-pack handling become Q29/Q30. |
| R1-8 | Promotion must be scoped by surface | **ACCEPTED.** Promotion ladder adopted; the prototype stops at Designer suggest-only/staged-patch; every further surface requires its own corpus, threat model, and gate (D18). |
| R1-scope | Ratification risk: runtime restatement dominates the SLM decision | **ACCEPTED.** Explicit ratification boundary added to §18. |
| R1-wording | Recommended governing SLM decision text | **ADOPTED** near-verbatim as §10.6's governing statement and D8's revised text. |

**v0.2 → v0.3** — realigned to ISA-002 v0.19 as an implemented baseline; corrections where v0.2 asserted mechanisms ISA-002's own corrections supersede; implementation framework added:

- **Dynamic arity corrected.** v0.2 described `FORK-DYN` as a kernel mechanism with V-3 weakened to `arity ≤ n`. ISA-002's §18 ruling H correction is adopted: dynamic arity is a **compiler lowering pattern** — `V2Fork.targets` is a compile-time-fixed array, the kernel always spawns `targets.len()` fibres, and an inapplicable branch (false gateway condition, MI index past collection length) lowers to skip straight to its `V2Join`. V-3 never weakened; `VerifiedLimits.max_fibers` reads the static bound directly. §4.6, §11.2, §12.1 rewritten.
- **Registers removed.** ISA-002 §28 deleted `Fiber::regs`; fibre state is PC + operand stack + control stack + status. §5 corrected.
- **Content-based message correlation adopted (§28/V7).** Correlation keys are content strings resolved from process data via `BindingSource` into the `v2_corr_sources` artifact table, one shared derivation, Camunda-8 floor, message start events deferred (F1). New §4.7; Designer correlation authoring added to §11.
- **Guard budgets are artifact-resident (§31/V8).** v0.2's "budgets currently hardcoded, a gap this surface eventually closes" is superseded: `v2_guard_budgets` + `default_guard_budget` exist in the artifact envelope, authored as a boundary-event `failureBudget` annotation. §11.6 rewritten around what now exists; the DTO round-trip budget-loss deviation (§31.1 #5) recorded as a Designer persistence constraint (C7).
- **Three-way zero/limit semantics.** Gateway zero-match → resumable Incident; MI empty collection → legal, completes; MI over-declared-max → `ResourceLimitExceeded`, a hard non-resumable typed error — three distinct rules, deliberately not unified (ISA-002 D5–D7, `V2MiArityCheck`). §4.3/§4.6.
- **Timer cycles precise.** `GUARD-TIMER>` fires once by default (§26 fix); `GUARD-TIMER-CYCLE>{max_fires}` bounds repetition; cycles are non-interrupting only; unbounded `R/PT` recurrence is rejected at parse and remains a genuine open decision. §4.5.
- **Word inventory corrected** to the ratified D2 vocabulary plus the named lowering-internal glue words (25 instructions), replacing v0.2's "16 V2* words". §3/§5.
- **Ring 4 deviation (§29)** adopted: detect→isolate→surface, PITR recovery, `replay()` on-demand forensic only. §5.4.
- **Terminology aligned to ISA-002 §2 glossary** (concurrency record, handle, barrier activation record, instruction cell, token-as-fibre). §5.2.
- **§30's deviations table adopted as the language profile.** Former Q20 substantially resolved: the profile exists and is normative; the Designer's job is to surface it, not restate it.
- **§16 Implementation Approach added** — the framework for building the Sage/Repl Designer UI as the prototype vehicle for the SLM inserted into the utterance → Repl discovery pipeline, phased D1–D4 with gates.
- **C-table updated:** C2 resolved-corrected (shared oracle is `compute_post_dominators`, ISA-002 §23), C6 resolved (landed through cutover), C4 partial (`Value::Array` and the flags/payload data model exist), C1/C3/C5 remain open, C7 added.
- v0.2→v0.1 changes retained (Terminate/Incident model, guard trichotomy, effect error classes, declared ceilings, self-certifying-pairing argument for P9, in-browser validation decision, slogan-vs-specification discipline).

---

## 0. Code Claims Requiring Trace

Claims this document makes about repository state beyond what ISA-002 v0.19's own receipts establish. Nothing gated on an open claim is dispatched until it is traced.

| # | Claim | Depends on it | Status |
|---|---|---|---|
| C1 | The kernel (`apply`), verifier, and canonical decoder build for `wasm32-wasip2` with a native/WASM replay-hash equality gate in CI | D11, §11.3, Phase D4 | **OPEN** — not evidenced in ISA-002 v0.19; trace before D11 ratification |
| C2 | ~~`dsl::rpst` is reusable as the pairing oracle~~ **RESOLVED-CORRECTED:** the shared structural oracle is `compute_post_dominators` (Cooper/Harvey/Kennedy) from which `compute_region_map`/`compute_gateway_pairing` derive (ISA-002 §23). Residual: whether that module is exposed at a crate boundary a Designer-side builder can consume (L1 layering, public-API gate) | §12.2, Phase D1 | residual **OPEN** |
| C3 | The synchronous macro runtime supports named placeholder bindings for identifiers created by earlier steps | §7.6, Q15 | **OPEN** |
| C4 | The instance data model can carry §7's nested tagged-union envelope shapes | §7.4 | **PARTIAL** — `Value::Array` (bounded by `MAX_VALUE_ARRAY_LEN`/`DEPTH`), scalar flags, and `domain_payload` exist (ISA-002 §18 K Pt 2, §28); the mapping of a nested tagged union onto flags vs `domain_payload` needs one deliberate design note, not discovery |
| C5 | The Candle-based utterance matcher exposes candidate scores usable as a ranking baseline without retraining | §10.3, Phase D3 | **OPEN** — locate the matcher; if outside visible repos, record where searched |
| C6 | ~~Nesting-aware pairing landed~~ **RESOLVED:** §19–§24 landed; cutover landed (ISA-002 §30) | — | closed |
| C7 | **(new, fact not trace)** The DTO authoring round-trip drops `failure_budget` (ISA-002 §31.1 #5, accepted deviation): XML-authored budgets survive compilation but not a DTO round-trip | §11.6, Phase D2 | documented constraint — the Designer's persistence path must not route budgets through the DTO surface |

---

## 1. Executive Summary

BPMN-Lite is not a conventional BPMN engine built around mutable process-variable bags and runtime interpretation of BPMN XML. It is a specialised, persisted stack-machine runtime over a compiler-validated, **acyclic** typed control-flow graph of stable execution states — and as of ISA-002 v0.19 it is **implemented**: the D2 word set with K-theorem discharge, the concurrency-record model, integrity rings 1–3 + 5 on the claim/hydrate and park/resume paths, V-1..V-11 in the verifier over every artifact, canonical tagged-binary encoding with golden bytes and a fuzz corpus, and content-based message correlation to Camunda-8 parity. This document no longer designs against a moving substrate; it designs against that baseline, and ISA-002 §30's deviations table is the normative statement of what the runtime does and does not do.

The graph is the workflow program at the design level. Its nodes are stable execution states; its connectors are typed routing alternatives; its adapters are the only route to side effects, waits, decisions, and bounded domain calls. The graph is validated before admission, lowered into a compact instruction program, and executed by fibres in a deterministic stack machine. Durable frames persist the continuation required to stop, wait, recover, replay, and resume.

Three representations, three lifetimes:

1. the **Designer DAG** — the authoritative authored process topology;
2. the **compiled artifact** — the verified, sealed, content-addressed instruction program, carrying its data-binding side tables (`v2_ffi_task_decls`, `v2_corr_sources`, `v2_guard_budgets`) inside the hashed envelope;
3. the **persisted frame** — the durable continuation of one invocation, content-bound to its artifact by hash.

The Business DSL and BPMN-Lite divide responsibility cleanly. The DSL expresses business-domain intent with typed arguments and owns semantic state transitions. BPMN-Lite owns durable temporal execution: sequence, waiting, racing, interruption, structured concurrency, retry, correlation, persistence, and eventual return to the suspended DSL invocation. A BPMN-backed invocation is validated and resolved **once** at dispatch into a typed, version-bound envelope delivered as instance data. The data rule is **pointer, not cargo**.

The Sage/Repl split remains fundamental. **Sage** is the free-thinking language collaborator; the **Utterance Engine** is the formal boundary that maps language onto a closed candidate board; **Repl** is the adjudication pipeline that proves proposals typed, reachable, and compilable. A small model inside the Utterance Engine ranks candidates on the board; it is non-authoritative. The components exist today — the Candle-based matcher, the ~1,300-verb catalogue, the 30k-utterance corpus — and §10 governs their promotion, not their invention.

**The Designer UI is the prototype vehicle.** The Sage/Repl-designed UI, with the SLM inserted into the utterance → Repl discovery pipeline, is what this V&S exists to make buildable; §16 gives the phased framework. Its primary output is a typed DAG constructed through deterministic graph operations and validated by the **production** verifier — the same theorems, the same module, potentially in the browser (D11, gated on C1).

Three designer-level concepts this revision holds as settled because ISA-002 ratified and implemented them:

- **Terminate vs Incident vs limit violation.** Terminate: the workflow decided to stop. Incident: the workflow couldn't decide (gateway zero-match, unrouted business rejection, exhausted retry) — resumable, never erased, never a result-contract variant. Limit violation (`ResourceLimitExceeded`, e.g. MI over its declared max): a hard, non-resumable typed error — an artifact/input invariant broken, not a routing gap. Three rules, deliberately not unified.
- **Static ceilings are authored.** A gateway's out-degree, an MI activity's mandatory declared maximum, a guard's `failureBudget`, a cycle's `max_fires` — all originate as authored declarations that the compiler seals into the hashed artifact. The Designer is where those numbers enter the system.
- **Data-binding side tables are the authoring target.** ISA-002's design principle — side tables are code, fibre state is data — permits artifact-resident data-binding tables (V-9 forbids control-flow tables). Adapter declarations, correlation sources, and guard budgets all land there; the Designer authors them.

The governing statements:

> **DSL owns business meaning and invocation identity. BPMN-Lite owns durable temporal execution and continuation state.**

> **Sage proposes meaning. The Utterance Engine discovers admissible formal interpretations. Repl proves executability.**

> **The Designer constructs the same typed graph the production compiler validates, and authors the declarations the sealed artifact carries. The DAG is the active design artifact, not a picture of a hidden program.**

---

## 2. Architectural Principles

Each load-bearing principle names one thing it permits that looks forbidden, and one it forbids that looks permitted.

### P1 — The DAG is the authored process program

The authoritative Designer output is a typed, traversable, **acyclic** directed graph. A diagram may project it; a designer command language may serialise edits to it; neither supersedes it.

Acyclicity is normative, not aspirational: ISA-002 D3 (deviations table) — SESE-only, any cyclic `IRGraph` rejected at compile time by `is_cyclic_directed`, repetition only via finite multi-instance or bounded `GUARD-TIMER-CYCLE>`, infinite recurrence rejected. V-8 rejects backward edges; V-11 requires a forward path to a terminal from every instruction.

*Permits (looks forbidden):* a reminder that fires every 24 hours, up to N times, until a document arrives — repetition without a backward edge, via a non-interrupting guard's bounded timer cycle.
*Forbids (looks permitted):* a connector from "review rejected" back to an earlier upstream node — even though every BPMN drawing tool will happily draw it. Bounded re-entry is expressed as forward rework topology or MI, never a cyclic edge.

### P2 — Nodes represent stable execution states

A node is a stable point at which the process can be identified, inspected, routed, or acted upon. Landing on one may invoke a declared side-effect adapter, a bounded synchronous DSL verb, a typed decision, a human task; arm a wait, timer, or race; enter or leave a structured concurrency extent; or return a typed outcome. A node does not contain an unrestricted embedded program.

"Stable state" is a design concept; persistence granularity is the **transition**, not the instruction (ISA-002 D1): the kernel executes non-parking instruction bursts atomically, and commit frequency is proportional to waits.

### P3 — Connectors express typed control flow

Outgoing connectors are compiler-known continuations selected by typed outcomes. For effect nodes, the outcome type is the ratified error-class taxonomy (§4.4). Adapters return outcomes; they never return program addresses.

### P4 — Side effects occur only through adapters

The kernel performs no arbitrary I/O. Adapters have typed contracts declared in artifact-resident data-binding tables, deterministic identities, idempotency rules, and explicit ownership; results that influence execution are captured into journaled transition history.

### P5 — Compilation, not runtime interpretation

BPMN XML, the Designer language, and the DAG are compile-time inputs. Runtime executes the sealed artifact. The race/join/boundary control-flow side tables do not exist in the v2 envelope (V-9); what the envelope does carry — adapter declarations, correlation sources, guard budgets — is data binding, hashed and pinned.

### P6 — Persistence stores continuation, not a business-data bag

The frame persists what the VM needs to resume (§5.4) and is content-bound: it names its artifact by hash inside its own digest. It is not a second store for domain aggregates.

### P7 — Probabilistic interpretation is bounded and non-authoritative

Sage and any SLM propose. Only deterministic gates establish that a proposal is reachable, well typed, valid in state, compilable, authorised, executable.

### P8 — One production compiler is the structural oracle

The Designer uses the real DAG builder, validator, lowerer, verifier, and compiler. A designer-only approximation would permit visual success followed by compile failure and create two definitions of validity. If C1 holds, the oracle can travel to the browser rather than being imitated there.

### P9 — Models select meaning; deterministic code constructs executable syntax

Models identify semantic candidates and bindings. Deterministic builders create ASTs, graph patches, pairing derivations, and compiler inputs.

ISA-002 supplied the decisive argument and then the implemented remedy. Fork/join pairing annotations are **self-certifying**: lowering produces the annotations V-3 checks, so a mispairing with agreeing arities passes verification (§19, Adam's ruling verbatim). Even a deterministic BFS stack got this wrong — the fix rebuilt pairing, region mapping, and layout on one `compute_post_dominators` (§23). A probabilistic model emitting pairing is therefore not a risk to mitigate but a category error.

*Permits:* a model choosing *which* production to apply and *where* to anchor it.
*Forbids:* a model emitting node IDs, edges, pairing metadata, or instruction streams — even correct-looking ones, and even under a validator, because some construction errors are invisible to the checks they feed.

### P10 — Stable intent is captured; mutable facts are observed explicitly

The durable invocation records what the caller asked. Mutable domain facts are read through explicit typed operations; if a fact affects routing, the observation is committed so replay cannot silently obtain a different answer.

### P11 — Ceilings are declared at design time

Every statically verified resource bound originates as a Designer-visible declaration or a derivable graph property: fork out-degree, the mandatory MI maximum, `failureBudget` per guard plus the workflow default, `max_fires` per timer cycle. The compiler seals them into the hashed artifact — a changed budget is a changed artifact (ISA-002 §31), never a mutated config.

*Permits (looks forbidden):* a gateway whose live branch count is unknown until runtime — provided the static branch set bounds it, which it does by construction (§4.6).
*Forbids (looks permitted):* an MI activity with no declared maximum, however reasonable "the collection is always small" sounds — a named deviation from Zeebe (D5), and over-max at runtime is a hard error, not truncation.

---

## 3. BPMN-Lite Runtime Philosophy

BPMN-Lite is:

> **A deterministic workflow virtual machine whose source programs are expressible as a constrained BPMN graph.**

The FORTH lineage is explicit in ISA-002 §1: a fibre is the FORTH inner-interpreter shape — the minimal machine whose complete state serialises at any instruction boundary — with v1's parameter stack completed by v2's control stack, and one deliberate departure: persistence granularity is the transition, not the instruction.

A durable execution resembles a persisted coroutine:

```text
DSL caller
    │ invoke durable verb with typed arguments
    ▼
Durable invocation
    │ bind artifact + input envelope (instance data)
    ▼
BPMN-Lite VM
    │ traverse compiled control flow
    │ invoke effects and bounded domain verbs
    │ wait, persist, recover, replay
    │ race, fork, join, guard, cancel
    │ raise incidents where the map has no answer
    ▼
terminal typed outcome
    │
    ▼
DSL continuation resumes
```

The caller does not see nodes, reminders, retries, races, fibres — or incidents. It waits on the semantic durable invocation and receives the result promised by the verb contract.

The three-way distinction is essential: the **DSL invocation** is the stable semantic contract; the **BPMN artifact** is the temporal implementation selected for it; the **BPMN instance** is the current durable execution, whose operational states (`Running`, `Incidented`, quarantined, terminal) belong to operations, never to the caller's result type.

The instruction surface the Designer ultimately targets is ISA-002 D2's ratified vocabulary — the guard trichotomy with arming triggers, `RACE{`/`ARM-*`/`}RACE`, `FORK`/`JOIN`, the wait words, `AWAIT-EFFECT`, `CANCEL-SCOPE` — plus the named lowering-internal glue words (`V2RouteZeroMatch`, `V2LoadPlaceholderMatch`, `V2MiIndexLive`, `V2MiArityCheck`, `V2MiLoadElement`, `GUARD-TIMER>`, `GUARD-TIMER-CYCLE>`), 25 instructions in all. The Designer never authors instructions; productions lower to them through the production compiler.

---

## 4. The Graph: Acyclic Topology of Stable Execution States

### 4.1 Graph, taxonomy, and map

The process is a directed **acyclic** graph whose topology is the permitted runtime journey. It is also a governed map: node kinds belong to a constrained vocabulary; connector kinds have bound meanings; current position determines legal edits and continuations; reachability restricts the active choice set; structured extents constrain admissible descendants; the compiler reasons over the whole (post-dominance is the one structural computation pairing, regions, and layout all derive from — §23).

Repetition is structural, per ISA-002 D3:

- **bounded timer cycles** — `GUARD-TIMER-CYCLE>{max_fires}` on a non-interrupting guard: the record re-arms on fire, bounded by the declared count; exhaustion reverts to the declared continuation. Unbounded recurrence (`R/PT…`) is rejected at parse — a genuine open decision, not an oversight (§4.5).
- **multi-instance regions** — one inner activation per collection element under a mandatory declared maximum (§4.6);
- **forward rework topologies** — bounded re-entry modelled as forward traversal with attempt state, never a cyclic edge.

### 4.2 Stable-state node model

Each node declares: its kind; typed input bindings; any side-effect or decision adapter; possible typed outcomes; legal outgoing connectors; wait or correlation contract; scope, guard, race, fork, or join semantics; source and design provenance.

### 4.3 Connectors, zero-match, and the Terminate/Incident distinction

Connectors are closed, typed routing alternatives. The route map need not be exhaustive, and the Designer must not pretend otherwise. The implemented semantics (ISA-002 ruling J, D7):

- **Zero-match at a gateway is not a compile error.** The lowered program routes an unmatched outcome to an incident-raising instruction (`V2RouteZeroMatch`); the instance enters `Incidented{incident_id}` — non-terminal, not schedulable, resumable: an operator amends the data, resolves the incident, the gateway re-evaluates. V-11 is satisfied at every gateway by exactly this edge.
- **A route-map gap is information for whoever fixes the workflow, not something the kernel erases.** The Designer renders unrouted outcomes as *visible, deliberate incident paths* — annotated, warnable, reviewable — never silently defaulted.
- **Terminate vs Incident.** `END-TERMINATE` means the workflow decided to stop (legal even inside a parallel region — the whole instance dies, every record retires, V-1's exemption applies because there is nothing left to orphan). An incident means the workflow couldn't decide. The Designer exposes both as distinct authored constructs.
- **Limit violations are a third thing.** `ResourceLimitExceeded` (MI over its declared max, frame limits) is a hard non-resumable typed error — a broken artifact/input invariant. It is not an incident and the Designer must not offer to "route" it.

`Incidented` never appears in a durable verb's result contract (I13a).

### 4.4 Effect outcomes are typed by error class

An effect node's routing surface exposes the implemented taxonomy (ISA-002 §14 ruling D, exhaustive match, no wildcard):

- **`Transient`** — retried under a budget; attempt count via `Command::EffectFailed`; budget enforced store-side at claim time. Exhaustion → Incident.
- **`BusinessRejection`** — a domain "no". Routed by the author to a business continuation; unrouted → Incident, always.
- **`ContractViolation`** — the only class eligible for automatic rollback, and only within a `GUARD-R>` scope, subject to that guard's failure budget (§4.5); budget exhaustion quarantines rather than retrying forever.

A future error class is a compile error at every consumer, not a silent default. The Designer's effect-node property surface mirrors this exactly.

### 4.5 Guards: the implemented trichotomy

Boundary behaviour is three distinct opcodes — never flags — because the verifier must see the disposition without inspecting operands:

- **`GUARD>`** — interrupting: unwind members, spawn handler. **No data rollback** (ISA-002 ruling G removed the unconditional snapshot: BPMN boundary events route control; they do not restore data).
- **`GUARD-N>`** — non-interrupting: spawn the handler beside the members; the record stays armed.
- **`GUARD-R>`** — interrupting **with data rollback, no handler**. Restores the A3 set — `domain_payload`, business flags, `join_expected`, session stack — to scope-open values; **preserves attempt history** (loop/retry counters), whose whole purpose is to accumulate across attempts. `CANCEL-SCOPE` and automatic rollback operate exclusively on `GUARD-R>` records; targeting a plain guard handle is a verify-time rejection (V-10).

**Arming triggers.** A guard is armed by a distinct trigger word — `GUARD-TIMER>` `( duration -- )`, verifier-enforced to immediately follow the guard it arms — so a boundary timer *is* a guard with a deadline, indifferent to whether the guarded work is synchronous, an effect, or a nested subprocess. `GUARD-TIMER>` fires **once** by default; `GUARD-TIMER-CYCLE>{max_fires}` (non-interrupting targets only; cycle-on-interrupting is rejected at verification) bounds repeated firing, mirroring BPMN `R<n>/PT<d>`. Handler-spawning guards redirect control safely: the handler inherits the enclosing control stack — a barrier handle among it — and is registered as a member of every ancestor record, which is exactly BPMN's token-moves-to-the-handler semantics.

**Placement rule (V-10), Designer-visible.** A `GUARD-R>` scope must dominate any `FORK` in its extent — it may contain a complete fork/join region but must not be contained by one. Rollback is instance-wide in data effect, so it must be instance-wide in control extent; this makes barrier starvation via rollback *unconstructible*, not merely detected. The Designer renders it as "a rollback boundary must enclose the whole of any parallelism it covers", refused at staging. The honestly-recorded cost stands: a data-rolling-back error handler cannot be written on a task inside a parallel branch — and BPMN never offered that anyway.

**Failure budgets (ISA-002 §31).** Each guard may declare its own `failureBudget`; un-annotated guards inherit the workflow default; the ceiling is compiled into `v2_guard_budgets` inside the hashed artifact, while the counter lives store-side, claim-gated. The Designer authors both the per-guard budgets and the workflow default (§11.6).

### 4.6 Concurrency: forks, races, joins, multi-instance — the corrected mechanism

- **FORK/JOIN** — structured parallel regions. `FORK n` allocates a fresh **barrier activation record**; each child inherits the parent's control stack with the barrier **handle** pushed; `JOIN` pops the inherited handle — resolution is by dynamic handle only, the static pairing id is verifier annotation. Survivor semantics: the last-arriving fibre continues; the N−1 others are deleted at barrier retirement.
- **Dynamic arity is a lowering pattern, not a kernel mechanism.** *(Correction from v0.2.)* `V2Fork.targets` is a compile-time-fixed array; the kernel unconditionally sets `arity = count = targets.len()`. An inclusive gateway or MI activity always lowers to a `V2Fork` with its full static maximum; a branch whose condition is false — or whose MI index exceeds the runtime collection length (`V2MiIndexLive`) — is compiled to jump straight to the shared `V2Join` and still makes a real barrier arrival. **V-3 never weakened**; `VerifiedLimits.max_fibers` reads `targets.len()` directly. Designer consequence: the arity ceiling *is* the branch set the author draws — no separate declaration exists or is needed for gateways.
- **RACE{ / ARM-\* / }RACE** — first-wins over declared arms; arms are registrations only (V-5), never imperative work; resolution runs the winner and cancels losers in one transition.
- **Parallel multi-instance** — implemented per ruling K with per-element value access: `inputCollection` is a bounded `Value::Array`; `V2MiArityCheck` hard-rejects a collection exceeding the artifact-declared maximum (`ResourceLimitExceeded` — deliberately *not* the incident mechanism); each live branch receives its own element **by value** via `V2MiLoadElement` into a per-branch flag flowing through the same `orch_flags` pipeline every service task uses. Empty collection is legal and completes immediately (D6). `completionCondition` deferred (F2) — a race over a barrier, wanting its own design pass. The Designer's MI production must therefore bind: the collection source, the mandatory maximum, and the inner activity's element parameter — and must not offer completion conditions.

### 4.7 Message correlation is content-based

*(New; ISA-002 §28/V7.)* A correlation key is a **content string resolved from process data**: at lowering, a `correlationKey` expression (`=case_id`) resolves via the existing `resolve_expression` into a `BindingSource`, carried in the artifact's `v2_corr_sources` table keyed by the wait/publish/arm instruction's address. At park/arm/publish time the source resolves against instance data to a scalar, canonicalised by **one shared** `correlation_key_string` — waiter, publisher, and wire derive the key identically or matching silently fails, so no second derivation path may exist. External publishers send raw content (`"ACME-42"`), as Camunda 8 does. Message **start events** are deferred (F1) — the Designer must not offer instantiate-by-message.

Designer consequence: a wait node's correlation contract is authored as an expression over declared process data, validated at staging to resolve to a scalar (arrays/objects are typed rejects), and displayed alongside the node's wait semantics.

### 4.8 The graph is immutable at runtime

Once compiled and bound to an instance, the artifact is immutable; runtime changes the frame, never the topology. A changed definition — including a changed budget, correlation source, or MI maximum, since all ride the hashed envelope — is a new artifact identity. Existing instances remain bound to the artifact under which they began; greenfield strategy, no live migration.

---

## 5. Stack-Machine Execution and Persisted Continuations

### 5.1 Compilation chain

```text
BPMN source or Designer operations
    ↓ typed authoritative DAG
    ↓ structural validation (cyclicity gate; post-dominance region/pairing derivation)
    ↓ lowering to the v2 instruction set (+ data-binding side tables into the envelope)
    ↓ artifact verification (V-1..V-11, VerifiedLimits, budget/correlation checks)
    ↓ sealed immutable artifact (content-addressed)
    ↓ stack-machine execution (kernel preserves K-1..K-3; rings 1–3+5 at every park/resume)
```

### 5.2 Terminology (ISA-002 §2 glossary, binding)

| Term | Meaning |
|---|---|
| graph node | design-level stable state |
| instruction (cell) | one element of the compiled, flattened program — static, immutable, addressed by `Addr`; never called a token |
| token | BPMN sense only — a locus of execution, **realized as a fibre** |
| fibre | PC + operand stack + control stack + status *(registers deleted, §28)* |
| concurrency record | snapshot-resident record — scope (guard/race) or barrier — keyed by record ID |
| barrier activation record | the per-activation barrier state `FORK` allocates; carries actual arity and count |
| handle | a fibre's reference to a concurrency record, pushed on its control stack |
| static pairing id | compile-time verifier annotation; never a runtime identity |
| frame | the complete persisted context-switch unit under the Ring-2 hash |

The activation law governs addresses in runtime state: code reference / jump target (`handler`) — legitimate; runtime execution identity — forbidden; static-site identity for cross-activation accounting (`opened_at`) — legitimate. The `Addr`/`RecordId` type wall enforces it; `v2_guard_budgets` keying by `opened_at` is a category-3 use.

### 5.3 Specialised stack machine

VM state: per-fibre PC, operand stack, control stack; the shared concurrency table (canonical for membership, barrier, race, guard state, rollback capture on `GUARD-R>` records); pending waits and effects; deterministic context; artifact identity and revision. Control-stack ↔ membership consistency is K-2; member liveness K-1; barrier soundness K-3 — inductive over `apply`, shadow-asserted by Ring 3. Mid-transition reads of the concurrency table go through the pending-aware fetch, enforced by a CI lint (`check-transition-read-safety.sh`) after per-site discipline missed three times — the fourth conversion of a per-site rule into a build failure on this project.

### 5.4 Persisted continuation and integrity

The frame is the continuation: fibres, concurrency table, instance data (flags, `domain_payload`), pending effects, revision, artifact hash — hashed as one canonical closure (Ring 2: BLAKE3 over `snapshot ‖ fibres ‖ concurrency table ‖ pending effects ‖ revision ‖ artifact_hash`). Encoding is the single canonical binary form with golden-bytes and round-trip fixed-point laws; NaN/Infinity are typed rejects; `Value::Array` is depth- and length-bounded at decode *and* at the runtime limits boundary, so one slot cannot hide an unbounded tree. Frame size is proportional to live tokens, never program size.

Integrity is detect-and-fail-stop, five rings, with Ring 4 **scoped to detect→isolate→surface** (ISA-002 §29): corruption is caught at load before decode, quarantined instances are excluded from claim and emit the audit event, recovery is point-in-time restore, and `replay()` is an on-demand forensic tool — no proactive fleet scan.

### 5.5 Event-driven resumption

External activity never mutates the frame directly. A message (matched by name + content correlation key, including against the buffered-message path), timer, form submission, effect completion, or human decision arrives as a typed command correlated to an existing wait. The kernel produces the next valid transition, rejects it as stale/duplicate/unknown/illegal, or quarantines on structural violation. `ResolveIncident` is the same shape against an `Incidented` instance.

### 5.6 Return to the DSL caller

```text
DurableVerbResult {
    invocation_id,
    outcome,               // a declared terminal variant only
    output_bindings,
    completion_evidence
}
```

This resumes the suspended DSL continuation. The caller is insulated from internal topology and operational states.

---

## 6. Separation of Business DSL and BPMN Designer DSL

### 6.1 Business DSL

The Business DSL binds domain verbs to domain nouns, references, and values:

```lisp
(assign-investment-manager
  :cbu @cbu
  :entity @manager)
```

It answers: what business operation is requested; against which entities; whether it is permitted; what domain transition is performed; what typed outcome is returned; whether execution is synchronous or durable. Macros compose governed business operations and remain business-semantic executables.

### 6.2 BPMN Designer DSL

The Designer language is a graph-construction language: add, insert, replace, connect, delete; wait, race, fork, join, guard, call, return; attachment points and structured extents; typed adapter bindings; declarations (budgets, correlation sources, MI collections and maxima); productions such as request-and-wait. Its active output is a candidate typed DAG plus its declarations — not a business-state mutation.

It is persisted as a serialisable edit log for audit, deterministic replay, undo/redo, provenance, fixtures, collaboration, and reproducible generation — subordinate always to the authoritative graph and the production validator. Persistence constraint C7: the DTO round-trip drops `failure_budget`, so the Designer's own persistence path must carry declarations through a surface that retains them, not the DTO shape.

### 6.3 Durable Business DSL verbs

A verb declares `execution_kind = synchronous` or `execution_kind = durable, runtime = BPMN-Lite, artifact = <bound process artifact>`. The durable verb is the external semantic contract; its BPMN implementation may invoke lower-level synchronous verbs and adapters but must not recursively invoke its own durable contract as a disguised implementation step.

```text
Durable contract:   solicit-document
Temporal impl:      create solicitation → resolve route → send request
                    → guard the wait with a bounded reminder cycle (GUARD-N> + GUARD-TIMER-CYCLE>)
                    → register document → review evidence
                    → return Received | Rejected | Expired | Cancelled
```

### 6.4 Ownership boundary

> **The Business DSL owns semantic intent, domain validation, domain mutation, and durable invocation identity. BPMN-Lite owns temporal topology, continuation, waits, effects, structured concurrency, correlation, and eventual return.**

The Designer may select an available Business DSL verb as a node adapter; it does not reproduce that verb's domain logic inside the graph.

---

## 7. BPMN Invocation Contract and Typed Context

### 7.1 The DSL verb definition is the contract

For a BPMN-backed verb the catalogue declares: verb identity; contract version; subject kinds; input type; result type; execution kind; artifact binding or selection rule; preconditions and authorisation. The contract may be a structured nested record; its wire form may be JSON, but the semantic form is a closed, versioned type, serialised under the same canonical discipline that governs the frame hash domain.

### 7.2 Field categories

Classified, never an undifferentiated bag:

**Required inputs** — existing client, CBU, person, case, or requirement references; document type; solicitation purpose; delivery intent.

**Tagged/conditional inputs** — alternatives use tagged unions, not a channel field plus unrelated optionals:

```rust
enum DeliveryIntent {
    ClientPortal { portal: PortalRef, client_account: PortalClientRef },
    Email        { recipient: ContactRef, template: TemplateRef },
    InternalTask { queue: WorkQueueRef },
}
```

**Genuinely optional inputs** — valid to omit independently of any variant. Optionality must not conceal missing contract design.

**Late-bound results** — outputs that do not exist at initiation: submission, blob, document, task references. Result bindings, never nullable input fields.

**Collections** — where an input is genuinely plural (the per-director signature case that motivated MI), it is a bounded collection feeding an MI region, with the declared maximum authored alongside it.

### 7.3 Example contract

```rust
struct SolicitDocumentInput {
    target: SolicitationTarget,
    request: DocumentRequestSpec,
    delivery: DeliveryIntent,
}

enum SolicitDocumentResult {
    Received { document: DocumentRef, submission: SubmissionRef },
    Rejected { reason: RejectionReason },
    Expired,
    Cancelled,
}
```

Illegal results are unrepresentable. `Incidented` is absent by rule (I13a) — an operational state, not an outcome the contract offers.

### 7.4 Bind once at initiation — the implemented mechanism

Parameterization is **data, not code**: one shared immutable artifact; per-instance data enters at start and is seeded into instance state (flags and `domain_payload`); words read resolved values; no per-instance artifact overlay exists. The external boundary enforces shape: flag keys are format-checked kernel-side, and collection values are length/depth-bounded at the request boundary before they ever reach instance state — with `Cancel`/`Terminate` exempted from the collection limits so an over-limit instance can never become unreachable.

At dispatch, the DSL runtime: parses and resolves the s-expression; expands macro bindings; validates the argument record against the verb contract; resolves entity references; confirms relationships and preconditions; binds contract and artifact versions; creates the immutable envelope; starts the instance with it. The envelope's representation on the instance-data model (which parts land as scalar/array flags, which as `domain_payload`) is a deliberate design note, not discovery — C4's residual.

The frame persists the resolved envelope; execution never re-queries the source s-expression (retained, with its hash, for audit). "Self-contained" means self-describing and content-bound.

### 7.5 The DAG contains no invocation data

The graph declares expected contracts, correlation *sources*, and ceilings. It contains no client UUID, no request-specific due date, no created document UUID, and no resolved correlation *values*. Those belong to instances.

### 7.6 Named late binding

Durable and parallel execution must never depend on "the last created UUID". Results bind to named, typed slots; binding changes persist atomically with the transition that accepts the producing result. Whether the synchronous macro placeholder environment is reused across the durable boundary or given a versioned durable representation remains open (Q15, gated on C3); one placeholder model, not two.

---

## 8. Pointer-Not-Cargo Data Philosophy

### 8.1 Governing rule

> **No authoritative domain aggregate is serialised into a BPMN frame. Frames contain typed references, immutable captured results required for replay, process-local control values, and late-bound result references.**

*Permits (looks forbidden):* persisting a captured `ResolvedPortalRoute`, a `RiskRoute::Enhanced` observation, or an MI input collection of per-director references — bounded evidence and control data with declared semantics.
*Forbids (looks permitted):* persisting the client's name "just for the form label" as ordinary state. Display snapshots, if allowed at all, are explicitly marked non-authoritative (Q14).

### 8.2 Why cargo is harmful

Stale data across long waits; dual ownership; ambiguous reconciliation; privacy and retention exposure; oversized frames; business logic leaking into the runtime; replay against undefined snapshots. The substrate now makes the cost structural as well as measurable: frame size is proportional to live tokens by guarantee, collection values are hard-bounded, and `GUARD-R>` rollback capture means every byte of payload is a byte snapshotted at each rollback scope — pointer-not-cargo is what keeps rollback affordable.

### 8.3 What may be persisted

**Typed references** — `ClientRef`, `PersonRef`, `CaseRef`, `DocumentRequirementRef`, `PortalRef`.
**Stable semantic values** — `DocumentType::Passport`, `DeliveryChannel::ClientPortal`.
**Process-local values** — `retry_count`, `selected_branch`, `race_winner`, `review_attempt` — noting attempt history is *preserved through rollback* by design.
**Captured observations** — `ResolvedPortalRoute`, `EffectCompletion`.
**Late-bound references** — `SubmissionRef`, `BlobRef`, `DocumentRef`.
**Bounded collections** — MI input collections of references or scalars, under the declared maximum.

### 8.4 Delivery intent versus resolved endpoint

Invocation-time context expresses stable intent; the operational route is resolved immediately before the outbound effect and captured as evidence. Retry policy explicitly chooses between retrying the captured destination and re-resolving; never accidental (typed per-fact policy: Q13).

### 8.5 Replay-safe dereferencing

A read that can influence control flow is an explicit bounded operation whose typed observation is committed before it controls continuation. Replay consumes the recorded observation or revalidates under a defined oracle — never a silent reread.

---

## 9. Sage, Utterance Engine and Repl Architecture

### 9.1 Roles

**Sage** — the free thinker and human-language collaborator: interprets conversational and incomplete phrasing, uses dialogue context, proposes intents, decomposes compound requests, notices unstated choices, asks natural clarifying questions, explains consequences, helps users discover what the system can express.

**Utterance Engine** — the formal boundary between language and Repl: receives the **raw utterance**, session context, and an exact content-addressed candidate board (SemOS or Designer); retrieves and ranks admissible verbs, macros, operations, or productions; assembles whatever evidence its separately certified producers supply (§10.3); and through its **deterministic disposition policy** emits a non-executable proposal, clarification, or escalation. A typed Sage hypothesis, where one exists, is an *additional* feature set — the Utterance Engine is not downstream of Sage, and tier-1 always retains access to the raw utterance (I26).

**Repl** — the adjudication pipeline: resolves subjects and references; applies pack and graph reachability; checks state predicates and policy; checks required and conditional arguments; deterministically constructs the DSL AST or graph patch; invokes the production validator, compiler, and verifier; stages rather than commits design changes; returns executable, staged, ambiguous, rejected, forbidden, or out-of-scope.

### 9.2 Typed boundary

Sage and Repl communicate through typed records, not prose alone:

```rust
struct SageHypothesis {
    action_phrases: Vec<ActionPhrase>,
    subject_mentions: Vec<SubjectMention>,
    entity_mentions: Vec<EntityMention>,
    temporal_relations: Vec<TemporalRelation>,
    concurrency_hints: Vec<ConcurrencyHint>,
    exception_hints: Vec<ExceptionHint>,
    argument_hints: Vec<ArgumentHint>,
    ambiguities: Vec<Ambiguity>,
}

enum ProposalDisposition {
    Candidate(IntentProposal),
    Ambiguous(ClarificationSpec),
    MissingArguments(MissingArgumentSpec),
    Compound(Vec<PartialProposal>),
    OutOfScope(OutOfScopeReason),
}

enum Adjudication {
    Executable(CompiledRunbook),
    Staged(GraphPatch),
    Clarification(ClarificationSpec),
    Rejected(RejectionReason),
    Forbidden(PolicyReason),
}
```

A proposal is never executable merely because a model produced or ranked it — and a `ProposalDisposition` is never issued by a model at all: it is the output of the Utterance Engine's deterministic disposition policy, computed over model-supplied evidence (§10.3).

### 9.3 Human-facing persona

The Repl persona remains: the pipeline finds the exact defect or ambiguity; Sage expresses it naturally ("Should FundRock replace the current investment manager, or be added as an additional manager?"). Persona and architecture are not the same component.

---

## 10. The SLM Inside the Utterance Engine

### 10.1 Starting position: components exist; the runtime is ruled

The pipeline today comprises pretrained LLMs plugged into SemOS plus a separate Candle-based (Rust) utterance phrase matcher, over ~1,300 verbs across 10 domain packs, with a 30k-utterance corpus for training and evaluation. **Runtime is ruled: Candle.** The path is: fine-tune a compact cross-encoder offline, export safetensors, inference in Candle on CPU; tier-0 and tier-1 share one runtime; `ort` only if benchmarks force a C++ dependency, with the burden of proof on `ort`. The model ships as a sealed, content-addressed artifact — pinned weights, pinned threading — so shadow-mode disagreements are replayable evidence, consistent with the substrate's own governance model.

### 10.2 Why the opportunity exists

SemOS packs and their DAGs deterministically constrain the search space — active pack, current subject, reachable nodes, valid verbs and macros, argument schemas, candidate entities, legal graph operations. The probabilistic task has collapsed from open-ended generation into contextual ranking over a small board — with slot resolution, rejection, and dispositions belonging to deterministic policy and separately certified producers, not to the ranker.

### 10.3 The SLM remains probabilistic and non-authoritative

> **The SLM is a non-authoritative semantic candidate selector operating inside a deterministically bounded choice set.**

Never "safe because small". And the tier-1 output contract is deliberately narrow — a cross-encoder scores candidate pairs; it is not thereby a trustworthy typed argument extractor or a disposition authority:

```rust
struct RankedCandidate {
    candidate_id: CandidateId,
    score: FiniteScore,                  // NaN/Infinity rejected — the model layer inherits the canonical encoder's rule
}

struct SlmResult {
    ranking: Vec<RankedCandidate>,       // canonical candidate-ID tie-break on equal scores
    retrieved_subset_hash: SubsetHash,   // the exact tier-0 output tier-1 saw
    board_hash: BoardHash,               // the exact inference board (§11.7)
    model_bundle_hash: ModelBundleHash,  // the sealed bundle (§10.8)
}
```

This is the **stable contract**: a scalar-scoring ranker is all it promises, because that is all a conventional cross-encoder produces — attention weights and post-hoc attribution are not treated as reliable source evidence (R2-2). Slot and span evidence enter the contract only as the output of a **separately identified, separately evaluated producer**; Q7's fork — (a) ranker-only with deterministic slot resolvers, (b) a multi-task model with ranking and token-labelling heads, (c) a ranker plus a separately trained extractor — is **provisionally ruled (a)**: deterministic resolvers over boarded schemas and typed candidate entities, upgraded only when a certified producer exists with its own evaluation. Every board additionally carries an explicit `NONE_OF_THE_ABOVE` candidate trained on off-board examples, so abstention is a rankable, testable hypothesis rather than a fragile top-one threshold.

**Deterministic policy** — not the model — then decides: whether the ranking is sufficiently separated; whether required slots resolve; whether clarification is necessary; whether to escalate to Sage; whether to abstain. Multi-peak score topologies do **not** reliably distinguish ambiguity from compound intent, so compound-suspected cases escalate to Sage until separately evaluated action-span evidence exists. The SLM supplies evidence for policy's decisions; it never independently issues a `ProposalDisposition`. Entity resolution follows the same rule: selection from typed candidate entities on the board, or deterministic resolution — never generated identifiers or unboarded values (I27). It generates no s-expressions, no graphs, no instructions.

### 10.4 Where the model ends and the decision table begins

The ratified boundary rule: **a model is needed when inputs cannot be pre-defined and the system still needs a sensible output; a DMN table suffices when every input-to-output mapping can be pre-defined.** Rules and DMN operate after language is normalised into facts. They do not economically solve the preceding language problem: encoding paraphrase, omission, alias, word order, and shorthand as decision rows recreates a brittle NL parser. Short spoken utterances with cross-pack word reuse are the known hard case.

> **The model interprets how humans describe an action. Deterministic rules govern whether and how that action may occur.**

### 10.5 Why Sage remains necessary — and where it sits

The long tail: novel paraphrase, incomplete language, multi-turn context, compound intent, implicit temporal relationships, explanation, discovery, natural clarification, cold-start packs and productions. The SLM cuts cost, latency, variance, and false invention on the routine constrained path; it does not replace general language capability.

*(Corrected in v0.4, R1-1.)* For the cost claim to be true, Sage must be an **enrichment and escalation path, not a mandatory upstream dependency**: a pipeline that runs the general LLM on every utterance before the SLM has already paid the cost, latency, and variability the SLM exists to avoid. Where a Sage dialogue is already active (a conversational Designer session), its typed hypothesis feeds tier-1 as additional features — but tier-1 always retains the raw utterance, so an invented Sage mention can never become an apparently established input (I26).

### 10.6 Routing and escalation architecture

```text
raw utterance + session context
        ↓
deterministically constructed candidate board   (§11.7 — content-addressed, policy-filtered, canonically ordered)
        ↓
tier-0: existing Candle phrase matcher          (high-recall retrieval)
        ↓
tier-1: fine-tuned cross-encoder                (ranking + span/slot evidence only)
        ↓
deterministic disposition policy
        ├── sufficient evidence, separated      → deterministic binding → Repl
        ├── bounded ambiguity / missing slots   → deterministic clarification policy → Sage renders it
        └── novel / compound / weak / off-board → Sage analysis → same board → Repl
```

The governing statement (adopted from review R1):

> **The Utterance Engine receives the raw utterance, session context, and an exact content-addressed candidate board. Tier 0 retrieves a high-recall candidate subset. Tier 1 ranks those candidates and returns score and source-span evidence only. Deterministic policy decides selection, abstention, clarification, or Sage escalation. Deterministic binders construct the DSL AST or graph patch, and Repl independently re-establishes reachability, typing, policy, authorisation, and compilation. Sage is invoked for novel, compound, contextual, explanatory, or low-confidence cases and may enrich — but never replace — the raw utterance evidence.**

Even on escalation, Sage is constrained by the board and cannot confer authority on an invented verb or graph construct. For off-board and wrong-pack cases its permitted protocol is fixed (D20): explain that the active context does not contain the requested operation; propose a governed context/pack/subject change; a **new** content-addressed board is then built and inference reruns. Sage never silently expands the board it was handed.

### 10.7 Evaluation strategy

Shadow mode first — corpus work first, live capture only behind the Q9 governance gate (§16 D3). Record utterance, board hash, hypothesis where present, ranking and scores, disposition, bindings, clarification, compiler result, user correction, final executable or DAG.

**The pipeline is measured per tier, because tier-1 can only rank what tier-0 admits** (R1-5):

- candidate-board completeness (is the correct operation on the board at all);
- tier-0 oracle recall@K (is it in the retrieved subset);
- tier-1 ranking accuracy *given* oracle inclusion;
- end-to-end correct selection;
- **abstention when the oracle candidate is absent** — the model must not confidently rank a board that doesn't contain the answer;
- latency as board size and K vary.

Plus: hard-negative discrimination, slot-resolution accuracy, out-of-scope rejection via the `NONE_OF_THE_ABOVE` hypothesis (calibrated per pack), clarification precision, production and anchor accuracy, compile success, correction count, repeatability — and **position invariance**: an independently scored cross-encoder's output must not vary with board order; if it does, that is a batching/model defect the suite exists to catch. Per-pack reporting is necessary but insufficient — the hard-case suites are cross-pack word collisions, rare verbs, short spoken utterances, paraphrase-family separation, and candidates with near-identical descriptions. C5 defines the tier-0 baseline the fine-tune must beat.

**False positives are tracked at every boundary they can cross** (R1-2): wrong proposal shown; wrong proposal accepted; wrong patch published; wrong business operation executed; compiler-invalid proposals; policy/authorisation refusals. **The primary safety metric is the accepted-and-published false-positive rate** — what the architecture prevents by construction is direct model-authorised execution, and nothing stronger is claimed.

### 10.8 The model bundle and replay stance

"Pinned weights" is not a reproducible decision (R1-6). The sealed, content-addressed **model bundle** comprises: weights; tokenizer and vocabulary; preprocessing and truncation rules; base-model identity; the candidate text/schema projection (how board entries become model input); calibration parameters and thresholds; label/catalogue version; runtime and library versions; locale. `SlmResult` records the bundle hash and board hash at decision time.

A recorded ranking alone does not reproduce a disposition, so the decision record closes over everything the disposition depended on: `disposition_policy_hash` (policy version, risk-class thresholds, tie-breaking, missing-slot and abstention rules), `context_projection_hash` (the session/context features as projected), and `retrieved_subset_hash` — alongside the board and bundle hashes already in `SlmResult`. Replay consumes the **recorded ranking, scores, and disposition as the historical truth**; later re-inference is forensic comparison, never the source of history — floating-point inference may legitimately differ across hardware, which is the model layer's version of ISA-002's canonical-form-drift risk, and it is handled the same way: the decision-time record is canonical, divergence on re-run is a signal to investigate, not a rewrite of what happened.

---

## 11. BPMN Designer Architecture

### 11.1 The DAG is the primary artifact

```text
raw design utterance + session context
    ↓ exact content-addressed Designer board (§11.7)
    ↓ tier-0 retrieval → tier-1 ranking
    ↓ deterministic disposition policy   (Sage enrichment/escalation where required)
    ↓ deterministic graph operation or production (builder)
    ↓ candidate typed DAG + declarations
    ↓ production validation (cyclicity gate; V-1..V-11; lowering; VerifiedLimits; budget/correlation checks)
    ↓ staged valid graph diff
    ↓ user ratification
    ↓ authoritative designer DAG
```

### 11.2 The production verifier actively participates in design

Validation is per staged operation, and the obligations are the implemented theorem set — not a paraphrase:

- the **cyclicity admission gate** (`is_cyclic_directed`) — any cyclic graph rejected before dominance runs;
- **V-1/V-2** control-stack balance and proper nesting (with the `EndTerminate`/`Fail` exemptions — whole-instance death orphans nothing), including the per-branch dangling-fork check;
- **V-3** static pairing agreement, its input derived from post-dominance (§23) — never re-derived by a Designer-side mechanism;
- **V-4/V-5** handler validity and race shape (arms are registrations only);
- **V-6/V-7** operand-stack safety and `VerifiedLimits` (max fibres from `targets.len()`, control depth, records; collection bounds);
- **V-8** bounded flow — no backward edges;
- **V-9** dictionary purity — no control-flow side tables in the envelope; data-binding tables only;
- **V-10** `GUARD-R>` dominance over any `FORK` in its extent — refused at board level, not discovered at publish;
- **V-11** forward terminal reachability from every instruction, gateway incident edges counted;
- budget keys resolve to actual guard-opening instructions; correlation sources resolve to scalars; MI maxima present.

Candidates are built on a copy or transaction; invalid proposals never corrupt the authoritative design. No designer-only approximation of any theorem is admitted (P8, I17).

### 11.3 In-browser validation and dry run

If C1 holds (kernel/verifier/decoder on `wasm32-wasip2` with a replay-hash parity gate), the Designer runs **validate → lower → verify → dry-run** in the browser: immediate feedback on theorem violations while authoring, no server round trip, no durability requirement. The frame's canonical-bytes interface is exactly the module boundary, crossed once per transition; store, effects, scheduler, and timers stay host-side, stubbed deterministically for dry runs. This is the strongest form of P8 — the oracle travels rather than being imitated — and D11 is gated on tracing C1 plus the Q23 verifier-verdict parity gate.

### 11.4 Designer interaction: propose, stage, validate, ratify

1. The user describes a process or edit.
2. Sage explains its reading and isolates meaningful ambiguity.
3. The Utterance Engine selects a graph operation or production.
4. A deterministic builder applies it to a candidate DAG.
5. The production verifier validates the candidate (locally where C1 holds).
6. The UI shows the graph diff and diagnostics.
7. The user accepts, modifies, or rejects.
8. The accepted DAG becomes the new designer state.

```text
┌──────────────────────┬─────────────────────────────────┐
│ Sage                 │ Authoritative BPMN DAG          │
│  process intent      │  typed nodes and connectors     │
│  questions           │  guard extents, triggers,       │
│  explanations        │    budgets, cycles              │
│  design alternatives │  correlation sources            │
│                      │  incident paths (highlighted)   │
│                      │  staged diff + verifier notes   │
├──────────────────────┴─────────────────────────────────┤
│ Repl: proposed operation, bindings, validation, commit │
└────────────────────────────────────────────────────────┘
```

Selecting a node exposes: human intention; node kind; input/output contracts; adapter binding; connector outcomes including error classes; correlation source where it waits; enclosing structured extents, guard triggers, and budgets; source utterance and edit provenance; verifier status; optionally the lowered instruction fragment.

### 11.5 Terminate / Incident / limit authoring surface

- Terminate ends and incident paths are visually distinct constructs; every gateway shows its unmatched-outcome disposition (the incident edge) explicitly, with warnings where a business rejection is unrouted.
- Incident paths carry the reason taxonomy so operations can be designed for, not merely endured (Q24); the publication checklist includes "every Incident in this graph is one an operator can act on".
- Limit violations (`ResourceLimitExceeded`) are surfaced as constraints on the design — a declared maximum with its consequences — never as routable outcomes.

### 11.6 The declaration surface (P11) — authoring what the artifact seals

The Designer collects, displays, and stages the declarations the hashed artifact carries:

- **MI maxima** — mandatory per multi-instance activity; staging refused without one; the collection source and inner-activity element parameter bound alongside;
- **guard failure budgets** — per-guard `failureBudget` plus the workflow `default_guard_budget`, lowered into `v2_guard_budgets` (implemented, §31); the Designer shows the effective budget on every guard (declared or inherited), and its persistence path must retain budgets (C7 — the DTO surface drops them);
- **timer cycles** — `max_fires` per bounded cycle, non-interrupting targets only; unbounded recurrence not offered (open decision, Q26);
- **correlation sources** — per wait/publish node, an expression over declared process data, validated scalar;
- **gateway branch sets** — the visible static ceiling for concurrency (`max_fibers` derives from them);
- **retry budgets** per effect class where the adapter contract exposes them.

A dry run (§11.3) executes against the derived `VerifiedLimits`, so an author sees resource behaviour before publication.

### 11.7 The candidate board — construction contract

The board is the central safety boundary (I15), so its construction carries a contract, not just a shape (R1-7):

```text
Universe (all operations/productions/verbs the system knows)
    → context/reachability filter (position, pack, subject, structured extents)
    → policy filter (authorisation)         [ratified: BEFORE inference — disclosure minimisation]
    → canonical ordering                    [ratified: reproducibility of inference input and ties;
                                             position-INVARIANCE is separately tested, §10.7]
    = policy-safe inference board
    → tier-0 retrieval → tier-1 ranking
    → Repl policy/authorisation RECHECK     [always — the pre-filter is hygiene, never the gate]
```

```rust
struct DesignerBoard {
    artifact: ArtifactId,
    selected_node: Option<NodeId>,
    enclosing_regions: Vec<RegionDescriptor>,
    legal_operations: Vec<GraphOperation>,
    legal_productions: Vec<ProductionId>,
    available_messages: Vec<MessageKind>,
    available_effects: Vec<EffectKind>,
    available_domain_verbs: Vec<VerbId>,
    available_decisions: Vec<DecisionId>,
    declared_data: Vec<DataDeclaration>,   // what correlation/MI expressions may reference
}
```

Every `SlmResult` and every provenance record carries a **content hash of the exact board** — candidate IDs, schema versions, the descriptions as supplied to the model, canonical ordering, reachability context, active pack, and policy-filter state — not merely a version number. Two ratified sub-decisions from R1-7: policy-forbidden candidates are removed before inference (the model never sees descriptions of operations the user cannot invoke), and candidate ordering is normalised before inference. **The filter's consequence is ruled, not glossed (D19):** a natural-language request for a forbidden operation resolves as off-board/weak/out-of-scope — the language path *conceals* unavailable operations, and the rendered denial is **helpful about the path forward while generic about the request** (ratification rider, 2026-07-25): it explains that the request cannot be executed because it is not part of the user's current working context, may offer the nearest legal alternatives *from the board*, and names the governed route — a context/pack change per D20, or an access request through governance. It never confirms the requested operation exists and never enumerates what the user cannot do. `Forbidden` remains a real Repl verdict, reachable when an operation is referenced **explicitly** — DSL text, a manual board selection, a staged patch naming it — because there an explicit artifact, not a model inference, put the operation in play. Wrong-pack escalation follows D20's protocol (governed context change, new board, rerun — never silent expansion). Open: how uncertain subject resolution shapes board construction (Q29); Q30's residual is the shape of the abstention evidence.

The board changes with position: an operation valid at a sequential node may be illegal inside a race arm, across a structured join, or within a `GUARD-R>` extent (introducing a FORK the guard does not dominate is refused at board level).

### 11.8 Provenance

Every accepted element or edit retains: source utterance or manual operation; Sage hypothesis id where one existed; the **board hash**, `retrieved_subset_hash`, model-bundle hash, and `disposition_policy_hash`; the complete recorded `SlmResult` and the resulting disposition; deterministic production and parameters and the binding result; user ratification; compiler and schema versions. Audit evidence and training data — never a claim that a model authored the executable graph.

---

## 12. Graph Productions Versus Free-Form Generation

### 12.1 Graph operations

Atomic typed operations, derived from current graph context:

```text
AppendNode          InsertBefore        InsertAfter
ReplaceNode         Connect             CreateBranch
CreateRace          CreateParallelRegion  CloseParallelRegion
CreateInclusiveRegion                  CreateMultiInstanceRegion
AttachGuard         AttachRearmingGuard AttachRollbackGuard
SetGuardTrigger     SetGuardBudget      SetCorrelationSource
CallSubprocess      DeleteSubgraph
```

The three guard operations mirror the opcode trichotomy; `SetGuardTrigger` carries the arming spec (timer duration; cycle `max_fires` for non-interrupting targets). `CreateInclusiveRegion` and `CreateMultiInstanceRegion` both lower through the static-maximum skip-to-join pattern — the operation authors the branch set or the collection binding plus maximum; the lowering pattern is the compiler's, not the operation's. No operation can introduce a backward edge (I23).

### 12.2 Graph productions

Recurrent topologies as deterministic functions:

```text
REQUEST_AND_WAIT            TIMER_MESSAGE_RACE
REMINDER_THEN_ESCALATE      (GUARD-N> + bounded cycle + escalation continuation)
PARALLEL_CHECKS_AND_JOIN    INTERRUPTING_TIMEOUT
NON_INTERRUPTING_NOTIFICATION
HUMAN_REVIEW_WITH_REWORK    (forward rework topology, attempt-counted)
CALL_DURABLE_SUBPROCESS
FOR_EACH_WITH_CEILING       (MI: collection source, mandatory max, element parameter)
```

```rust
fn apply(dag: &WorkflowDag, anchor: NodeId, bindings: ProductionBindings)
    -> Result<GraphPatch, GraphBuildError>
```

The SLM ranks boarded productions and anchor candidates and supplies score evidence; **deterministic disposition policy selects** the production, anchor, and typed parameters (via the ruled slot resolvers), escalating multi-peak or compound cases to Sage. The builder owns node creation and IDs, edge creation, region formation, default connector rules (including the gateway incident edge), declaration insertion, provenance, and candidate construction. **Structural derivation — pairing, regions, merge identity — is never a Designer-side mechanism at all**: it derives from the compiler's one post-dominance computation (§23), consumed through whatever crate boundary C2's residual establishes. Designer-constructed and importer-constructed graphs share one structural oracle by construction.

### 12.3 Why not generate arbitrary graphs

Free-form model generation risks dangling nodes, invalid join topology, mismatched extents, fabricated adapter identities, missing routes, shapes that look plausible but do not lower, nondeterministic IDs, unauditable syntax — plus the self-certification failure, which no downstream check catches. Productions reduce the semantic action to a constrained choice while preserving compositional power: complex workflows are composed from valid productions, never emitted whole.

### 12.4 Whole-graph intent remains Sage's concern

Sage may reason over the whole journey and suggest a production sequence. Repl stages one coherent change or an explicitly grouped transaction, validates the complete candidate, and shows the diff.

### 12.5 Designer language as an edit log

```text
insert TIMER_MESSAGE_RACE
after IssuePassportRequest
message DocumentUploaded correlation =case_id
timer P7D
on-message ReviewPassport
on-timeout SendReminder
```

Not executable bytecode: a deterministic graph-patch constructor whose output must pass production validation before touching the authoritative DAG. Production versions are bound into the edit log so replay reconstructs the same graph after implementations change (Q5).

---

## 13. Deterministic Compiler and Execution Boundaries

### 13.1 Authority chain

```text
raw utterance + session context
    ↓ exact content-addressed board      (reachability- and policy-filtered)
    ↓ tier-0 retrieval                   non-authoritative
    ↓ tier-1 ranking                     non-authoritative
    ↓ deterministic disposition policy   (Sage enrichment/escalation where required — Sage output non-authoritative)
    ↓ typed intent / graph proposal
    ↓ SemOS reachability and schema gates
    ↓ deterministic binder and builder
    ↓ production verifier (V-1..V-11) and lowering
    ↓ user confirmation / policy / authorisation
    ↓ sealed artifact or compiled runbook
    ↓ deterministic kernel transitions (K-1..K-3, integrity rings)
```

No earlier stage bypasses a later gate.

### 13.2 Compile-time boundary

The compiler owns graph validity, typed compatibility, post-dominance structural derivation, lowering, verification, resource limits, declaration checks, artifact identity and integrity. It does not decide business truth; it accepts declared adapter and verb contracts from governed registries.

### 13.3 Runtime boundary

The kernel owns instruction semantics, fibre and stack changes, concurrency-table mutation (pending-aware, lint-enforced), deterministic effect creation, wait and content-correlation matching, cancellation and unwind in record-nesting order, incident raising, transition construction, structural invariants, and the persisted continuation. It does not parse language, infer intent, choose undeclared routes, execute JSON-defined behaviour, reinterpret the source graph, erase business rejections, or perform uncontrolled domain writes.

### 13.4 Domain boundary

Business adapters and synchronous DSL verbs own domain validation, authoritative reads and writes, relationship and state rules, entity-identity creation, and typed outcomes — invoked through deterministic effect or call boundaries with replay and idempotency contracts (deterministic creation identity: Q17).

### 13.5 Confirmation and risk

Every valid final instruction may still fail to express the user's intent. Risk controls: graph-diff ratification, runbook confirmation, two-person approval, policy evaluation, dry run, environment-specific authorisation. Compiler validity is necessary, not sufficient, for real-world authority.

---

## 14. Design Invariants

**I1 — Authoritative graph.** Exactly one authoritative typed Designer DAG per published artifact version; visuals and text are projections or edit commands.

**I2 — Stable program, mutable frame.** Instances mutate frame state, never topology. Declarations (budgets, correlation sources, maxima) are topology-side: changing one is a new artifact.

**I3 — Stable-state nodes.** Every node has a declared kind, typed bindings, bounded behaviour, and closed connector outcomes.

**I4 — Adapter-only effects.** All external and domain side effects occur through governed adapters or bounded DSL verbs.

**I5 — Typed routing with explicit incident disposition.** Route maps need not be exhaustive; every unmatched outcome resolves to a designed, visible incident path. The kernel never erases a route-map gap and never invents a destination.

**I5a — Terminate / Incident / limit separation.** Terminate (the workflow decided to stop), Incident (the workflow could not decide), and limit violation (a broken artifact/input invariant, hard and non-resumable) are three distinct constructs. None masquerades as another; only the first two are authorable paths.

**I6 — DSL semantic ownership.** Business meaning, domain validation, and authoritative mutation remain in the Business DSL and its adapters.

**I7 — BPMN temporal ownership.** Sequence, wait, race, fork, join, guard, cancellation, retry, correlation, persistence, and continuation remain in BPMN-Lite.

**I8 — Typed invocation.** Every BPMN-backed durable verb has a versioned input and result contract; dispatch fails before BPMN starts if inputs or relationships are invalid.

**I9 — No arbitrary variable bag.** Unknown invocation keys are rejected absent a declared typed extension point; the external boundary enforces key format and collection bounds. There is no per-instance artifact overlay.

**I10 — Pointer, not cargo.** Authoritative aggregates are never copied into frames; domain values are references, stable enums, explicit marked snapshots, captured observations, or bounded collections with declared semantics.

**I11 — Replay-visible observation.** Any mutable observation that affects routing is captured in transition history before it controls continuation.

**I12 — Named late binding.** Runtime-created values bind to named typed slots; nothing depends on an implicit most-recent UUID.

**I13 — Required terminal outputs.** Each terminal outcome binds exactly the outputs its result variant requires.

**I13a — Operational states never leak into contracts.** `Incidented`, quarantine, and limit errors are invisible to the DSL caller; result contracts contain declared terminal variants only.

**I14 — Non-authoritative models, distinct capabilities.** Sage may hypothesise, enrich, explain, and render clarification; the SLM ranks boarded candidates and supplies score evidence. Neither can make an unreachable verb reachable, validate an invalid graph, expand a board, authorise execution, or bypass the compiler.

**I15 — Closed, content-addressed candidate board.** The Utterance Engine's admissible choices come from SemOS or the current Designer context, policy-filtered before inference and canonically ordered; the exact board is content-hashed into every result and provenance record; model output outside the board is rejected.

**I16 — Deterministic construction; structural derivation is the compiler's.** Final ASTs, graph patches, node identities, connectors, declarations, and artifacts are constructed by deterministic code — and pairing, region, and merge-identity derivation is not reimplemented Designer-side at all: it is consumed from the compiler's post-dominance computation, because its errors are self-certifying to the verifier.

**I17 — Production validation.** The Designer invokes the production validator and compiler — the implemented theorem set, no simplified designer-only oracle. A WASM build of the same module satisfies this; a reimplementation does not.

**I18 — Transactional design edits.** Model-assisted edits apply to a candidate, are validated, displayed as a diff, and ratified before replacing authoritative state.

**I19 — Runtime handle separation.** The activation law holds: code references and static-site accounting are legitimate `Addr` uses; runtime execution identity is never an `Addr`. JOIN consumes the handle inherited from FORK. The type wall enforces it.

**I20 — Provenance.** Accepted model-assisted interpretations and graph changes retain provenance sufficient for audit, regression analysis, and training-data construction.

**I21 — Explicit uncertainty.** The Utterance Engine can return ambiguous, missing-argument, compound, and out-of-scope dispositions; it is never forced to choose.

**I22 — Safety metrics over raw accuracy.** Evaluation prioritises the accepted-and-published false-positive rate, boundary-tracked false positives, correct clarification, abstention on oracle-absent boards, compile validity, and correction rate over top-one accuracy.

**I23 — Acyclic authored topology.** No Designer operation or production introduces a backward edge; the cyclicity gate is the admission backstop, never the working mechanism. Repetition is bounded cycles, MI under a declared maximum, and forward rework topologies.

**I24 — Declared ceilings, sealed.** Every statically verified resource bound and governance ceiling originates in a Designer-visible declaration or derivable graph property, is sealed into the hashed artifact, and survives the Designer's persistence path intact (C7). Staging is refused where a mandatory declaration (MI max) is absent.

**I25 — One correlation derivation.** Correlation keys are content strings from declared process data through the single shared derivation; the Designer never introduces a second key-derivation path, and correlation sources are validated scalar at staging.

**I26 — The raw utterance is never replaced.** Tier-1 always receives the raw utterance and session context; a Sage hypothesis is an additional feature set where present, never a substitute — so an invented Sage mention cannot become an apparently established model input, and Sage is an enrichment/escalation path, never a mandatory upstream dependency.

**I27 — Models supply evidence; policy issues dispositions.** `ProposalDisposition` and all selection/abstention/clarification/escalation decisions are computed by deterministic policy over model-supplied ranking and certified-producer evidence. Entity resolution selects from boarded candidates or resolves deterministically — a model never generates identifiers or unboarded values.

**I28 — The disposition is reproducible from its record.** Every decision record closes over the board hash, retrieved-subset hash, model-bundle hash, disposition-policy hash, and context projection; scores are finite; ties break canonically. Recorded values are the historical truth; re-inference is forensic.

---

## 15. Questions Resolved Since v0.2

| v0.2 item | Resolution |
|---|---|
| Q20 (BPMN language profile) | **Resolved by ISA-002 §30.** The consolidated deviations table (D1–D8, F1–F4) is the normative profile — persistence granularity, tripwire versioning, SESE-only topology, guard-failure semantics, MI ceiling, empty-MI vs zero-match, incident rule, Ring 4 scope; deferrals: message start events, `completionCondition`, compensation, event subprocesses. The Designer surfaces this table; it does not restate it. |
| Q25 (budget declaration surface) | **Resolved by ISA-002 §31.** Per-guard artifact-resident budgets + workflow default exist; the Designer's job is authoring them, and C7 constrains the persistence path. |
| C2 | Resolved-corrected: the shared structural oracle is `compute_post_dominators` (§23); residual is the crate boundary only. |
| C6 | Resolved: landed through cutover (§30). |
| v0.2's FORK-DYN / V-3-weakening account | Corrected per ISA-002 §18's own correction: lowering pattern, static targets, V-3 unchanged. |
| v0.2's guard re-arm account | Sharpened: fire-once default; bounded `GUARD-TIMER-CYCLE>`; non-interrupting only; unbounded rejected. |
| Baseline-review directive (EOP-REV-…-001) | Its Phase 0 pin list is superseded by the cutover baseline; C1/C3/C4-residual/C5 remain its live trace items if a validation pass is still wanted before Phase D1 dispatch. |

## 16. Implementation Approach — the framework this V&S ratifies

This section is strategy, not the plan; EOP-PLAN-BPMN-DESIGN-003 decomposes it into tranches with gates, receipts, and executor tiers on ratification. The prototype objective, in Adam's framing: **the Sage/Repl-designed UI as the vehicle for prototyping an SLM inserted into the utterance → Repl discovery pipeline.** Phases D2+D3 are that prototype's core; D1 is its substrate contact; D4 is an optional accelerator.

### Phase D1 — Substrate contact: the Designer graph crate

A Designer-side crate consuming the production compiler/verifier as a library. Deliverables: the canonical typed DAG schema (Q2 closes here); the graph-operation and production sets of §12 as deterministic builders emitting `GraphPatch`es; the staged-candidate transaction; declaration authoring (§11.6) landing in the real envelope tables; provenance records. Structural derivation consumed from the compiler (C2 residual resolves here — expose or wrap `compute_post_dominators`, never reimplement).
**Gate:** for a fixture corpus spanning §30's normative topologies (including the D5–D7 three-way, V-10 refusals, cyclicity refusals, budget/correlation checks), every production-built candidate admits or rejects with verdicts identical to direct compilation of the equivalent source. Layering: the crate respects L1 and the public-API surface gate.

### Phase D2 — The Sage/Repl UI shell

The propose/stage/validate/ratify loop over D1: Sage pane, DAG surface, Repl strip; staged diffs with verifier diagnostics; Terminate/Incident/limit authoring surfaces; the declaration surface with effective-budget display; node inspection incl. lowered-fragment view; edit-log persistence that retains declarations (C7 honoured by construction).
**Gate:** a competent author can construct, stage, ratify, and publish the solicit-document workflow (§6.3) end to end with every declaration surviving round-trip, and every deliberately-invalid edit in a red-team script is refused at staging with the correct theorem named.

### Phase D3 — Utterance Engine + SLM shadow insertion (the prototype's point)

**Entry gate (R1-3): the Q9 data-governance charter, ratified before any live capture.** Deliverables: permitted and prohibited fields; redaction before persistence; retention and deletion rules; separation of evaluation, training, and audit datasets; access controls; consent/lawful-use basis; whether user corrections may enter training; model and dataset lineage; training/test contamination protection. Corpus-only work on the existing 30k utterances may proceed ahead of live capture, with the same charter applied to the corpus retrospectively before any training use.

Then: content-addressed board construction per §11.7's contract; the typed contracts of §9.2/§10.3; tier-0 wiring of the existing Candle matcher with scores logged (C5 traced here — it defines the tier-0 baseline and its oracle recall@K); routing per §10.6 — **Sage-independent on the routine path, Sage-enriched where a dialogue is active, Sage-escalated on policy decision**; shadow-mode capture per §10.7; offline fine-tune of the cross-encoder (with the `NONE_OF_THE_ABOVE` hypothesis in its training set); Candle CPU inference behind the `SlmResult` stable contract; the ruled option-(a) deterministic slot resolvers; deterministic disposition policy with calibrated thresholds and canonical tie-breaks. Any move off option (a) — token-labelling heads or a separate extractor — is a Q7 amendment with its own evaluation, not a quiet contract widening.

**Gate:** shadow-mode metrics over a held-out slice plus (charter-governed) live Designer sessions, reported per pack and per tier (§10.7's decomposition). Promotion from shadow to **Designer suggest-only** requires absolute criteria, not merely "beats tier-0" (R2-r3): minimum sample sizes with confidence bounds; per-risk-class non-regression (aggregate improvement must not conceal regression on rare or high-risk operations); maximum accepted and published false-positive rates; required abstention coverage on oracle-absent boards; minimum tier-0 recall@K; maximum latency — thresholds set in the plan, met per pack. The claim is exact: **zero direct model-authorised executions by construction** — the SLM's output remains evidence into deterministic policy, and its proposals remain proposals into Repl, throughout; promotion changes what the user sees, never what executes. Semantic false positives remain measured at proposal, ratification, publication, and execution boundaries.

**Promotion ladder (R1-8, D18):** shadow → Designer suggest-only → Designer staged-patch. **The prototype stops there.** Business DSL runbook suggestion, execution requiring explicit confirmation, and any future low-friction execution path are separate surfaces, each requiring its own corpus, threat model, and gate — D3's evidence does not transfer.

### Phase D4 — In-browser oracle (conditional)

Gated on C1 traced true and Q23's verifier-verdict parity gate designed. Ship the WASM module into the Designer; local validate/lower/verify/dry-run; server remains the admission authority.
**Gate:** parity corpus — identical verdicts and identical replay hashes, browser vs server, across the D1 fixture corpus.

### Cross-cutting discipline

The programme inherits the ISA-002 working rules unchanged: GRIND vs CAREFUL tiers with authorship-blind review at CAREFUL closes; Rule 7 halt-on-mismatch; red→green for every remediation; receipts in the plan doc; build proof over assertion; per-site rules converted to build failures where a class recurs; every code claim marked and traced before anything rests on it.

## 17. Open Questions

**Q2 — Canonical graph schema.** Node identity, region nesting, connector keys, adapter bindings, declarations, edit provenance. *Closes in Phase D1.*

**Q3 — Designer graph versioning.** Draft revisions, publication, immutable artifact versions, concurrent editing; merge strategy for two valid but conflicting patches.

**Q4 — Minimum production catalogue.** Primitive enough to compose, meaningful enough to match; §12.2's list is the candidate set, pruned/extended by D2 experience.

**Q5 — Production evolution.** Binding production versions into the edit log so replay reconstructs the same graph after implementations change.

**Q6 — Sage hypothesis contract.** The smallest typed hypothesis preserving Sage's reasoning without emulating the compiler. *Closes in Phase D3.*

**Q7 — SLM model architecture and the evidence producer.** Runtime ruled (Candle); evidence production provisionally ruled option (a) — ranker-only plus deterministic slot resolvers. Remaining: cross-encoder base selection, fine-tune regime, calibration, and whether/when a certified span-slot producer (multi-task head or separate extractor) replaces the deterministic resolvers. *Base/fine-tune/calibration close in Phase D3; producer upgrades are amendments.*

**Q8 — Confidence calibration.** Per-operation, per-risk-class thresholds; score-separation ambiguity; `NONE_OF_THE_ABOVE` calibration per pack (with an open-set/OOD head as the fallback architecture if it proves fragile across board sizes and model versions).

**Q9 — Training-data governance charter.** *Re-scoped by R1-3: a Phase D3 entry gate, not a question closed during or after D3.* Anonymisation/redaction, permitted fields, retention/deletion, dataset separation, access controls, lawful-use basis, correction-into-training policy, lineage, contamination protection.

**Q10 — Compound designer utterances.** When one utterance may stage multiple productions atomically vs sequential ratified edits.

**Q11 — Business-semantic review of structurally valid graphs.** The verifier proves structure, not sense; the §11.3 dry run is a candidate mechanism.

**Q12 — Adapter contract registry.** Describing form, message, decision, effect, and DSL adapters so the Designer exposes only legal bindings — the `v2_ffi_task_decls` pattern generalised.

**Q13 — Delivery-route timing.** Stable intent vs send-time resolution vs re-resolution on retry, as typed replay-visible policy.

**Q14 — Display snapshots.** Whether non-authoritative display values may be captured; marking, refresh, retention.

**Q15 — Binding environment reuse.** One placeholder model across the durable boundary (gated on C3).

**Q16 — BPMN return and caller recovery.** The durable record linking `InvocationId`, instance, caller continuation, result, acknowledgement; crash recovery between completion commit and DSL consumption.

**Q17 — Domain write idempotency.** Deterministic identity for BPMN-invoked domain creation so replay returns the same entity.

**Q18 — Observation semantics.** Immutable observations vs safely re-evaluable reads; policy changes and time-sensitive facts under replay.

**Q19 — User authority over staged changes.** One-confirmation edits vs explicit review or independent approval.

**Q23 — Designer/runtime verifier drift.** The gate proving the shipped WASM module is the server's admission verifier revision (candidate: extend the replay-hash parity gate with a verifier-verdict corpus). *Ratification condition for D11/Phase D4.*

**Q24 — Incident authoring taxonomy.** The reason taxonomy an incident carries and how the Designer constrains authored incident paths to operator-actionable ones.

**Q26 (new) — Unbounded recurrence.** `R/PT…` is rejected today and needs a real decision (a distinct representation plus verifier support) if ever wanted; until then the Designer does not offer it.

**Q27 (new) — Declaration authoring vs the DSL frontend.** The DSL frontend emits default budgets and has no budget/correlation authoring surface; the DTO round-trip drops budgets (C7). Decide where Designer-authored declarations canonically live (the edit log? the DAG schema?) such that XML-, DSL-, and Designer-authored artifacts converge on the same envelope tables without a lossy intermediate.

**Q28 (new) — Message start events.** Deferred at the substrate (F1). If the Designer's process catalogue needs instantiate-by-message, that is a substrate ask to raise, not a Designer workaround to build.

**Q29 (new, R1-7) — Uncertain subject resolution and the board.** When the active subject is ambiguous, is the board constructed per candidate subject, unioned with subject tags, or does subject clarification precede board construction? The answer affects both disclosure and ranking quality.

**Q30 (re-scoped) — Wrong-pack abstention evidence.** The protocol is ratified (D20: explain absence → governed context change → new board → rerun; never silent expansion). Residual: the shape of the abstention evidence — what the record carries to show *why* the board was judged not to contain the answer.

## 18. Strategic Decisions

**Ratification boundary (R1-scope):** ratification of D7–D10 and the D3 prototype does not close substrate claims (C1, C3, C4-residual, C5) or authorise runtime changes; D1/D2/D4 phases remain subject to their own traced gates, and open questions are closed where their sections say they close, not by this table.

Proposed for ratification:

| ID | Decision |
|---|---|
| D1 | Define BPMN-Lite as a specialised persisted stack-machine runtime over a compiler-validated, acyclic typed control-flow graph — per the implemented ISA-002 v0.19 baseline, whose §30 deviations table is the normative language profile. |
| D2 | Treat the Designer DAG as the primary authored artifact and the production verifier (cyclicity gate + V-1..V-11 + lowering + declaration checks) as an active design validator. No designer-only approximation. |
| D3 | Keep Business DSL semantic ownership separate from BPMN temporal ownership. |
| D4 | Bind BPMN-backed DSL calls once into typed, versioned invocation envelopes delivered as instance data and persisted in the frame. |
| D5 | Adopt pointer-not-cargo as the BPMN data rule; prohibit authoritative business-data bags; bounded collections admitted under declared maxima. |
| D6 | Use named, typed late-bound results for values created during durable execution. |
| D7 | The Utterance Engine is the formal language-to-Repl boundary; Sage is the general-language enrichment and escalation component — never a mandatory upstream stage of the routine path. |
| D8 | Permit an SLM inside the Utterance Engine only as a non-authoritative evidence supplier under §10.6's governing statement: raw utterance + content-addressed board in; ranking and span/slot evidence out; deterministic policy decides; Sage enriches and takes escalation, never gates the routine path. Candle is the ruled runtime. |
| D9 | Models rank and evidence boarded operations and productions; deterministic policy selects the disposition; deterministic builders construct candidate patches; structural derivation (pairing, regions, merges) is consumed from the compiler's post-dominance computation, never reimplemented. |
| D10 | Require staged diffs, production validation, and user or policy ratification before Designer changes become authoritative. |
| D11 | Target in-browser design-time validation and dry run via the wasm32-wasip2 build — ratification gated on C1 traced and Q23's parity gate. |
| D12 | Adopt Terminate/Incident/limit as first-class Designer distinctions: unmatched outcomes are authored incident paths; business rejections are never erased; limit violations are constraints, not routes; none appears in result contracts. |
| D13 | Make the Designer the authoring surface for the artifact's sealed declarations: MI maxima, guard budgets and the workflow default, timer-cycle bounds, correlation sources, retry budgets — with a persistence path that retains them (C7). |
| D14 | Mirror the guard trichotomy with arming triggers and budgets in Designer operations, enforcing V-10 at board level. |
| D15 | Author correlation as content-based expressions over declared process data, validated scalar at staging; never a second derivation path; no instantiate-by-message until the substrate lifts F1. |
| D16 | Adopt §16's phased framework (D1 substrate crate → D2 UI shell → D3 SLM shadow insertion → D4 conditional in-browser oracle) as the implementation approach, decomposed by EOP-PLAN-BPMN-DESIGN-003 under the inherited working discipline. |
| D17 | The Q9 data-governance charter is a Phase D3 **entry gate**: no live interaction capture before it is ratified; the existing corpus falls under the same charter before any training use. |
| D18 | Promotion is scoped by surface and risk class — shadow → Designer suggest-only → Designer staged-patch is the prototype's full extent; runbook suggestion and any execution-adjacent surface each require their own corpus, threat model, and gate. |
| D19 | Pre-inference policy filtering intentionally conceals unavailable operations from the language path: NL requests for them resolve off-board with a denial that is helpful about the path forward (reason: not part of the current working context; nearest legal alternatives from the board; the governed route) while never confirming the requested operation exists. `Forbidden` is reachable only from explicit references (DSL text, manual selection, a staged patch naming the operation). No denial-recognition model head — it would re-expose the catalogue. **Ratified with rider, 2026-07-25.** |
| D20 | Sage's off-board/wrong-pack escalation protocol: explain absence → propose a governed context/pack/subject change → rebuild a new content-addressed board → rerun inference. Never silent expansion of the board in hand. |

## 19. Closing Position

This is not "an LLM that generates BPMN," and not "a BPMN engine with a JSON variable bag."

```text
Sage
    understands the human journey
        ↓
Utterance Engine
    maps language onto a closed semantic or graph board
        ↓
Repl
    proves the proposal typed, reachable, compilable
        ↓
Designer DAG
    records the authoritative temporal topology
    and the sealed declarations the artifact carries
        ↓
Compiler and verifier
    derive structure from post-dominance, lower,
    and prove the sealed artifact (V-1..V-11)
        ↓
BPMN-Lite stack machine
    persists and advances the durable continuation (K-1..K-3)
    and raises incidents where the map has no answer
        ↓
Business DSL result
    returns typed semantic completion to the caller
```

The graph is the map of the runtime journey — acyclic by admission gate and by theorem, against an implemented substrate whose deviations from Camunda are named and normative. Its nodes are stable execution states; its connectors are typed switches whose gaps are visible incidents; its guards are a three-opcode trichotomy with declared triggers and sealed budgets; its correlation is content resolved from declared data through one derivation; its adapters are the only route to effects and bounded domain operations. The verifier validates the map — potentially in the browser, as the same module that admits artifacts in production — the stack machine traverses its lowered program, and the persisted frame carries the continuation across time and failure.

Sage remains imaginative without imagination becoming executable. The SLM improves routine matching without probabilistic inference pretending to be deterministic. Repl remains pragmatic because the final answer to "can this be done?" comes from types, reachability, graph validation, compilation, policy, and the kernel.

> **DSL owns business semantics. BPMN-Lite owns durable temporal execution. The Designer owns a validated graph and its sealed declarations. The models propose; deterministic machinery constructs, proves, and executes.**

---

## 20. Amendment v0.7 — Gameboard Turn Model, Loop Unrolling, Content Identity, Parameter Manifest

**Status:** proposed amendment to the RATIFIED v0.6 text. Ratified items below are Adam's rulings of 2026-08-11 unless marked *proposed*.
**What surfaced it:** the `codex/bpmn-gameboard-refactor` branch (103 commits) implemented a turn-based designer the v0.6 vocabulary does not describe; and two research passes (EOP-DIR-BPMN-GAMEBOARD-RESEARCH-001 and -002, both findings-only, measured) established the code facts this amendment rests on. v0.6 was found stale in the same week CLAUDE.md and `docs/sage-designer-glossary.md` were found stale — this amendment exists so the ratified layer is not the third instance of that failure.

### 20.1 The gameboard turn model supersedes the proposal vocabulary

v0.6 §9.2 and §11 describe `ProposalDisposition`, `IntentProposal`, and `GraphPatch`. The implemented model is a **turn**: a content-hashed board position, a legal-move set derived from it, a typed disposition, and a receipted attempt outcome. Where the two disagree, the turn model governs; the v0.6 types remain valid as the *shape* of the Sage↔Repl boundary but are not the implemented names.

| Concept | v0.6 name | Implemented |
|---|---|---|
| Board position | (implicit in "board") | `DesignPosition` — `state_id`, `graph_revision`, `graph_hash`, `compiler_profile`, `policy_identity`, `focus`, `history_hash`, `legal_moves`, `move_set_hash` |
| A candidate action | `IntentProposal` / `GraphPatch` | `LegalMove` — pack candidate id, anchor, arguments, binding state, `GraphDeltaPreview` |
| Disposition | `ProposalDisposition` (5) | `GameDispositionKind` (10), each with a contract-enforced shape invariant |
| Outcome | (not modelled) | `MoveAttemptOutcome` (10) + a receipt per attempt, successful or not |

Three properties of the implementation are hereby ratified as invariants, because they are stronger than what v0.6 required:

**I29 — Legal moves are compiler-proved, not heuristic.** `legal_moves::enumerate` admits a move only if it passes structural legality *and* semantic-pack admission *and* dry-runs through the production `apply_production` + `admit()` chain. A move that would fail to compile never appears; it surfaces as a typed `CompilerRefused` diagnostic. This is the one-oracle principle (P8/I17) realised at the candidate level: **the DSL cannot offer what the compiler will reject, so unsupported capabilities need no separate guard.**

**I30 — History cannot affect legality.** `move_set_hash`'s preimage provably excludes `history_hash`. Belief and motif state re-weight evidence fusion and correction branches only; the legal-move set at a position is a function of the position, never of what happened before it.

**I31 — A staged proposal gets one authority shot.** Ratification re-checks graph identity (409 on drift), re-runs the same validation path, and removes the pending proposal on success or refusal alike.

*Proposed, not ratified:* the LSP correspondence (enumerate ≡ completion, `CompilerRefused` ≡ diagnostics, `GraphDeltaPreview` ≡ item detail, productions ≡ code actions, `RequestMoveArguments` ≡ signature help) is recorded as design intent for the SME-dictation persona. No protocol surface exists today; whether to ship a real language server or a bespoke client remains open (**Q31**).

### 20.2 Loop unrolling is the ratified lowering strategy

**Finding.** The legacy `NodeAst::Loop` is `LoopAst{id, ceiling: u32, body, next, span}` — no loop condition, no collection, and `body.next` pointing back to the loop's own id: a counter-bounded sequence-flow cycle. It lowers to real bytecode (`IncCounter` + `BrCounterLt`, an unconditional backward jump), is live via `POST /api/dsl/macro/apply`, and is the sole distinctive construct of `AstMutator`. Two front-ends therefore share one kernel target under **different** cyclicity rules: `designer-graph`→`IRGraph` rejects back-edges; the S-expression DSL whitelists this one.

**Ruling.** `ceiling` is a compile-time constant and there is no loop condition, so "repeat N times unconditionally" and "N sequential copies" are the *same program*. Unrolling is therefore **exact, not approximate**, and is ratified as the lowering strategy.

**Decisive reason (D21).** The alternative — a bounded-repeat opcode with a runtime counter — would reintroduce a backward jump into a system whose entire proof structure rests on acyclicity: V-8, V-11's forward-reachability walk, the dominance-derived region map, and `VerifiedLimits` all hold *because* the admitted graph is acyclic. **A proven invariant is never spent to save a lowering pass.**

Consequences, all favourable:
- **No concurrency impact** — unrolling is sequential expansion; `max_fibers` and every barrier invariant are untouched.
- **The size check already exists** — unroll before verification and `VerifiedLimits` sees the true program; the existing limit machinery becomes *more* accurate, and no new check is required.
- **A divergence is removed, not added** — once both front-ends emit acyclic output, the `IncCounter`/`BrCounterLt` back-edge whitelist is deleted rather than maintained.

**I32 — Total unrolled size is capped, and the cap is declared.** The bound is on **total unrolled program size**, not per-loop iteration count: nested loops multiply, and a loop inside a multi-instance region multiplies again. The cap is artifact-resident and verifier-checked, on the same footing as the mandatory MI maximum (D5).

**I33 — Unrolled copies have deterministically derived identity.** Per-copy node keys are derived by a deterministic function of the loop id and iteration index. This is the identity class this codebase has already fought three times (BFS order as a proxy for nesting, for layout, and in-degree as a proxy for merge identity); it is not to be fought a fourth.

**D22 — Audit semantics, stated rather than discovered.** BPMN's standard loop is one activity executing N times; unrolled, it is N activities each executing once. The system's position is that **N distinct journalled instances is the better audit record** for a governed banking workflow — "reminder 2 of 3, sent on this date" is more auditable than one node with an opaque execution count. This is stated here so that compliance review meets it in a document rather than discovering it in a journal.

**Consequential:** `AstMutator` retirement becomes a migration rather than a design fork. Its retry macro's meaning survives as a `RepeatNTimes` production over unrolled lowering; the SME abstraction ("do this three times") is preserved while the topology stays acyclic.

**Out of scope, explicitly:** sequential multi-instance. RESEARCH-002 found it *explicitly rejected* with a named parse error, not merely absent. Parallel MI covers the motivating per-director case. Adding sequential MI is a substrate ask with its own justification, not a Designer-plan inclusion.

### 20.3 Content-derived graph identity alongside route-derived

**Finding (empirical, not inferred).** `graph_revision` and `graph_hash` both hash the session's `graph_edit_payloads()` in event order. Two edit sequences reaching a structurally identical graph (`ir_graphs_equivalent == true`) produce **different** hashes. `state_id` is therefore transitively route-derived. No canonical `IRGraph` digest exists; the canonicalisation logic to build one exists only as a pairwise comparator.

**Ruling (D23).** A content-derived graph hash is added **alongside** the route-derived one, not replacing it. Chain preview requires it: a hypothetical mid-chain state has no edit-log entry to hash.

**The semantic consequence is stated, not discovered.** Every traced consumer (ratify drift, board/workbook staleness, preview-apply drift) is a plain equality check and is mechanically indifferent. But content-derivation **loosens** those checks: edit-log churn that nets to a structural no-op currently always trips staleness and would cease to. And receipt matching changes — receipts against structurally-identical-but-differently-routed positions cannot dedup today and would begin to. Both are acceptable; both are recorded here so neither is a surprise.

**I34 — Route identity and content identity are distinct and both explicit.** Neither silently substitutes for the other. Drift and staleness checks name which they use.

### 20.4 The parameter manifest and its slot kinds

A template is not runnable until its unresolved references are supplied. The AST linter's unresolved-reference walk does not merely diagnose — **it derives the template's parameter contract**. Derived, so it cannot drift from the template it describes.

**I35 — The manifest is derived, sealed, and typed by slot kind.** Three kinds, and the distinction is load-bearing:

| Slot kind | Meaning | Supplied at dispatch? |
|---|---|---|
| **Scalar** | one value — client ref, endpoint URL, document type | Yes, one value |
| **Collection** | an input collection plus its element shape | Yes, an array (bounded; MI maximum applies) |
| **Element-scoped** | a reference *inside* an MI body, e.g. `@director.email` | **No** — declared for typing, resolved per branch from the element |

Flattening these is the failure mode to avoid: an element-scoped reference has one value *per branch*, not one per instance, and a factory that asks for it directly has asked an unanswerable question. The walk can classify correctly because it knows whether a reference sits inside an MI region.

**I36 — The manifest travels with the template.** It is sealed alongside the compiled DTO snapshot. A factory validates an invocation envelope against the manifest **before** an instance exists; a missing required slot fails at dispatch, not mid-flight.

**This does not weaken D4 or the data-not-code rule.** Resolution remains once, at dispatch, into data. There is no per-instance artifact, no AST fix-up, no overlay. The manifest describes what must be supplied; the envelope supplies it; the artifact is untouched. Per-element variation is already data — `V2MiLoadElement` loads each branch's element by value through the same pipeline every service task uses.

*Instance creation itself remains out of scope for this document* — the Designer programme ends at a published, manifest-bearing template. The manifest exists now so the factory, when built, validates against a typed contract rather than a reconstructed one.

### 20.5 Amendment register

| ID | Item | Status |
|---|---|---|
| I29 | Legal moves are compiler-proved | Ratified |
| I30 | History cannot affect legality | Ratified |
| I31 | One authority shot per staged proposal | Ratified |
| I32 | Total unrolled size capped, declared, verifier-checked | Ratified |
| I33 | Deterministic per-copy identity derivation | Ratified |
| I34 | Route and content identity distinct and explicit | Ratified |
| I35 | Manifest derived, sealed, typed by three slot kinds | Ratified |
| I36 | Manifest travels with the template; validated pre-instance | Ratified |
| D21 | Loop unrolling as lowering strategy; no backward-jump opcode | Ratified |
| D22 | Unrolled copies produce N distinct journalled instances | Ratified |
| D23 | Content hash added alongside route hash, semantics stated | Ratified |
| Q31 | LSP protocol surface: real language server, bespoke client, or both | Open |

### 20.6 Corrections to the record

Two figures this document's author supplied to RESEARCH-002's directive do not trace to the codebase: a "~2.5s embed cold-cache cliff" and a "30-node" realistic graph size. Neither exists in the repo. The measured reality is better on both counts — nothing invalidates the embed description cache (it is a process-lifetime map, cold once per description), and the HTTP round-trip is single-digit milliseconds. Recorded here as a sixth instance of the untraced-claim pattern, author-side, per the standing marking rule.

