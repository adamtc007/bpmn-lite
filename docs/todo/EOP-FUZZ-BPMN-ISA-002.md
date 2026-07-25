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

---
*v0.1 drafted 2026-07-25. Amend in place on review; lock on sign-off.*
