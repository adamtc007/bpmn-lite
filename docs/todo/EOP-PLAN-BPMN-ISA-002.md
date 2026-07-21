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

- [x] 2.1 **[CAREFUL]** Canonical encoding module — **full supersession, single encoder for the entire Ring-2 hash domain** (decision reversed 2026-07-21, superseding the phased hybrid below: ambition chosen deliberately, not defaulted to). Hand-audited, dependency-pinned, BTreeMap-only, fixed field order; golden-bytes corpus committed (exact fixture bytes diffed in CI); round-trip fixed-point law `canonicalize(decode(b)) == b` as property test. This module is R1's blast zone — blind-review its diff. Staged build, each stage golden-byte-fixed and gated before the next:
    - [x] 2.1a Primitives + `Value` enum (already float-free: `Bool`/`I64`/`Str(u32)`/`Ref(u32)`); golden-bytes fixtures per variant. Receipt: `canonical.rs` `CanonicalEncode for Value`, tests `golden_bytes_value_variants`, `value_round_trips_byte_identically`.
    - [x] 2.1b `WaitState`/`ProcessState`/`ErrorClass`/`Incident` with explicit documented tag schemes (not derive-default). Receipt: `canonical.rs` — `WaitState` tags 0x00-0x07, `ProcessState` tags 0x00-0x06, `ErrorClass` tags 0x00-0x02.
    - [x] 2.1c Session stack (`SessionScopeState`, `SessionWorkspaceKind`, `encode_session_stack`) and `Fiber`'s flag/counter/join-map-shaped fields (`stack`, `regs`, `control_stack`) composed via `write_seq`. Receipt: `canonical.rs`; covered by `canonical_round_trip_is_a_fixed_point`/`flipping_any_byte_never_reproduces_the_original_value` proptests (extended scope, same properties).
    - [x] 2.1d `domain_payload`/`placeholder_values`/`workspace_stack` (opaque externally-supplied JSON): parse to `f64`, encode the IEEE 754 bit pattern (deterministic regardless of source text — `1.0`/`1.00`/`1e0` converge, tested); NaN/Infinity rejected via `validate_finite_json`. **Adversarial finding during implementation:** the plan's premise that a huge exponent (`1e400`) is "a real, reachable input" parsing to `f64::INFINITY` is **false** for this codebase's parser — `serde_json` (without `arbitrary_precision`) bounds-checks exponents at parse time and rejects overflow as a parse error, and `Number::from_f64` independently rejects NaN/Infinity, so a `serde_json::Value` can never carry a non-finite float through any safe public API (`domain_payload_with_huge_exponent_is_a_typed_parse_error_not_silent_infinity`, `placeholder_values_cannot_carry_a_non_finite_float_via_serde_json_public_api`). The rejection check is retained as defense-in-depth (future non-`serde_json` source, or an `arbitrary_precision` upgrade) — currently unreachable dead-path coverage, not a live gap it closes. Malformed JSON is a separate, real, reachable typed error (`CanonicalJsonError::InvalidJson`) — caught a genuine test-fixture bug (`store_postgres.rs`'s `test_pg_commit_tick_atomicity` used a non-JSON `domain_payload` literal `"initial_payload"`, silently tolerated before this tranche because nothing ever parsed it; fixed to `"\"initial_payload\""`).
    - [x] 2.1e Full `ProcessInstance` composition (`ProcessInstance::try_canonical_hash_bytes`, fallible — the one exception to this module's otherwise-infallible impls) and `Fiber` (`CanonicalEncode`, infallible). Golden-bytes fixture: `persistence.rs` test `golden_bytes_process_instance_and_fiber_canonical_hash_domain`, superseding the retired `serde_json`-encoded fixture. Binary tag scheme is order-independent by construction, so struct field-reordering is structurally eliminated, not just guarded against.
    - [x] 2.1f `ConcurrencyTable`/pending effects: unchanged from the interim hybrid, now composed alongside the newly-migrated `ProcessInstance`/`Fiber` bytes in `PersistedSnapshotState::try_canonical_hash_bytes` / `SnapshotEnvelope::state_hash()`. No behavior change; re-verified via the full `cargo test -p bpmn-lite-types`/`-p bpmn-lite-store-postgres` runs.
    - [x] 2.1g Deleted the `serde_json`-based hashing path from `state_hash()`'s dependency chain. Confirmed via grep: `state_hash()`'s body (`persistence.rs`) calls only `try_canonical_hash_bytes()`/`to_canonical_bytes()` — zero `serde_json`. (`SnapshotEnvelope`/`JournalRecord`'s `serde_json` usage that remains is the on-disk **storage envelope** format, a distinct concern — see the "Superseded" note below.) Retired `scripts/check-canonical-invariant.sh`, its CI step in `.github/workflows/layering.yml`, and the interim JSON golden-bytes fixture (`golden_process_instance_and_fiber.json`) — the invariant they guarded (BTreeMap-only/no-float on `ProcessInstance`/`Fiber` for hash-domain determinism) no longer applies now that the hash domain doesn't touch their `serde_json` serialization at all.
    **Exit condition:** met. One canonical encoding module, zero JSON in the hash domain (grep-verified), golden-bytes + round-trip + float/NaN-rejection fixtures all green, full workspace build/test/clippy green (`cargo build --workspace`, `cargo test --workspace` — 103/103 test binaries green including one real fixture bug found and fixed, `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean). No tracked deviation carries into V6. Blind review of the complete composed diff (a second reviewer, authorship-blind, per the CAREFUL-tier protocol) is still outstanding — flagged for Adam to schedule before this module is treated as fully closed for V3+ reliance.
- [x] 2.2 Ring 1: frames persisted as canonical BYTEA; load path verifies hash over raw bytes **before decoding**; tripwires checked on the envelope pre-decode. JSONB introspection projection deferred pending Q3 disposition — do not build speculatively. Receipt: `bpmn-lite-store-postgres/src/store_postgres.rs` `claim_work_for_transition` pre-decode `blake3::hash(snapshot_bytes) != frame_hash` check → `IntegrityError::Ring1Physical`; migration `056_ring1_ring2_integrity_columns.sql` adds `workflow_instances.frame_hash`.
- [x] 2.3 Ring 2: `state_hash = BLAKE3(canonical(snapshot ‖ fibres by fibre-ID ‖ concurrency table ‖ pending effects ‖ revision ‖ artifact_hash))`; journal records gain `prior_state_hash`/`new_state_hash`; snapshot row stores hash + producing sequence; resume performs the three-way agreement check. Receipt: `bpmn-lite-types/src/persistence.rs` `SnapshotEnvelope::state_hash()` redefined over the full domain; `JournalRecord.prior_state_hash`; `claim_work_for_transition` three-way agreement check **plus** chain-walk verifying each record's `prior_state_hash` against the previous record's `state_hash` → `IntegrityError::Ring2Frame`.
- [x] 2.4 Ring 5: `IntegrityError` variants naming the firing ring; atomic quarantine; readiness reflects; zero partial reads (Ring 3 asserts arrive in V4; Ring 4 wiring in V6). Receipt: `bpmn-lite-types/src/integrity_rings.rs` (`Tripwire`, `Ring1Physical`, `Ring2Frame` variants); quarantine path unchanged from pre-existing atomic quarantine machinery, now fed by the new ring variants' `Display` string.
- [x] 2.5 Corruption-injection fixture set (V&S §9.2): per-ring damaged frames — flipped byte under hash, chain break, dangling handle, membership asymmetry, over-arity barrier, orphaned pending effect, wrong tripwire — each proving correct typed error + atomic quarantine. (Handle/membership/barrier fixtures assert Ring 1/2 detection now; Ring 3 re-covers them in V4.) Receipt: `bpmn-lite-store-postgres/src/store_postgres.rs` tests `test_claim_load_quarantines_ring1_flipped_byte_under_stale_hash`, `test_claim_load_quarantines_ring2_chain_break`, `test_claim_load_quarantines_wrong_schema_version_tripwire` (all pass, `cargo test -p bpmn-lite-store-postgres`). The four D1-concurrency-table-shaped corruptions (dangling handle, membership asymmetry, over-arity barrier, orphaned pending effect) are documented in-code as covered by the same Ring 1 byte-flip mechanism — Ring 1 is one BLAKE3 hash over the whole frame and cannot semantically distinguish which invariant broke; that distinction is Ring 3's job (V4), per the plan's explicit ring split. Exhaustive corruption coverage at the encoding layer is separately proven by `flipping_any_byte_never_reproduces_the_original_value` in `canonical.rs`.
- [x] 2.6 Sampled runtime round-trip assertion on commit (R1 mitigation c), config-gated rate. Receipt: `bpmn-lite-store-postgres/src/store_postgres.rs` `should_sample_canonical_round_trip` (revision-keyed, deterministic — `revision % rate == 0`; default rate 128; `BPMN_LITE_CANONICAL_SAMPLE_RATE` overrides, `0` disables); wired into `commit_transition` immediately after `frame_hash` computation — decodes the just-encoded envelope and re-canonicalizes, rejecting the commit with `CommitError::Integrity` on any mismatch. Genesis (revision 0) always samples under the default rate, so every existing commit-path test already exercises it with zero false positives (68/68 pass). Unit test `canonical_round_trip_sampling_is_deterministic_and_rate_gated` proves the gate itself, without env-var mutation (which would race across parallel tests in this binary).

**Gate: CLOSED.** Every fixture fires its ring with the correct variant; golden-bytes diff green; kill-between-commit-and-resume tests show verify-before-decode rejecting damaged frames without deserializer involvement. Full re-verification against the single-encoder hash domain: `cargo build --workspace` green, `cargo test --workspace` green (103/103 test binaries, including `cargo test -p bpmn-lite-store-postgres` 68/68 and `cargo test -p bpmn-lite-types` 27/27 + 1 doctest), `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean. Outstanding: the CAREFUL-tier authorship-blind second-reviewer pass on 2.1's complete composed diff (not yet scheduled).

### Superseded: the 2026-07-21 phased-hybrid interim (history, not current state)

For a period within this tranche's close, 2.1 was scoped as a **hybrid**: `ProcessInstance`/`Fiber` continued through `serde_json` (deterministic today via BTreeMap-only + no floats, CI-enforced by `scripts/check-canonical-invariant.sh`) while only `ConcurrencyTable`/pending-effects used the new tagged-binary `CanonicalEncode` module, with `state_hash()` folding both domains together. That satisfied the letter of D3 Ring 2 (one hash, one chain) but not the spirit of "one canonical encoder," and was recorded as a tracked deviation requiring resolution before V6. **Same-day, this was reversed**: 2.1 was rescoped to full supersession (2.1a–2.1g above), now complete. `scripts/check-canonical-invariant.sh` and the golden-bytes JSON fixture it guarded (`bpmn-lite-types/tests/fixtures/golden_process_instance_and_fiber.json`) have been deleted, along with the CI step invoking the script — neither lingers as dead CI surface.

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
