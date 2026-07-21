# EOP-PLAN-BPMN-ISA-002 — ISA v2 Implementation Plan (Control Stack, Concurrency Table, Integrity Rings)

**Source of truth:** EOP-VS-BPMN-ISA-002 v0.3 (FROZEN). Where this plan and the V&S disagree, the V&S wins; the disagreement is a plan defect to be corrected, never a licence to reinterpret.
**Acceptance oracle:** green `cargo build --workspace` + full test suite + tranche gates. Agent prose is not evidence.
**Substrate:** EOP-PLAN-BPMN-KERNEL-001, closed 2026-07-21 under amended scope — **verified by Tranche V0, not assumed.** T4–T10 100% complete; T11 COMPLETE (AMENDED SCOPE: store decomposition, typed errors, `TenantId` boundaries landed; performance metric suite deferred to this plan's V6.2); T12 DEFERRED-STAGED (production CI, nightly chaos, fail-closed config, demo split staged and compiling, not finalized). V0 records these deferrals as **known state, not DEVIATION**. `Claim`/`Transition`/`commit_transition`, fencing, journal, replay harness, differential harness, layering + E4 lint consumed as-is once V0's report is signed off.
**Deployment context:** greenfield. No migration code of any kind may be written. Cutover is wipe-and-recompile (V6.4).

## Executor and gate tiers

**Executor: Sonnet 5 Medium for all tranches** (consolidated from split routing; Codex 5.6 retired from this plan on token-economy grounds after KERNEL-001).

Tranches remain tiered — but by **gate strictness**, not by executor:

- **GRIND** — compiler-gated mechanical work: types, plumbing, lowering, fixtures, deletion sweeps. Gate: green build + tests + tranche checklist.
- **CAREFUL** — components that can be wrong while green: the verifier's abstract interpretation (V3), kernel word semantics + K-theorem discharge (V4), and the canonical encoding module (V2.1). Gate: green build + tests **plus** adversarial blind review of the diff, oracle/property-fuzz obligations enforced. These gates are executor-independent and survive any future model change.

V1 doubles as the executor calibration run: it is cheap and fully mechanical, so protocol adherence (honest gate reports, real burn-downs, no premature completion claims) is verified there before any load-bearing tranche begins.

## Execution protocol (both tiers)

1. Strict tranche order V1→V6; sub-tranche gates report progress % + E-invariants exercised, then → IMMEDIATELY proceed. Stopping after a green build without proceeding is a protocol violation.
2. Rip-and-replace: when a v1 construct is superseded, delete it in the same tranche. No dual paths, no compatibility shims (greenfield rule).
3. Zero suppressions: new `#[allow]` = gate failure. No `unwrap()`/`expect()` outside tests. Typed errors only in touched crates.
4. **Glossary conformance is a gate.** All new identifiers (types, fields, tables, columns, error variants, test names) must use V&S §2 terms with their bound meanings: `Instr`/cell never "token"; `Fibre` realizes token; `ControlStack`, `ConcurrencyTable`, `RecordId`, `Handle`, `Guard`, `Race`, `Barrier`, `Frame`. A CI grep-lint (V1.5) enforces the forbidden collisions (e.g. `Token` as a bytecode type, `Scope` naming the table).
5. Migrations numbered, ephemeral-PostgreSQL-tested in CI. Down paths documented; greenfield permits destructive downs.
6. Ignored-test count monotonically decreasing; any test a tranche makes runnable is un-ignored in that tranche.
7. **Substrate mismatch = halt.** If any tranche discovers the KERNEL-001 substrate does not match a plan assumption (missing API, different type shape, absent gate, unexpected behaviour), the executor HALTS and reports the mismatch verbatim. It NEVER adapts around it — no local reimplementation, no reinterpretation, no shims. Adaptive bridging of substrate gaps is the prohibited failure mode this rule exists for: a capable executor will produce a plausible bridge, and plausible bridges are how assumption drift becomes architecture drift. Every mismatch is dispositioned by Adam as either a plan amendment or a substrate fix before the tranche resumes.

## Entry conditions and external inputs

- **V&S v0.3 frozen** — satisfied.
- **EOP-EX-BPMN-ISA-002 (worked example / oracle):** REQUIRED before V4 begins; drafted during V1–V3 by the careful tier. Must contain: interrupting guard over a parallel subprocess nested inside a race with a message alternative, full dual-stack traces per transition including the cancellation cascade, **and a re-entrant FORK/JOIN** (bounded loop around a parallel block) exercising activation-handle semantics. Locked as oracle at V4 entry; V4 golden-transition fixtures are generated from it.
- **DSL lowering confirmation (V&S §9.3):** REQUIRED before V5 begins. Any residue construct is a V&S amendment — halt V5, amend, re-freeze.

## Dependency order

```
V0 (substrate reconciliation)              [GRIND — probes only, zero production code]
 └── V1 (types & tripwires)
      └── V2 (canonical encoding & integrity rings)     [GRIND, one CAREFUL module]
           ├── V3 (verifier: V-1..V-9)                  [CAREFUL]
           └── (EX oracle drafted in parallel)
                └── V4 (kernel words + K-theorems + deletion)  [CAREFUL]
                     └── V5 (frontend lowering)          [GRIND]
                          └── V6 (sweep, cutover, gates) [GRIND]
```

---

## Tranche V0 — Substrate Reconciliation — GRIND (probes only; writes no production code)

**Objective:** convert every substrate assumption this plan consumes into a machine-checked fact. Output: a reconciliation report — one row per assumption: probe executed, evidence (command + result), VERIFIED or DEVIATION. Deviations are dispositioned by Adam (plan amendment or substrate fix) before V1 begins. The executor does not fix deviations in V0; V0 is read-only against production code (test/probe additions only).

Assumptions and probes:

- [ ] 0.1 **E1 single write path.** Probe: grep + `cargo public-api` confirm no store mutation method for instance/fibre/join/job/event/incident/effect state is `pub` outside `commit_transition`; the T7 deprecated-method set is *deleted*, not deprecated. Any survivor is a DEVIATION.
- [ ] 0.2 **E2 fencing live.** Probe: re-run the T4.6 fence race test (two claimants, expiry, `StaleFence`, zero rows); confirm `CommitError` taxonomy matches the plan's expectations (Conflict/StaleFence/Integrity/Unavailable).
- [ ] 0.3 **Transition/TransitionBuilder shape.** Probe: compile a V0 test constructing a `Transition` via the builder with every field class the ISA-002 tranches will extend (fibres upsert/delete, events, effects, terminal cleanup). Field/shape mismatch vs KERNEL-001 T4.2 is a DEVIATION.
- [ ] 0.4 **Kernel purity + E4 lint.** Probe: run the layering/dependency lint script against `bpmn-lite-kernel`; confirm it exists, runs in CI, and denies tokio/sqlx/SystemTime/now_v7. Confirm `apply`'s signature matches T7.1 (`&ExecutableWorkflow, &Snapshot, &Command, &DeterministicContext`).
- [ ] 0.5 **Deterministic context & IDs.** Probe: golden-transition determinism test green (same inputs ⇒ byte-identical Transition, 10 runs); `DeterministicContext::derived_id` exists and is the sole ID source in kernel-reachable code.
- [ ] 0.6 **Durable effects (E5).** Probe: crash-replay test from T8.7 green; `EffectId` derivation function present with the `(instance, revision, ordinal)` shape; inbox unique-key idempotency demonstrated.
- [ ] 0.7 **Timers (T5).** Probe: WaitFor-resume and kill-matrix timer tests green; deterministic timer-ID derivation `(instance, fibre, pc)` present — V4's `WAIT-*` words wrap exactly this mechanism.
- [ ] 0.8 **Journal shape.** Probe: confirm journal records are written inside `commit_transition` (not after), carry `command_id`/`logical_time`/revisions/`state_hash`, and the record type admits the two Ring-2 chain columns as an additive migration. If `state_hash` semantics differ from Ring 2's frame definition, record the delta — V2.3 redefines the hash domain and must know the starting point.
- [ ] 0.9 **Canonicalization inventory.** Probe: locate T6.1's canonical-serialization code for artifacts; document exactly what it covers, its encoding choices, and whether V2.1 extends it or supersedes it. **This is the highest-value probe:** two canonical encoders with different rules is an R1 incident waiting to happen; the reconciliation report must recommend extend-vs-supersede for Adam's disposition.
- [ ] 0.10 **Verifier baseline.** Probe: T6.3 operand-stack dataflow + property tests green; confirm the CFG representation V3 will extend (edge model, abstract-state type) matches the plan's expectations.
- [ ] 0.11 **Harnesses.** Probe: T10 replay harness and T9.7 differential harness both runnable from CI entry points; run each on one existing fixture.
- [ ] 0.12 **PlanWalker extinct.** Probe: grep for `plan_hash` runtime discrimination, `PlanWalker`, `current_node_id`, dual instance tables — all must be absent (T9). Residue is a DEVIATION, not something V5 quietly absorbs.
- [ ] 0.13 **CI gates inventory.** Probe: enumerate live vs staged gates (fmt, clippy -D, ephemeral-PG migrations, layering lint, public-API surface, WASM build, ignored-test count; staged: production CI workflow, nightly chaos) and record current ignored-test count (expected 0) as the V-series baseline. The T11 perf-suite deferral and T12 DEFERRED-STAGED status are recorded as KNOWN STATE, not DEVIATION.

**Gate:** reconciliation report complete — every row VERIFIED or dispositioned; zero undispositioned DEVIATIONs; V1 entry unlocked by Adam's sign-off on the report, not by the executor's assessment.

## Tranche V1 — Types, Tripwires & Naming Substrate — GRIND

- [x] 1.1 `Addr` and `RecordId` as distinct newtypes with **no conversion in either direction** (no `From`, no accessor leaking the inner integer of one into the other). The activation law is a compile error from this point on. — `Addr` (u32 newtype, `bpmn-lite-types/src/types.rs`) vs `RecordId` (Uuid newtype, `bpmn-lite-types/src/concurrency.rs`); compile-fail doctest on `RecordId`.
- [x] 1.2 Fibre gains `control_stack: Vec<Handle>`; snapshot gains `concurrency_table: ConcurrencyTable` — records `{ id: RecordId, kind: RecordKind, members, handler: Option<Addr>, state, counters }`. `RecordKind` includes `Compensation` as an uninhabited-for-v2 variant (V&S admission requirement); retirement archives rather than deletes where kind demands history. — `bpmn-lite-types/src/concurrency.rs`.
- [x] 1.3 Tripwire versions: single accepted value per surface (`ArtifactAbi`, `SnapshotSchema`, journal schema); any other value ⇒ `IntegrityError::Tripwire`, checked pre-decode (wired fully in V2, declared here). — `bpmn-lite-types/src/integrity_rings.rs`.
- [x] 1.4 `TransitionBuilder` extended for concurrency-table mutations and control-stack deltas; builder remains the sole `Transition` constructor. — `bpmn-lite-types/src/transition.rs`.
- [x] 1.5 Glossary lint: CI script rejecting forbidden identifier collisions per §2; committed with a violations-fixture test. — `scripts/check-glossary.sh` (+ `--self-test`), `scripts/fixtures/glossary_violations.rs`, wired into `.github/workflows/layering.yml`.
- [x] 1.6 `VerifiedLimits` gains `max_control_depth`, `max_barriers`, `max_records` (populated by V3; typed now). — `bpmn-lite-types/src/artifact.rs`, zeroed until V3.

**Gate:** workspace green; a deliberate `Addr`→`RecordId` conversion attempt committed as a compile-fail test; glossary lint live in CI. — **CLOSED 2026-07-21.** `cargo build --workspace`, `cargo test --workspace` (all crates, 0 failed, 0 ignored), `cargo clippy --workspace --all-targets --all-features -D warnings`, `scripts/check-layering.sh`, and `scripts/check-glossary.sh` (+ `--self-test`) all green.

## Tranche V2 — Canonical Encoding & Integrity Rings (D3) — GRIND, except 2.1 CAREFUL

- [ ] 2.1 **[CAREFUL]** Canonical encoding module: hand-audited, dependency-pinned, BTreeMap-only, fixed field order; golden-bytes corpus committed (exact fixture bytes diffed in CI); round-trip fixed-point law `canonicalize(decode(b)) == b` as property test. This module is R1's blast zone — blind-review its diff.
- [ ] 2.2 Ring 1: frames persisted as canonical BYTEA; load path verifies hash over raw bytes **before decoding**; tripwires checked on the envelope pre-decode. JSONB introspection projection deferred pending Q3 disposition — do not build speculatively.
- [ ] 2.3 Ring 2: `state_hash = BLAKE3(canonical(snapshot ‖ fibres by fibre-ID ‖ concurrency table ‖ pending effects ‖ revision ‖ artifact_hash))`; journal records gain `prior_state_hash`/`new_state_hash`; snapshot row stores hash + producing sequence; resume performs the three-way agreement check.
- [ ] 2.4 Ring 5: `IntegrityError` variants naming the firing ring; atomic quarantine; readiness reflects; zero partial reads (Ring 3 asserts arrive in V4; Ring 4 wiring in V6).
- [ ] 2.5 Corruption-injection fixture set (V&S §9.2): per-ring damaged frames — flipped byte under hash, chain break, dangling handle, membership asymmetry, over-arity barrier, orphaned pending effect, wrong tripwire — each proving correct typed error + atomic quarantine. (Handle/membership/barrier fixtures assert Ring 1/2 detection now; Ring 3 re-covers them in V4.)
- [ ] 2.6 Sampled runtime round-trip assertion on commit (R1 mitigation c), config-gated rate.

**Gate:** every fixture fires its ring with the correct variant; golden-bytes diff green; kill-between-commit-and-resume tests show verify-before-decode rejecting damaged frames without deserializer involvement.

## Tranche V3 — Verifier: Artifact Theorems V-1..V-9 — CAREFUL

**V0 amendment (2026-07-21, probe 0.9):** T6.3's "operand-stack dataflow" does not exist in the substrate. What exists is (a) `bpmn-lite-compiler/src/verifier.rs::verify` — a structural CFG well-formedness checker (reachability, fork/join matching) over a `petgraph::DiGraph<IRNode, IREdge>`, no abstract-state/lattice type; and (b) `bpmn-lite-types/src/artifact.rs::verify_program` — a flat-bytecode referential-integrity checker, not a dataflow analysis. Neither has property/fuzz tests. **3.1 is therefore new-build, not an extension** — dual-stack abstract interpretation must be authored from scratch over the existing CFG representation, with its own property/fuzz suite (see 3.4). Scope/estimate for V3 should account for this before V3 begins.

- [ ] 3.1 Dual-stack abstract interpretation over the CFG: extend T6.3's operand-stack dataflow with the abstract control stack. Implement V-1 (balance; empty at END), V-2 (nesting/bracketing across joins of control flow), V-3 (arity agreement via static pairing annotations; runtime resolution is handle-only and *not* the verifier's concern beyond pairing), V-4 (handler entry-state validity), V-5 (race shape), V-6 (operand effects of all v2 words), V-8 (bounded flow across handler/resolution edges).
- [ ] 3.2 V-7: compute `max_control_depth`/`max_barriers`/`max_records` maxima; embed in envelope.
- [ ] 3.3 V-9 structurally: the v2 `ArtifactEnvelope` type **does not contain** race-plan/join/boundary-route tables — dictionary-only metadata plus the DSL vocabulary pin. (Compiler emission of v2 arrives in V5; verifier + envelope type land now, fixtures hand-assembled.)
- [ ] 3.4 Property/fuzz obligations: mutated programs violating each theorem rejected with typed errors, zero panics; corpus includes re-entrant FORK/JOIN (legal) and cross-path bracket violations (illegal).
- [ ] 3.5 Blind review of the abstract-interpretation core with the V&S theorem list as the review checklist — reviewer confirms each theorem maps to code, authorship-blind.

**Gate:** all V-theorem fixtures (legal admitted, illegal rejected) green; fuzz run clean; review sign-off recorded.

## Tranche V4 — Kernel Words, K-Theorems & the Deletion — CAREFUL

**Entry:** EX oracle locked.

- [ ] 4.1 Implement the D2 word set in `kernel::apply` exactly per §5 stack effects: `GUARD>`/`<GUARD`, `GUARD-N>`/`<GUARD-N`, `RACE{`/`ARM-TIMER`/`ARM-MSG`/`ARM-EFFECT`/`}RACE`, `FORK`/`JOIN` (handle-based; fresh activation per FORK execution), `WAIT-*`, `AWAIT-EFFECT`, `CANCEL-SCOPE` (terminate semantics). Effect-emitting words append to `Transition.effects`; deterministic IDs per E5; cancellation order fibre-ID, innermost-first unwind.
- [ ] 4.2 K-theorem discharge protocol: each word carries a doc-comment proof sketch of K-1 (member liveness), K-2 (stack↔membership consistency), K-3 (barrier soundness) preservation; property tests generate command sequences and assert K-1..K-3 after every `apply` — a K-violation in test is a kernel defect by definition.
- [ ] 4.3 Ring 3 shadow asserts at every park/resume: PC bounds, limit conformance, handle resolution, K-1/K-2/K-3 shadows, single-owner pending effects. Unconditional, fail-closed, O(fibres + records).
- [ ] 4.4 **The deletion:** remove race-resolution and boundary-promotion interpretation from the kernel; remove the corresponding runtime rows/tables. Kernel LOC delta recorded (net negative expected — exit metric).
- [ ] 4.5 Golden-transition fixtures generated from the EX oracle: byte-identical `Transition` per (artifact, frame, command, context) across runs; the cancellation cascade and re-entrant FORK/JOIN traces match the hand-computed oracle exactly.
- [ ] 4.6 Blind review: word semantics + proof sketches against V&S §5/§7, oracle-excluded reviewer confirms traces independently.

**Gate:** EX oracle reproduced by the kernel byte-for-byte; K property fuzz clean; deletion metric recorded; corruption fixtures from V2.5 re-fired through Ring 3 (handle/membership/barrier now caught semantically as well as physically).

## Tranche V5 — Frontend Lowering — GRIND

**Entry:** DSL lowering confirmation complete; residue = halt and amend.

- [ ] 5.1 XML compiler lowers to v2 words: gateways → branches/`FORK`/`JOIN`; boundary events → `GUARD>`/`GUARD-N>` extents; event gateways → `RACE{` arms; waits → `WAIT-*`; static pairing annotations emitted for V-3.
- [ ] 5.2 DSL frontend lowers plan constructs per §9.3 mapping (splits→`FORK`, joins→`JOIN`, callouts→`AWAIT-EFFECT`, timeouts→`RACE{`/`ARM-TIMER`); Scoped Verb Binding vocabulary version pinned into the envelope.
- [ ] 5.3 Delete v1 instruction emission from both compilers and the v1 instruction variants from types (greenfield: no translator, no dual emission).
- [ ] 5.4 Differential harness (T9.7 reuse): DSL-authored and XML-authored equivalents of each fixture workflow produce identical replay state hashes.
- [ ] 5.5 All demo/test workflows recompiled; full verifier pass over the recompiled corpus is itself a test.

**Gate:** zero v1 emission paths; differential suite green; every corpus artifact admits under V-1..V-9.

## Tranche V6 — Sweep, Cutover & Standing Gates — GRIND

- [ ] 6.1 Ring 4 wiring: nightly from-checkpoint replay fleet-wide; forensic from-genesis replay on quarantine; divergence ⇒ hard incident.
- [ ] 6.2 Re-run KERNEL-001 standing gates against v2: WASM kernel build, native/WASM replay-hash equality, codec/benchmark regression gate. **Build the deferred T11 performance metric suite here, against the final v2 frame format** (latency percentiles, allocations, resident memory, DB round-trips per transition, lock wait, scheduler lag, outbox age), plus the two v2 claims as first-class metrics: frame size ∝ live tokens, commits ∝ waits. This discharges the KERNEL-001 amended-scope deferral.
- [ ] 6.3 E6 sweep of new paths: no silent fallback values anywhere in word/lowering/integrity code.
- [ ] 6.4 Cutover: wipe persisted state, recompile corpus, cold-start readiness (migrations, recovery scan, artifact verification) green.
- [ ] 6.5 Docs: V&S marked IMPLEMENTED with deviations table (any deviation is a V&S amendment, reviewed); glossary lint promoted to blocking on all crates.

**Gate:** all standing CI gates green including nightly replay; kernel LOC net-negative confirmed; zero ignored production-path tests.

## Traceability

| V&S decision | Tranches |
|---|---|
| D1 (control stack + concurrency table, canonicity, activation law) | V1, V4 |
| D2 (word set, deferred v3 admissions) | V4, V5 |
| D3 (five rings, tripwires, R1 canonical drift) | V2, V4.3, V6.1 |
| D4 (V-1..V-9 / K-1..K-3 split) | V3, V4.2 |
| §2 glossary (binding identifiers) | V1.5, V6.5 |
| §9 inputs | EX → V4 entry; corruption fixtures → V2.5; DSL confirmation → V5 entry |
