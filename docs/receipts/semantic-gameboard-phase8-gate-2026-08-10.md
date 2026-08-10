# Semantic gameboard Phase 8 gate

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8").

Status: **YELLOW — real progress on every front, not fully green.** Unlike Phase 7
(which closed against a red receipt whose 8 items were all genuinely disposed of),
Phase 8 never had a red receipt scoping an exact, closable item list — §14's own scope
is broad (11 fuzz targets, ~15 property invariants, perf budgets, differential tests,
PostgreSQL fault tapes, resource-abuse corpora), and each tranche this session was
deliberately scoped to the highest-value gap rather than attempted exhaustively. This
receipt disposes of Gate 8's own bullet list honestly, including the bullets that stay
open — writing this as green would be exactly the kind of trap door the working
contract forbids.

**Updated same day:** bullet 6 was closed later in this session by
`docs/receipts/semantic-gameboard-phase8-coverage-audit-2026-08-10.md`; its
disposition below reflects that closure rather than the original "out of scope this
session" note.

## Disposition of every Gate 8 bullet

1. **"Every new fuzz target is discovered, independently sharded and receipted."**
   **CLOSED.** `cargo xtask fuzz list` confirms all 5 new targets
   (`clarification_policy`, `move_attempt_feedback`, `correction_history`,
   `rule_explanation_decode`, `game_turn_replay`) are auto-discovered from
   `utterance-engine/fuzz/Cargo.toml`'s `[[bin]]` entries — no manual allowlist to
   maintain. Each has its own receipt entry in
   `docs/receipts/semantic-gameboard-phase8-fuzz-target-tranche-2026-08-10.md` with
   real run evidence (7.4k-300k executions each, 0 crashes, named scenario counters
   confirmed hit).

2. **"No regression directory is empty after a finding is committed."** **CLOSED,
   vacuously.** `cargo xtask fuzz list` shows 0 regressions for all 5 new targets. Every
   crash found during this session's fuzz-target development was a harness bug (my own
   test code — a `u8` shift overflow, two wrong assumptions about
   `MoveAttemptReceipt`/`validate_attempt_history` semantics, an id-mismatch, an
   unreachable branch), fixed before any crash artifact needed to become a permanent
   regression case. No product-level finding occurred that required committing one.

3. **"P95 interactive latency meets the ratified budget on representative hardware."**
   **OPEN.** `docs/receipts/semantic-gameboard-phase8-perf-budget-2026-08-10.md` built
   the measurement harness (`utterance-engine/benches/gameboard_perf.rs`) and captured
   baseline numbers, but no budget is ratified anywhere in this repo to check against.
   Closing this bullet requires someone to ratify numbers first — not decided here.

4. **"Resource-limit failures are typed and leave the session usable."** **OPEN, not
   addressed this session.**

5. **"Expected wrong-move traffic cannot cause unbounded history, feedback recursion or
   repeated compiler work."** **PARTIALLY covered, not freshly verified.** The bound
   this bullet names already exists and is unit-tested pre-session
   (`MAX_HISTORY_ATTEMPTS`/`MAX_HISTORY_BYTES` in `utterance-engine/src/history.rs`,
   `projection_is_canonical_bounded_and_keeps_corrections`), and `correction_history.rs`
   incidentally exercises bounded history through that same code path. No new test
   specifically drove *realistic wrong-move traffic* (repeated ambiguous/incomplete/
   rejected attempts) through the disposition/recovery loop end-to-end to prove the
   *loop itself* — not just the storage bound — degrades gracefully. Left open rather
   than claimed via adjacency.

6. **"Every target has a completed receipt; semantic coverage includes every move
   kind, attempt outcome, disposition, disclosure class and correction lifecycle or
   records a reviewed unreachable justification."** **CLOSED**, per
   `docs/receipts/semantic-gameboard-phase8-coverage-audit-2026-08-10.md` (same day,
   later in this session — the audit named as missing below was in fact done). All 20
   candidate move kinds, all 10 attempt outcomes, all 10 disposition kinds, all 5
   disclosure classes, and correction lifecycle stages including self-correction
   refusal, forward-reference resolution, phantom targets, and multi-hop chains are
   constructed and exercised somewhere across the full 15-target suite, cross-checked
   against the canonical enum/registry definitions directly (not re-trusting per-target
   receipt prose). One nuance recorded, not hidden: `SystemFailure` and
   `DisclosureSafeRefusal` have no producer anywhere in this codebase today (both are
   consumer-only in `disposition.rs`/`history.rs`/`fusion.rs`/`funnel.rs`) — a reviewed
   unreachable-by-construction fact, matching the bullet's own escape clause, not a
   fuzzing gap.

7. **"PostgreSQL fault tapes, native/Wasm differential packets and resource-abuse
   corpora pass their separately receipted lanes."** **MIXED.**
   - PostgreSQL fault tapes: **CLOSED** —
     `docs/receipts/semantic-gameboard-phase8-postgres-fault-tapes-2026-08-10.md`, 3
     tests against real Postgres, one real bug found and fixed (a seq-assignment race),
     wired into `nightly-chaos.yml` as a recurring gate.
   - Native/Wasm and Python/Candle differential packets: **ruled N/A**, not "passing" —
     `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md`'s v0.10 amendment. No `wasm32` target or
     `pyo3` binding exists in this product; building either solely to satisfy this
     bullet would be new infrastructure, not a qualification pass.
   - Resource-abuse corpora: **OPEN, not addressed this session.**

8. **"Corpus minimization and regression-manifest validation run in CI without
   silently rewriting committed artifacts."** **CLOSED**, pre-existing and reconfirmed:
   `scripts/check_fuzz_regressions.py` is wired into both `nightly-fuzz.yml` and
   `production-gates.yml`; ran it fresh this session (`validated 3 governed fuzz
   regression case(s)`, exit 0) — unaffected by anything added this session, since
   nothing added a new regression case.

9. **"Public-API snapshots and compile-fail boundary tests are unchanged except for
   separately reviewed facade/contract additions."** **CLOSED.**
   `scripts/check-semantic-gameboard-boundaries.py` re-run after every single change
   this session (fuzz targets, property tests, bench, Postgres fix) — pass, all surface
   hashes unchanged throughout. No new `pub` surface was added anywhere; every new
   fuzz/property/bench target reached the gameboard model exclusively through the
   already-`pub` production facade.

## Session summary — what actually landed

- **5 fuzz targets** closing the 5 genuinely-missing targets out of 11 named
  (`docs/receipts/semantic-gameboard-phase8-fuzz-target-tranche-2026-08-10.md`).
- **6 new property tests** closing 6 of ~15 named invariants directly, with the
  remaining ~9 audited and either cited as already covered elsewhere (7) or left
  explicitly open (2: the resource-abuse/wrong-move-traffic bullet above, and
  "production and reference-model agree... not only at final state" as a universal
  claim) (`docs/receipts/semantic-gameboard-phase8-property-tests-2026-08-10.md`,
  `docs/receipts/semantic-gameboard-phase8-property-audit-2026-08-10.md`).
- **Performance measurement infrastructure** for 5 of 8 named metrics, no ratified
  budget to gate against (`docs/receipts/semantic-gameboard-phase8-perf-budget-2026-08-10.md`).
- **PostgreSQL fault-tape replay** for the designer-session store, finding and fixing a
  real concurrency bug along the way
  (`docs/receipts/semantic-gameboard-phase8-postgres-fault-tapes-2026-08-10.md`).
- Two scope rulings recorded rather than silently decided: Wasm/Python-Candle
  differential testing ruled N/A (v0.10); the concurrent-append race fix required
  explicit sign-off before touching production code (v0.14).
- **Full 15-target semantic coverage audit** closing bullet 6 same-day
  (`docs/receipts/semantic-gameboard-phase8-coverage-audit-2026-08-10.md`): all 20
  candidate move kinds, all 10 attempt outcomes, all 10 disposition kinds, all 5
  disclosure classes, and correction-lifecycle stages (self-correction refusal,
  forward-reference resolution, phantom targets, multi-hop chains) verified
  constructed across the suite against canonical enum definitions, with
  `SystemFailure`/`DisclosureSafeRefusal` named as having no producer anywhere in this
  codebase — a reviewed-unreachable fact, not a gap.

## Results (aggregate, this session)

- `cargo test -p utterance-engine --all-features`: 115 passed, 0 failed, 5 ignored.
- `cargo test -p bpmn-lite-store-postgres` (against real Postgres): 102 passed, 0
  failed.
- `cargo +nightly fuzz build` (all 15 `utterance-engine` targets): clean.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, unchanged throughout.
- `python3 scripts/check_fuzz_regressions.py`: pass, unaffected.

## Carried forward — explicit, not silently dropped

- Ratify a performance budget; close Gate 8 bullet 3 against the measurement harness
  this session built.
- Resource-limit typed-failure testing (bullet 4) and resource-abuse corpora (bullet 7)
  — genuinely untouched.
- A dedicated wrong-move-traffic / disposition-loop resource-bound test (bullet 5),
  distinct from the pre-existing storage-layer bound it currently rests on.
- Property-bullet "production and reference-model agree after every op, not only at
  final state" as a claim covering every subsystem, not just the two targets that
  currently demonstrate the methodology.

Phase 8 is not closed GREEN. It carries real, receipted progress on every front named
in §14, with every remaining gap named explicitly rather than assumed away.
