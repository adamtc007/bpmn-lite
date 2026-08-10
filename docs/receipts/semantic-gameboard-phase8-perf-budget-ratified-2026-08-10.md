# Gate 8 bullet 3 — performance budget ratified and wired as a real gate

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8" bullet 3,
"P95 interactive latency meets the ratified budget on representative
hardware"). `docs/receipts/semantic-gameboard-phase8-perf-budget-2026-08-10.md`
built the measurement harness but explicitly left ratification undecided — "there
is nothing to gate against, and inventing thresholds unilaterally would be
deciding a fork that isn't mine to decide." Surfaced to Adam this session with
fresh baseline numbers; ratified.

## What Adam ratified

Presented the fresh dev-machine baseline (anchored-task fixture, 14 legal moves,
5,000 iterations):

```
legal_move_enumeration_ns=427884
full_disposition_ns=7355
belief_update_ns=9311
rule_feedback_retrieval_ns=13823
```

and three options: ratify generous-headroom defaults now, ratify tighter
owner-specified numbers, or leave open pending real production/staging
hardware. Adam chose generous-headroom defaults:

- Legal move enumeration: **5ms** (~12x headroom over the ~428us baseline).
- Full disposition, belief update, rule/feedback retrieval: **1ms** each
  (~70-140x headroom over their ~7-14us baselines).

Framed explicitly as P95 ceilings, not means — the multiplier absorbs both
real-hardware variance and tail behavior a 5,000-iteration same-machine mean
does not capture, without being loose enough to miss an actual regression (an
order-of-magnitude-plus slowdown in any of these four operations is a real
defect, not noise).

## Wiring — a gate that runs, not a documented number

Per the working contract ("the gate that doesn't run is not a gate"), a ratified
number sitting in a receipt is not a gate. Wired concretely:

- `utterance-engine/benches/gameboard_perf.rs`: added `BUDGET_ENUMERATION_NS`,
  `BUDGET_DISPOSITION_NS`, `BUDGET_BELIEF_UPDATE_NS`,
  `BUDGET_RULE_FEEDBACK_RETRIEVAL_NS` constants and a real `assert!` after each
  of the four measurements. The harness's doc comment previously said
  assertions exist "only on machine-independent shape claims... never on raw
  nanosecond latency, since no performance budget is ratified" — updated to
  match reality now that one is.
- `.github/workflows/production-gates.yml`: added a
  `cargo bench -p utterance-engine --bench gameboard_perf` step, immediately
  after the sibling `v2_perf` step this bench's own doc comment already claimed
  to match the convention of. Before this change the bench existed but nothing
  in CI ran it — an unwired gate is not a gate, regardless of what its
  assertions say.

## Red-green verification

Before restoring the real budget, set `BUDGET_ENUMERATION_NS` to `1` (from
`5_000_000`) and reran the bench: failed exactly as expected —
`legal move enumeration regressed: 400211ns/iter exceeds the ratified 1ns
budget` — proving the assertion actually fires rather than being a dead check.
Restored the ratified value; reran clean.

## Results

- `cargo bench -p utterance-engine --bench gameboard_perf`: clean at the ratified
  budgets (measured this session: enumeration ~397-428us, disposition ~7us,
  belief update ~9us, retrieval ~13-14us — all comfortably under budget).
- Red-green check above: confirms the assertion is live, not decorative.
- `cargo test -p utterance-engine --all-features`: all passing, 0 failed.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, unchanged
  (bench/CI changes carry no public library surface).
- `python3 -c "import yaml; yaml.safe_load(...)"` on `production-gates.yml`:
  valid.

## Scope note

This closes Gate 8 bullet 3 for the four metrics this harness measures
(enumeration, disposition, belief update, rule/feedback retrieval). It does not
add preview-compilation or learned-lane (Candle) latency budgets — both were
already named out of scope in the harness's original receipt for reasons
unrelated to ratification (realistic fixture cost, network-dependent model),
and remain so. "Representative hardware" in the bullet's own wording is a real,
not-yet-satisfied caveat: these numbers are ratified against one development
machine's CI runner, not staging/production hardware — acceptable per Adam's
explicit choice of generous headroom over a tighter, hardware-specific number,
but worth naming rather than silently treating "ratified" as "measured on
production."
