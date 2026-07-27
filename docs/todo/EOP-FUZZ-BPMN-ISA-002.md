# EOP-FUZZ-BPMN-ISA-002 — Fuzzing strategy & coverage plan v0.1 (FOR REVIEW)

Status: **RATIFIED v0.1 (Adam, 2026-07-25) — all five fork recommendations accepted
(F-A byte-tape generators, F-B public-surface-only, F-C per-crate layout,
F-D separate nightly-fuzz.yml, F-E engine tier deferred to P2). Implementation
proceeding F1→F4.**
Scope: cargo-fuzz across the bpmn-lite workspace, unified under `cargo xtask fuzz`,
heavily weighted toward the runtime kernel / stack machine, Postgres-independent.

---

## 1. Ground truth (surveyed at HEAD, `feat/baseline-remediation-001` @ 38f188f)

- **Existing fuzz assets:** one cargo-fuzz project at `bpmn-lite-types/fuzz/` with two
  byte-decode targets — `canonical_decode.rs` (`ConcurrencyTable::from_canonical_bytes`)
  and `canonical_decode_value.rs` (`Value::from_canonical_bytes`). Corpus evolved on
  disk but git-ignored; artifacts empty (no recorded crashes). **Zero CI wiring** —
  no fuzz in `.github/workflows/` or `scripts/`.
- **Kernel is the ideal fuzz substrate:** `bpmn-lite-kernel` depends only on
  `bpmn-lite-types`; `pub fn apply(&ExecutableWorkflow, &Snapshot, &Command,
  &DeterministicContext) -> Result<Transition, TransitionError>` (lib.rs:665) is pure,
  synchronous, deterministic, no I/O. `pub fn replay(&ExecutableWorkflow,
  &SnapshotEnvelope, &[JournalRecord]) -> Result<SnapshotEnvelope, ReplayError>`
  (lib.rs:176). `check_k_invariants` is public (lib.rs:349). Error discipline is
  `Result`-first; production panics are essentially absent (one invariant-guarded
  `.expect` at lib.rs:2307) — so **any panic under fuzz is a finding, not noise**.
- **Postgres independence is already solved, not something to build:** the kernel
  needs no store at all; the engine runs against `MemoryStore`
  (`bpmn-lite-store/src/store_memory.rs:87`) exactly as `engine/src/tests.rs` does.
  The "auto-gen mocks" requirement therefore reduces to **auto-generating inputs**
  (workflows, snapshots, command tapes), not mocking storage.
- **Admission surface:** `ArtifactEnvelope::verify(&[u8])` (artifact.rs:253) is the
  full public decode→ABI-check→canonical-re-encode→`verify_program` flow.
  `verify_program` itself is **private** (artifact.rs:323).
- **No `arbitrary` crate anywhere** in the dependency graph; all generative testing
  is `#[cfg(test)]` proptest.
- **Toolchain:** `rust-toolchain.toml` pins stable `1.95`; cargo-fuzz needs nightly.
  The existing fuzz crate has an isolated `[workspace]` but no toolchain override —
  it only works when invoked `cargo +nightly fuzz`.

## 2. Thesis — what fuzzing buys *this* system

The V&S value proposition is: **verifier admits ⇒ runtime is safe**. That implication
is exactly what coverage-guided fuzzing can attack mechanically. So the flagship
targets are not "throw bytes at parsers" (necessary but shallow); they are
**property-oracle fuzzers over the admitted-artifact space**:

> Generate a program → gate it through the real public admission path → for admitted
> artifacts, drive `kernel::apply` with an adversarial command tape → assert the
> theorems, not just "no crash."

Oracles, in priority order (the rigor is the product — crash-only fuzzing would be
the trap-door version of this):

| Oracle | Statement checked | Source of truth |
|---|---|---|
| O1 No-panic | admitted artifact + any command/tape ⇒ `apply` returns `Ok`/`Err`, never panics/aborts | libFuzzer + `debug_assertions` on (Ring 3 shadow asserts stay armed) |
| O2 K-invariants | `check_k_invariants` holds after every accepted transition | lib.rs:349 |
| O3 Replay determinism | journal produced by stepping, replayed via `replay()`, reproduces the same `state_hash` | Ring 2 |
| O4 Limits conformance | observed runtime peaks (control depth, barriers, records) **never exceed** the envelope's `VerifiedLimits` — a runtime excursion above verified bounds is a verifier soundness bug | V-7 |
| O5 Terminate-always-succeeds | `Cancel`/`Terminate` succeed even against an instance poisoned by oversized/deep `Value::Array` | documented invariant, lib.rs:671-684 |
| O6 Decode idempotence | `from_canonical_bytes(b) = Ok(x)` ⇒ `to_canonical_bytes(x)` re-decodes to `x` (and, where canonical-unique, equals the accepted prefix) | canonical.rs |
| O7 Quiescence | a non-terminal end state retains an external unblock channel — every-fibre-barrier-parked (a cross-barrier wait cycle, provably a deadlock given K-3) is a sinkhole the SESE proofs claim to exclude at admission | `check_progressable` (kernel fuzz lib); verifier-soundness class, same as O4 |

## 3. Target inventory & coverage plan

Priority P0 (flagship) → P3 (explicitly out of scope with reasons).

### P0 — kernel / stack machine (`bpmn-lite-kernel/fuzz/`)

| Target | Input shape | Oracles |
|---|---|---|
| `kernel_step` (flagship) | byte tape → **structured program generator** (§4) → `ArtifactEnvelope::from_legacy_program` admission gate (rejects are cheap, discarded) → remaining tape drives a command sequence (start, task completions in/out of order, message publishes, timer fires, cancel/terminate injections) against `apply` in a loop until quiescence or step bound | O1 O2 O4 O5 |
| `kernel_replay` | same generator; step N transitions collecting the journal, checkpoint mid-stream, `replay()` the tail | O1 O3 |
| `kernel_replay_hostile` | valid workflow + **fuzzed/truncated/reordered `JournalRecord` tail** | O1 (must `Err`, never panic) |

### P0 — admission & decode surfaces (`bpmn-lite-types/fuzz/`, extends existing)

| Target | Input shape | Oracles |
|---|---|---|
| `artifact_verify` | raw bytes → `ArtifactEnvelope::verify(&[u8])` | O1; also: `Ok` ⇒ re-serialize ⇒ `verify` again ⇒ `Ok` (admission idempotence) |
| `canonical_decode` (existing, keep) | bytes → `ConcurrencyTable::from_canonical_bytes` | O1 O6 |
| `canonical_decode_value` (existing, keep) | bytes → `Value::from_canonical_bytes` | O1 O6 |
| `canonical_decode_envelope` (new) | bytes → `SnapshotEnvelope` / `ProcessInstance` / `Fiber` decode (top-level aggregates not currently covered) | O1 O6 |

### P1 — verifier interior (structured, not raw bytes)

| Target | Input shape | Oracles |
|---|---|---|
| `verifier_admission` | byte tape → instruction-stream generator (plausible-but-hostile: valid opcodes, adversarial addresses/arities/metadata tables) → the **public** admission path | O1; coverage goal: every `ArtifactError` variant reachable in corpus |

Deliberately fuzzes through `ArtifactEnvelope`, not a specially-exposed
`verify_program` — see fork F-B (§7). This subsumes and out-guns the existing
proptest at v2_verifier.rs:1777 by adding coverage feedback.

### P2 — deferred until P0/P1 prove out (listed for the coverage map, not built now)

- **Engine command-sequence fuzz** over `BpmnLiteEngine` + `MemoryStore`: covers
  scheduler/claim/outbox/idempotency logic the kernel targets can't reach. Cost:
  async (tokio `block_on` per iteration) ⇒ ~2 orders of magnitude fewer execs/s.
  Worth having; not first.
- **Compiler frontends:** `parse_bpmn` XML (quick-xml robustness) and the DSL
  S-expression parser. Real admission surfaces, but Sage-side and lower blast
  radius than runtime.
- **dmn-lite parser**, same rationale.

### P3 — out of scope, with reasons (no silent gaps)

- `bpmn-lite-store-postgres`: I/O-bound, integration domain — `nightly-chaos.yml`
  cut-point testing is the right instrument there; libFuzzer is not.
- gRPC/HTTP/server crates: request-decode fuzzing is possible later, but tonic/axum
  do the parsing; our layer adds little attack surface beyond what P0 covers via
  the types they deserialize into.
- ffi/bus crates: revisit after P2.

## 4. Input generation strategy — the "auto-gen mocks" answer

**Fork F-A (recommendation below): byte-tape generators in the fuzz crates, not
`derive(Arbitrary)` on production types.**

- Naive `Arbitrary` on `ExecutableWorkflow`/`Snapshot` yields ~100% verifier-rejected
  garbage → the fuzzer never gets past admission and `apply` coverage stays ~0.
- Instead each fuzz crate owns a small `generator.rs` that consumes the libFuzzer
  byte tape as a decision stream and **builds structurally plausible programs via the
  same public constructors the compiler uses** (`Instr` values,
  `ArtifactMetadata` builders, `ArtifactEnvelope::from_legacy_program`). Plausible ≠
  valid: the generator intentionally emits boundary garbage (dangling addresses,
  arity-0 forks, budget 0, duplicate corr sources) at a tuned rate so both the
  reject and admit paths stay hot.
- Command tapes likewise: a byte-driven interpreter chooses among
  start/complete/publish/timer/cancel/terminate with fuzzer-controlled payloads
  (including oversized/deep arrays to exercise O5).
- Production crates stay clean: no `arbitrary` dep, no fuzz-only features, no
  cfg-gated API — nothing for the no-trap-doors rule to object to.
- Seed corpora harvested from real assets: the golden-transition fixtures (V4.5),
  compiled corpus artifacts, and the existing evolved corpora on disk.

## 5. Harness: `cargo xtask fuzz`

Extend the existing `xtask` dispatcher (no new crate). Subcommands:

- `cargo xtask fuzz list` — enumerate targets across all per-crate `fuzz/` dirs.
- `cargo xtask fuzz run [--target T] [--time SECS]` — build via `cargo +nightly fuzz`
  and run each selected target for a time budget; evolved corpus persists under the
  crate's git-ignored `fuzz/corpus/`.
- `cargo xtask fuzz smoke` — CI/pre-push mode: build **all** targets, run each
  briefly (e.g. 60 s), run the regression corpus (below), exit non-zero on any crash.
- `cargo xtask fuzz regress` — run every target over its **committed**
  `fuzz/regressions/` inputs only (seconds, deterministic). This is the
  cement-locked-test rule applied to fuzzing: every crash artifact found gets
  minimized (`cargo fuzz tmin`) and committed as a regression input alongside its
  fix; the gate then runs on stable CI time budgets forever.
- `cargo xtask fuzz clean` — delete fuzz target dirs + evolved corpora (disk
  hygiene; today's cleanup recovered 38 G, fuzz builds will regrow some of it).
- Results capture: each `run`/`smoke` writes `fuzz-results/<UTC-stamp>/summary.md`
  + per-target JSONL parsed from libFuzzer stderr (execs, execs/s, cov edges, ft,
  corpus count, crash artifact paths). `fuzz-results/` git-ignored; the summary is
  the receipt you review.
- Toolchain handling: xtask invokes `cargo +nightly fuzz …` explicitly and
  fails with a clear message if the nightly toolchain or `cargo-fuzz` binary is
  absent (no silent skip — a fuzz gate that didn't run is not a gate).

Layout (fork F-C): per-crate `fuzz/` sub-crates (`bpmn-lite-types/fuzz/` as today,
new `bpmn-lite-kernel/fuzz/`), each with isolated `[workspace]`; xtask is the
unifier. Keeps dependency hygiene (kernel fuzz deps = kernel + types only) and
matches the established convention in-repo.

## 6. CI cadence & receipts

- **New `nightly-fuzz.yml`** (fork F-D): nightly job, ~20 min/P0 target, evolved
  corpus cached via actions/cache keyed per target, crash artifacts uploaded,
  job fails red on any finding.
- **`production-gates.yml` addition:** `cargo xtask fuzz regress` (deterministic,
  seconds) so every previously-found crash is a permanent blocking gate.
- **Red→green receipts for the harness itself** (a fuzzer that has never seen red
  proves nothing): before sign-off of each P0 target, a **planted-defect run** —
  temporarily reintroduce a known-fixed bug class on a scratch branch (e.g. drop an
  array-depth check; skip a `check_k_invariants` call) and show the target finds it
  within the smoke budget. The planted-defect transcript goes in the results dir as
  the red receipt; the clean nightly run is the green.

## 7. Forks — your call before implementation

- **F-A Input generation:** byte-tape generators in fuzz crates (recommended, §4)
  vs `arbitrary` as an optional feature on `bpmn-lite-types`. Feature route is less
  code but pollutes production crates and generates mostly-rejected inputs.
- **F-B Verifier access:** fuzz admission only through public
  `ArtifactEnvelope::verify`/`from_legacy_program` (recommended — fuzz the real
  surface, no fuzz-only API holes) vs exposing `verify_program` behind
  `cfg(fuzzing)` for finer-grained interior fuzzing. Can revisit if coverage data
  shows the envelope path shadows interior branches.
- **F-C Layout:** per-crate `fuzz/` dirs unified by xtask (recommended) vs one root
  fuzz crate depending on everything.
- **F-D CI placement:** separate `nightly-fuzz.yml` (recommended) vs folding into
  `nightly-chaos.yml`. Separate keeps failure semantics distinct (chaos = Postgres
  cut-points; fuzz = kernel/admission findings).
- **F-E Engine-tier scope:** P2 as planned (recommended) vs pulling engine
  command-sequence fuzzing into the first tranche.

## 8. Phasing (each phase = commit + receipt, per working contract)

1. **F1 Plumbing:** `xtask fuzz` subcommands, nightly-toolchain guard, results
   capture, keep+seed the two existing types targets, add `artifact_verify` +
   `canonical_decode_envelope`. Receipt: `cargo xtask fuzz smoke` green transcript;
   deliberate bad-input artifact shows crash-capture path works.
2. **F2 Kernel flagship:** generator.rs + `kernel_step` with oracles O1/O2/O4/O5.
   Receipt: planted-defect red + N-million-exec green; admission-rate stat in
   summary (generator tuning target: ≥30 % of generated programs admitted).
3. **F3 Replay & verifier:** `kernel_replay`, `kernel_replay_hostile`,
   `verifier_admission`. Receipt: O3 differential red (planted journal-skip bug)
   → green.
4. **F4 CI:** `nightly-fuzz.yml` + regress gate in production-gates. Receipt: a
   real nightly run's uploaded summary.
5. **F5 (deferred, separate sign-off):** P2 targets.

## 9. Implementation status (2026-07-25)

- **F1 landed** (`493c1e3`): xtask fuzz {list,run,smoke,regress,seed,clean},
  results capture, `artifact_verify` + `envelope_decode` targets, compiled-
  artifact seed corpus. Smoke green (4 targets, ~16M execs, 0 crashes;
  seeded artifact_verify cov 3575). Red path proven with a temporary
  planted-crash target (found in 917 execs; reproducer captured; exit 1).
- **F2 landed**: `bpmn-lite-kernel/fuzz` with byte-tape generator
  (fork/join correct-by-construction + hostile arm), admission via the
  public path only, `kernel_step` flagship under O1/O2/O4/O5. Cement
  receipts: admission rate ≥30% over 1000 deterministic tapes;
  120s live run 389k execs / cov 4322 / 0 crashes.
- **FINDING F2-KERNEL-001 (fixed same day):** the O5 oracle fired on the
  harness's FIRST benign run — `Command::Cancel`/`Terminate` against any
  in-flight fork (armed record) was rejected by Ring 3 with a K-1
  violation (terminal cleanup deleted fibres but never swept the
  concurrency table): in-flight instances were un-cancellable and
  un-terminatable. Sibling of #103e (EndTerminate had the sweep; the
  command path didn't). Fixed via `retire_all_armed_records` + red→green
  cement test `terminal_commands_succeed_mid_fork_and_leave_a_k_clean_frame`.
  This is the harness's real planted-defect receipt — the defect was
  already planted.
- **F3 landed**: `kernel_replay` (O3: journal built during live stepping,
  `replay` must accept it and reproduce the final state_hash),
  `kernel_replay_hostile` (drop/dup/swap/byte-flip corruption, no-panic
  fail-closed), `verifier_admission` (generator → admission at max rate).
- **F4 landed**: `nightly-fuzz.yml` (20 min/target nightly, corpus cached
  across nights, receipts + crash artifacts uploaded) + `fuzz-regressions`
  job in production-gates (blocking; toolchain install conditioned on a
  non-empty regression set, emptiness reported explicitly).
- **Deviation flagged for review:** kernel `materialize_snapshot` made
  `pub` (one-implementation rationale, mirrors `check_k_invariants`) so
  the stepper folds transitions through the production path instead of a
  drifting local copy. Not in the ratified plan text; veto reverts to a
  harness-local copy, argued against in §4's no-trap-doors terms.
- **F5 un-deferred (Adam, 2026-07-25) and landed:** fork F-E's deferral
  rescinded by ruling. `bpmn-lite-engine/fuzz::engine_commands` drives the
  full engine over MemoryStore (compile → start → adversarial tape of
  run/complete/fail/signal/tick/cancel/inspect) under E-O1 no-panic,
  E-O2 known-good-fixtures-must-compile, and E-O5 cancel-succeeds-on-
  non-terminal (the engine-level net over F2-KERNEL-001). Toolchain note:
  nightlies 2026-07-17/24 ICE compiling tokio ≥1.53 under sancov — the
  fuzz crate pins `tokio <1.53` with the rationale in its manifest.
  Fixture gaps recorded in the target header (exclusive-gateway routing,
  boundary timers, MI XML) — next fixtures to lift from the test corpus.
- **Generator gaps filled (same ruling):** guard/GuardN/timer-armed-guard,
  race, and wait blocks are now correct-by-construction (shapes lifted
  from the verifier's admitted fixtures); corr sources key to real message
  words half the time (R3 accept branch); TimerKind::Race commands.
  Receipt: per-construct standalone-admission cement test (7 shapes) +
  kernel_step cov 4322 → 5997. Remaining: MI opcodes (need collection
  setup — covered end-to-end by F5's fixture path once MI XML lands).
- **Oracle strengthening (2026-07-25, post-F5 review — objective split:
  primary = logic/verifier-soundness flaws, secondary = gating leakage /
  implementation defects; O4/O7 serve the primary, O1/E-O2/hash-discipline
  the secondary):**
  (a) **O7 quiescence** landed in `step_workflow`: a non-terminal end
  state must keep an external unblock channel; structural over
  `WaitState` (no `apply`-probing false positives), sound given K-3.
  Red→green cement test `quiescence_check_flags_all_barrier_parked_frames_only`.
  Scope gap recorded in its doc: semantically-dead external waits need
  the P2 differential oracle.
  (b) **`Integrity` rejects are findings**: `kernel_step`'s reject arm no
  longer `continue`s on `TransitionError::Integrity` — Ring 3 fires only
  on kernel-computed frames and every harness snapshot is kernel-produced,
  so a fail-closed reject there masks a real defect. Now a panic.
  (c) **F5 clock is tape-driven** (`FuzzClock` via
  `new_with_runtime_context`): tick arms jump logical time 0..=25.5s in
  100ms grains, un-deadening the PT1S boundary-timer fire path that
  `SystemRuntimeContext` (wall clock) made unreachable inside
  microsecond execs; counter IDs replace `now_v7` for crash repro
  determinism. Kernel tier unchanged and deliberate: events injected
  directly (incl. spurious/stale/duplicate fires) is strictly stronger
  than clock simulation there.
  (d) **E-O3 XOR exclusivity** in F5: `task_a1` activating with
  `take_a != true` is a routing finding (one-sided oracle; semantics
  cemented by `t_xor_v2_merge_unequal_branch_lengths`).
  (e) Stale harness-header notes corrected (guard/race/wait ARE
  correct-by-construction; `TimerKind::Race` IS emitted).
- **F6 ratified and landed (Adam, 2026-07-25 — "the shape of the DAG is
  the source; tokens are ordered by it"):**
  `bpmn-lite-engine/fuzz::engine_graph` — tape → SESE shape grammar
  (flat composition: task / AND 2-3×1-2 / XOR guarded+empty-default →
  shared merge / boundary timer both variants / parallel MI) → real BPMN
  XML → the REAL compiler under **G-A must-admit** (a rejection of a
  grammar-legal graph is a lowering finding per the liveness thesis) →
  tape-driven token interleavings under **G-T shape-derived conservation**
  (per task, distinct observed job keys ≤ the bound the authored shape
  implies: plain/host/handler/merge 1, flag-false guarded branch 0 —
  subsuming E-O3 — MI = collection length; sound because job keys are
  `{instance}:{task_id}:{pc}:{loop_epoch}`, stable across retries and
  redeliveries) + E-O5. Closes the tier gap: kernel fuzzing generates
  shapes but bypasses the compiler; F5 runs the compiler on 5 fixed
  shapes; F6 fuzzes the shape THROUGH the compiler. Receipts: 5 cement
  tests — 100-shape deterministic must-admit population green (proves
  multi-task AND branches, both boundary variants, XOR+merge, MI all
  lower), dangling-flow red for G-A, bounds-derivation, G-T red→green
  (distinct-key duplication flagged, same-key redelivery not, off-shape
  task flagged), 25-tape benign drive clean. Recorded limits in the lib
  header: gateway nesting is the v2 widening; empty-default XOR tears can
  be dedupe-masked at the merge (two-sided catch needs a task-bearing
  default branch, pending a compiler receipt for that shape). `FuzzClock`
  + `Tape` hoisted to the engine fuzz lib, shared with F5.
- **F6 v2 nesting widening landed (`5861bae`, 2026-07-26, Adam: "deeper
  shapes are very likely where logic issues will be lurking"):**
  `Block::And.branches`/`Block::Xor.guarded` are now recursive
  `Vec<Block>` regions (MAX_DEPTH=3, BLOCK_BUDGET=24); conservation
  bounds fold multiplicatively through nesting (untaken guard zeroes its
  whole subtree). Broke the coverage plateau: 13,591 → 14,951 in 5 min.
  One constraint surfaced and RESOLVED as grammar overreach, not a
  compiler bug: Boundary inside a parallel AND branch is correctly
  rejected (handler end-event escapes the branch → join barrier never
  closes, V-1); isolated by minimal-shape probes, cemented in
  `boundary_in_parallel_branch_is_correctly_rejected`, Boundary now
  top-level-only in the grammar.
- **F7 covering-array topology corpus (ratified 2026-07-26):** Adam's
  constraint on the cartesian explosion of valid execution graphs —
  cover the LOCAL logic alphabet (typed node-pair `(node → next)` +
  switch semantics the verb allows) deterministically instead of
  sampling whole DAGs; composition explosion tamed t-wise
  (covering-array), with NESTING DEPTH as an explicit factor so the
  non-local pairing cases pure pairwise adjacency would miss stay
  covered. `covering` module: 194 enumerated canonical shapes = every
  ordered archetype adjacency (10×10) + every switch outcome (XOR
  taken/untaken; MI {0,1,4}) + every (gateway, content) depth-1 and
  (gateway, gateway′, content) depth-2 nesting. Hybrid: enumeration owns
  STRUCTURE, libFuzzer owns DYNAMICS (`fuzz seed` writes the shapes as
  tape seeds; the runtime suffix mutates). Cement receipts: coverage
  witness RECOMPUTED from the shapes via classification;
  encode↔grammar round-trip locked; full corpus compiles and steps
  clean under all oracles deterministically in CI. Recorded limits:
  Boundary adjacency/singles only; depth 3 remains the random grammar's
  territory, reached by mutation from the seeds.
- **F7b alphabet widening (2026-07-26, Adam: "ok do that"):** the three
  gaps closed — (1) task-bearing XOR default regions (both switch
  outcomes; the untaken side bounded 0 in BOTH directions — the
  two-sided tear catch the empty-default shape dedupe-masked); (2)
  Boundary under XOR via an `under_barrier` grammar flag — legality is
  barrier-ANCESTOR, not barrier-parent: Boundary-in-XOR compiles,
  Boundary-under-XOR-inside-AND and Boundary-in-OR-branch are rejected
  (both hypothesis cells CONFIRMED against the real compiler, cemented
  in the legality-matrix test); (3) OR/inclusiveGateway as a gateway
  letter (2-3 branches, per-branch activation flags, named-subset
  outcomes {both, one, none}; all-false = ruling-J zero-match → incident,
  cemented). Covering corpus regenerated for the widened alphabet: 15
  archetypes × 6 gateway letters → 748 seeds (was 194), seed writer
  pre-cleans stale `cov-*.bin`.
- **HARNESS DEFECT found by the widening (the two-sided catch went red
  on its first shape):** XOR routing was NEVER exercised — the engine's
  flag table starts empty, the start payload is opaque domain data, and
  routing flags are only writable via completion `orch_flags`
  (`flag_<u32>`, resolved through `flag_symbol_table`). The old
  start-payload flag fields were doubly dead (completions also overwrite
  `domain_payload`). Every XOR in every prior run fell to its default
  flow; invisible because G-T is upper-bound-only (0 ≤ bound always
  passes). Fix: every generated graph opens with an `init` task; the
  driver delivers the shape's full flag-intent set on every completion.
  Lower-bound cement `routing_follows_delivered_flags`: taken guard runs
  its branch and NOT the default (and vice versa), OR subsets
  {both,one,none} route exactly, zero-match raises exactly one incident.
  NOT an engine bug — engine routing is correct once flags are actually
  delivered — but it retroactively voids "XOR guarded-path dynamics
  covered" claims for earlier engine_graph runs; structural admission
  (G-A) and all other oracles were unaffected.

## §10 — F8 gap-closure batch (ratified 2026-07-26, Adam: "next batch —
I want to test the shit out of this - as far as its feasible to do")

Scope = the capability-audit not-fuzzed list plus the two gaps the F7b
work exposed (store/recovery tier, mutable-flag dynamics). Feasibility
line: everything below is Postgres-independent and in-process; true
kill-9/multi-process durability and lease-expiry recovery remain the
Postgres test suite's territory (recorded, not skipped silently).

- **F8.1 xml_compile** — arbitrary bytes at the XML frontend. X-O1
  no-panic (parse/lower/admission); X-O2 admit-honest: whatever compile
  ADMITS must start, step, and cancel clean (the fail-closed complement
  of G-A). Receipts: hostile corpus rejects without panic (incl. 4000-
  deep nesting bomb, invalid UTF-8, dangling refs); valid shapes step
  clean through the same raw-bytes driver; 17 well-formed seeds. Smoke
  2.6M execs / 2min, clean.
- **F8.2/F8.3 engine_recovery** — FaultStore wraps MemoryStore behind
  the full WorkflowStore surface (43 methods) with a tape-seeded fault
  plan: fail BEFORE the store sees the call or AFTER the durable effect
  (response lost — at-least-once hazard); tape-chosen engine RESTARTS
  (fresh transition owner, same store). R-O1 no-panic; R-O2 G-T
  conservation across faults+restarts (job keys redelivery-stable);
  R-O3 post-storm the instance is finishable or cancellable by some
  owned engine — a stuck instance is the finding. Receipts: engine
  reconstruction over a live store (previously untested ANYWHERE, per
  the store-surface probe), full-storm red receipt, 40-tape population.
  Smoke 41k execs / 2min, clean.
- **F8.4 error-boundary grammar** — Block::ErrBoundary: specific arm
  (errorCode R7 via definitions-level catalog + errorRef) or catch-all.
  Legality cells confirmed against the real compiler (top-level/in-XOR
  admit, in-AND reject — barrier-ancestor rule shared with the timer
  boundary). Runtime cement: R7 match routes to the handler and
  completes; foreign code raises an incident and parks (reject-don't-
  skip), still cancellable; catch-all catches any code. Covering
  alphabet now 17 archetypes / 818 seeds.
- **F8.5 dsl_compile** (bpmn-lite-compiler/fuzz) — hostile bytes at
  dsl::compile (lex→parse→lint→dag, demo-bindings registry), admitted
  plans continue through lower_plan to bytecode admission. D-O1
  no-panic; D-O2 gate parity: frontend-admitted plans MUST lower+verify.
  Smoke 970k execs / 90s, clean, D-O2 never fired.
- **F8.6 wire_decode** (bpmn-lite-server/fuzz) — the gRPC boundary's
  pure decode/validate units as bytes→Result (prost ProtoValue decode,
  array-limit admission, conversions, parse_*). W-O1 no-panic; W-O2
  limit parity: wire-admitted values must sit within the shared kernel
  MAX_VALUE_ARRAY_LEN/DEPTH on recomputation. Visibility-only pub
  widening in grpc.rs (R8 precedent). Smoke 1.9M execs / 90s, clean.
- **F8.7 engine_flagstorm** — inconsistent flag histories: every
  completion re-draws every routing flag, hammering split evaluation /
  guard rollback / OR-subset sync under mid-run re-routing. Structural
  oracles only (S-O1 no-panic, S-O2 shape membership, S-O3 cancel
  discipline) — intent-derived G-T is unsound under mutable flags by
  construction and stays engine_graph's tier.

Fleet after F8: 15 targets across types / kernel / engine / compiler /
server, all auto-discovered by xtask fuzz and the nightly workflow.

**First F8 soak (2026-07-26/27, 30min/target × 15): 14 clean, 1 CRASH —
F8-COMPILER-001, found and FIXED.**
- Soak numbers (execs / cov): dsl_compile 8.4M/4144; engine_commands
  878k/12202; engine_graph 321k/17587 (up from 15516 — error boundaries
  + flag routing); engine_flagstorm 290k/16086; kernel_step 2.6M/6083;
  kernel_replay 2.0M/6315; kernel_replay_hostile 2.5M/10181;
  verifier_admission 31M/2025; wire_decode 39.7M/782; canonical_decode
  1.6M/1452; canonical_decode_value 256M/104; artifact_verify 94M/4994;
  envelope_decode 84M/2468; engine_recovery clean.
- **F8-COMPILER-001** (X-O1, xml_compile, first soak): lowering PANICKED
  at the successor-address `.expect` when a parseable-but-malformed
  document (spliced double-XML mutant) produced an MI activity with no
  successor — fail-closed violation: crash instead of localized reject.
  Sweep found NINE sibling `.expect("lowering: successor has no
  assigned address")` sites (XOR edge targets, AND fork branch heads,
  task/wait/send/human-wait successors, inclusive branch heads, MI).
  All nine converted to `anyhow!` Errs naming the source and dangling
  node; `lower_inclusive_diverging_v2` became `Result`. Receipts: crash
  artifact re-runs clean and is committed as seed
  `regress-f8-compiler-001.xml`; minimal MI-no-successor cement case in
  `hostile_xml_rejects_without_panic`; compiler suite 134 green, engine
  suite green. Note: this vindicates X-O1 specifically — the parser and
  IR admitted the shape, and only the raw-bytes tier could reach it
  (the graph grammar never emits flow-less nodes).

- **F9.1 MsgWait grammar letter (2026-07-27, Adam: "ok - add it")** —
  the sleeping-token/external-event gap closed at the engine-graph tier:
  `Block::MsgWait` emits an intermediateCatchEvent + messageEventDefinition
  with a content-correlation subscription (`={key}` resolved from the
  domain payload, §28); the drive loop gains a PUBLISH action
  (`signal_with_value` with tape-chosen matching or junk keys + tick) and
  completion payloads preserve the correlation fields so waits parking
  after a completion still resolve. Covering alphabet now 18 letters /
  896 seeds; encode mirror updated (byte-7 sub-selector: Mi vs MsgWait).
  Cement `message_wait_unblocks_only_on_matching_signal`: parked on the
  wait, a NON-matching content key leaves it parked, the matching key
  wakes the downstream task through to completion — the
  external-input-unblocks-sleeping-token contract end-to-end through the
  compiler. Time recap (recorded here because Adam asked): the fuzzed
  clock is LOGICAL and tape-driven (`FuzzClock.advance(byte x 100ms)` as
  an interleaving-alphabet action straddling the PT1S due edge), so
  timer/event orderings are fuzzed deterministically and replays are
  exact.

**SURFACED FINDINGS from the F8 reconnaissance (design forks — Adam to
rule; none fixed unilaterally):**
1. **REST DSL path stops halfway across the admission seam.** Every
   bpmn-lite-server REST call site (rest.rs:261, 1170, 1826, 1943,
   2103) invokes `dsl::compile` only — the resulting
   WorkflowExecutionPlan is stored/used WITHOUT `lower_plan` →
   `verify_program` bytecode admission. `lower_plan` is invoked only in
   tests. The DSL path substitutes lint+validate_dag+SESE for the
   graph verifier (defensible, separate regimes) but the live REST flow
   never reaches the ONE shared admission gate. G4-adjacent: if
   admission is the product, a served path that skips it is a hollow
   gate. Recommend: route REST DSL compile through lower_plan+verify
   before store_plan, or record an explicit ruling why plan-tier
   admission suffices there.
2. **Non-interrupting error boundaries are silently downgraded.** The
   parser reads `cancelActivity` for every boundary event but DISCARDS
   it in the error arm (parser.rs boundary-close): a
   `cancelActivity="false"` error boundary parses fine and runs as
   interrupting. Trap-door class (silent semantic rewrite of author
   intent). Recommend: reject `cancelActivity="false"` +
   errorEventDefinition at parse with a diagnostic until
   non-interrupting error semantics are modeled.
3. **Exporter emits unresolvable errorRef.** export_bpmn.rs writes
   `errorRef="<code>"` on errorEventDefinition but never emits the
   matching top-level `<bpmn:error id=... errorCode=...>` catalog
   entry, so a re-import resolves to error_code None (catch-all) — a
   silent semantic widening on round-trip. Recommend: emit the catalog
   entry (fix is mechanical).

---
*v0.1 drafted 2026-07-25; ratified same day (all recommendations accepted);
§9 status appended during implementation; §10 F8 batch appended
2026-07-26. Amend in place; lock on F4 review.*
