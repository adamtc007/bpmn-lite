# EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 — U3 receipt

Baseline: Gate U2 accepted, `c57341f` (branch
`codex/bpmn-gameboard-refactor`). This tranche's revision: pending commit
(see below). **Tier: CAREFUL. Default disposition: defer rather than
widen visibility (plan's own text).**

- **Scope delivered:** work items 1 and 2 (benchmark) executed in full;
  work item 3 (bounded target) not taken, per the benchmark's own
  numbers; work item 4 (deferred disposition + property-test coverage)
  taken, with one honest gap flagged rather than papered over.

## Work items 1–2 — the benchmark

Added `u3_router_level_benchmark`, a `#[tokio::test] #[ignore]` function
inside `bpmn-lite-server-designer/src/rest.rs`'s own existing test module
(69 lines, `+69/-0`) — reusing the module's already-reviewed
`seed_graph_backed_session`/`post_json`/`body_json`/`get_req` helpers, no
new public or `pub(crate)` surface, no `resolve_compound_chain` /
`start_workbook` / `PendingProposal` call outside what the router already
exposes. `#[ignore]`d deliberately: this is a one-time decision-tranche
measurement, not a permanent CI-gated perf test (unlike `gameboard_perf`,
which tracks an already-shipped path's regression budget — there is
nothing shipped here to regress-guard yet).

**Harness**: memory-backed `DesignerState::try_new()` (in-memory
`MemoryStore` + `MemoryTemplateStore`, no env config, no model
environment — `candle-probe`/`embed` fields default `None` when those
features aren't compiled in, and this test doesn't compile them),
`designer_router(state)`, one fixed graph-backed session via the existing
`seed_graph_backed_session` helper (start → `review_documents` →
`end`). 200 iterations cycling three phrases from the already-reviewed
U1/U0 bounded family: `BINDABLE_UTTERANCE` (proven single-span bindable),
the existing negative-path strict-compound phrase, and an abstention
phrase — through the router's `/utterance` endpoint only, never
ratifying or rejecting.

**Results** (`cargo test -p bpmn-lite-server-designer --lib
rest::tests::u3_router_level_benchmark -- --ignored --nocapture`):

```
u3-bench: setup (DesignerState + router + seed session) = 5.007542ms
u3-bench: 200 iterations in 5.162974542s (38.7 iters/sec)
u3-bench: 3 PendingProposal entries accumulated after 200 iterations on one un-ratified/un-rejected session
```

- **Setup cost**: 5ms — cheap, not the bottleneck.
- **Throughput**: 38.7 iterations/sec, **uninstrumented** (plain `cargo
  test`, no ASAN, no SanitizerCoverage). This is the *ceiling*, not a
  realistic fuzzing rate — a real `cargo-fuzz` build adds both
  instrumentation costs on top. For comparison, U1's `history_belief_state`
  (in-process, no HTTP/JSON/router layer, but internally doing *more*
  total work per top-level call — up to 65 outcome-loop sub-steps, each
  itself calling `record_bpmn_attempt` + `project_bpmn_attempt_history` +
  `update_bpmn_design_belief` ×2 + `decide_bpmn_game_disposition` ×2)
  reached 1878 execs in **60s** (U1's receipt states "60-second bounded
  live run, 1878 executions" — the "≈31/sec" figure is this tranche's own
  arithmetic from that stated total, not a literal number U1's receipt
  itself states) **under full ASAN+coverage instrumentation**. The router
  harness's *uninstrumented* rate is already the same order of magnitude
  as the *instrumented* rate of the target it would sit on top of.
  **Everything past this point is an estimate, not a measurement**: no
  cargo-fuzz build of the router path was actually attempted, so the
  ASAN/coverage overhead ratio below is inferred by analogy, not applied
  arithmetically to 38.7. Directionally, instrumenting the router path
  would plausibly push it well below that, into low-single-digit execs/sec
  territory — cargo-fuzz-specific costs this estimate doesn't itemize
  (constructing a JSON body from raw fuzz bytes, driving a persistent
  tokio runtime from a normally-synchronous libFuzzer harness) would add
  further, unquantified overhead on top. At an estimated low-single-digit
  rate, even the existing PR-smoke pattern's `-runs=64` bound would take
  multiple seconds per call (tolerable) but the nightly 20-minute
  coverage-guided run would complete on the order of a few
  hundred to low thousands of total mutations — two to three orders of
  magnitude below what the existing gameboard targets achieve in the
  same window. Not "useful throughput" per the plan's own §U3 language.
- **Allocation growth**: confirmed real, not hypothetical — 3
  `PendingProposal` entries accumulated from 200 calls with zero
  reset mechanism available from outside `DesignerState` (the `proposals`
  field is private; only recreating the whole `DesignerState` clears it).
  A real coverage-guided fuzz run doing millions of iterations against
  one long-lived process would need either a fresh `DesignerState` per
  iteration (defeating any setup-cost amortisation) or accept unbounded
  proposal-map growth — a second, independent reason this boundary
  doesn't fit the existing hermetic-target pattern well.

## Work item 3 — bounded black-box target: not pursued

Per the benchmark above, throughput does not clear the bar. Per the
plan's own text ("If it is not fast enough, retain the existing core
target... and add focused server property tests... Record the reason as
a reviewed non-fuzzable integration boundary"), work item 4's path is
taken instead.

## Work item 4 — deferred disposition + existing property-test landscape

**Ruling: defer.** The composition boundary
(`resolve_compound_chain`/`start_workbook`) remains a reviewed
non-fuzzable integration boundary — coverage-guided fuzzing continues to
own `history_belief_state` (the deterministic gameboard core, U1) and
`preview_compilation` (materialise/dry-apply/compiler-admission), neither
of which changes. No visibility widened; the "prohibited shortcut"
(`resolve_compound_chain`, `start_workbook`, `PendingProposal`, or any
server test helper going `pub`/`pub(crate)` solely to enable cargo-fuzz)
was never taken — the benchmark itself, and every existing test cited
below, all go through the router or same-crate private-function access,
exactly as the plan requires.

**Existing coverage surveyed** (not assumed — read directly):
- The deep algorithmic invariant this whole boundary exists to protect —
  "span 2 resolves against span 1's actual hypothetical result, never the
  original position" — is already property-tested at the layer that owns
  it: `utterance-engine/src/bpmn_board.rs`'s own unit tests around
  `resolve_hypothetical_chain` (G2.1/G2.2's own closure, both a success
  case ~line 2489 and a hostile/refusal case ~line 2553 — from this
  project's own prior G2 gate, already closed and cement-locked).
- `start_workbook` has direct unit-test coverage in its own crate
  (`bpmn-lite-server-designer/src/proposal.rs`'s `mod tests`, two direct
  calls).
- `resolve_compound_chain`'s *negative* path (a genuinely ambiguous
  2-span utterance must never silently collapse to one proposal) is
  proven at the router layer:
  `test_strict_compound_utterance_never_falls_through_to_one_proposal`.
- `start_workbook`'s single-span success path, including the "router
  never mutates the graph" invariant, is proven at the router layer:
  `test_utterance_proposal_stages_without_mutating_graph`.

**Gap found while attempting the positive success-path test — and its
resolution.** No existing test — at any layer — exercised
`resolve_compound_chain`'s full **positive** 2-span success path
(`CompoundChainOutcome::Ready`). Attempting to construct one surfaced a
larger, structural finding: it isn't currently constructible at all.

Investigated directly against the shipped semantic pack
(`utterance-engine/config/bpmn-semantic-pack.yaml`, **22 capabilities —
corrected below; the first pass of this receipt undercounted and missed
a second single-required-argument capability, both caught by independent
blind review, not by this tranche's own first-pass reading**) and two
real graph shapes (plain linear; the same graph with a guard attached,
verified live via a temporary diagnostic harness before being replaced by
the permanent test below):

- `StrictCompoundSyntax::detect` (`utterance-engine/src/disposition.rs`)
  only recognises a compound span when the *entire* trimmed span exactly
  matches a governed phrase — no room for an embedded free-text argument
  (a quoted node name, a condition, etc.). Confirmed, unchanged from the
  first pass.
- **Corrected**: `WorkbookEvidence` (`bpmn-lite-server-designer/src/proposal.rs`)
  has **two** variants, `Decision` and `PaletteSelection`, not one as
  first stated. More importantly, `start_workbook` **does** perform
  real free-text argument extraction (quoted names, node/data
  references, counts, durations, boolean words) — the first pass's "it
  performs no free-text argument extraction of its own" was flatly
  wrong. The narrower, correct claim: `resolve_compound_chain` only ever
  calls `start_workbook` with a *bare governed-exact span* as the
  evidence text (`&spans[0]`, `rest.rs:3184`) — there is nothing beyond
  the phrase itself for that extraction machinery to find, for *this*
  caller specifically. `start_workbook`'s own extraction capability is
  real and exercised elsewhere (the mainline single-utterance path); it
  is simply never fed anything to extract from in the compound-chain
  path.
- Two capabilities, not one, have exactly one required argument beyond
  the position's auto-filled anchor slot: `op.delete_subgraph` (`target`)
  and `op.close_parallel_region` (`split`) — **corrected**, the first
  pass missed the second. `op.close_parallel_region` is declared
  `bpmn.binder_support: not_representable` in the pack, and
  `NotRepresentable` capabilities are filtered out before legal-move
  enumeration (`bpmn_pack.rs:388`, `bpmn_board.rs:1161`) — it can never
  appear as a legal move at all, so it doesn't change the empirical
  conclusion. `op.delete_subgraph` was never legal in either graph shape
  tried, as originally stated.
- Net effect, unchanged by the corrections above: a bare governed-exact
  compound span can never reach `ReadyForDryRun`/`ReadyForRatification`
  today, for any graph shape — `resolve_compound_chain`'s `Ready` arm is
  currently unreachable given the shipped pack. Not a code defect —
  `resolve_compound_chain`'s and `resolve_hypothetical_chain`'s own
  mechanics are separately, correctly proven (the negative-path test,
  `start_workbook`'s direct tests, and G2's own upstream tests all still
  hold).

**Ruled (Adam, this session): write the negative/structural proof
instead of continuing to chase an unconstructible positive example.**
Added `no_legal_move_is_anchor_only_bindable_today` (permanent,
`#[tokio::test]`, no `#[ignore]`) to `bpmn-lite-server-designer/src/rest.rs`'s
test module: for both graph shapes, asserts no non-abstention legal move
ever reaches `MoveBindingState::Complete` from anchor-context alone. This
pins the finding precisely and durably — if a future pack change ever
makes a capability anchor-only-bindable, this test fails, flagging that
`resolve_compound_chain`'s `Ready` arm just became reachable for the
first time and needs a real success-path test built at that point (not
before, since one wasn't constructible when this was written). The two
throwaway diagnostic harnesses used to investigate this live were
replaced by this one permanent test, not left in the tree.

**Disclosure, added after blind review (not caught in this tranche's own
first pass)**: `design_position.legal_moves[].binding_state` — the value
this test asserts on — is computed by
`utterance-engine/src/legal_moves.rs::position_bound_move`, a separate,
independently-implemented auto-fill computation from
`start_workbook`'s own argument-binding logic
(`proposal.rs::anchor_slot`) — different crate, different function. This
test therefore pins a property of `position_bound_move`'s output, not a
direct call into `start_workbook`/`resolve_compound_chain` — it would
catch a future pack change making some capability anchor-only-bindable
(the scenario its doc comment names), but it would **not**, by itself,
catch a regression introduced purely inside `start_workbook`'s own
extraction logic without a matching change to `legal_moves.rs`. Checked
this precisely rather than accepting the "they agree" claim at face
value: `anchor_slot`'s hardcoded per-candidate table and
`position_bound_move`'s generic "first required node-reference argument"
rule pick the *same argument name* for every capability except one —
`anchor_slot` has no entry for `prod.human_review_with_rework` (falls
through to `None`), while `position_bound_move` computes `anchor` for it
generically. This one naming mismatch does not change any capability's
Complete/Incomplete outcome, because `prod.human_review_with_rework` has
two required arguments (`anchor`, `max_attempts`), not one — auto-filling
`anchor` alone still leaves it Incomplete either way. So the test's
Complete/Incomplete conclusion holds for all 22 capabilities today, but
the two computations are not a perfect 1:1 mirror of each other — a real,
disclosed scope boundary of what this specific test protects, not a
direct proof about `resolve_compound_chain`'s own internals.

- **Target(s) and owner crate:** none added — this tranche is a decision,
  not a fuzz target. `u3_router_level_benchmark` (`#[ignore]`d, one-time
  measurement) and `no_legal_move_is_anchor_only_bindable_today`
  (permanent, always-run) both live in `bpmn-lite-server-designer/src/rest.rs`'s
  existing test module.

- **Public API diff:** none. `python3 scripts/check-semantic-gameboard-boundaries.py`:
  `{"status": "pass", ...}`, identical item counts/hashes to U2.
  `python3 scripts/check-test-only-pub.py`: `ok: 0 #[cfg(test)] pub item(s)`
  — neither new function carries any `pub` qualifier, consistent with the
  prohibited-shortcut constraint.

- **Focused checks:**
  - `cargo test -p bpmn-lite-server-designer --lib rest::tests::u3_router_level_benchmark -- --ignored --nocapture`:
    1 passed, real numbers captured above.
  - `cargo test -p bpmn-lite-server-designer --lib rest::tests::no_legal_move_is_anchor_only_bindable_today -- --nocapture`:
    1 passed.
  - `cargo test -p bpmn-lite-server-designer --lib`: full suite, 94
    passed, 0 failed, 1 ignored (the benchmark, correctly).
  - `cargo check --workspace --all-targets`: clean, same 2 pre-existing
    unrelated `bpmn-lite-server-designer` warnings as every prior
    tranche.
  - `python3 scripts/check-semantic-gameboard-boundaries.py`,
    `python3 scripts/check-test-only-pub.py`: both clean.

- **Known deviations or explicitly parked work:**
  - Same repo-wide `cargo fmt` drift and `bpmn-lite-engine::xml_compile`
    pre-existing compile failure documented in prior tranches — unchanged,
    unrelated, not touched.
  - The structural finding itself (no capability can currently reach
    `Ready`) is now a documented, pinned fact rather than an open gap —
    but it's worth naming as a standalone observation outside this
    fuzzing plan's scope: whether the pack should eventually gain a
    minimal anchor-only capability (making compound chains genuinely
    exercisable end-to-end) is a product/pack-authoring question this
    tranche surfaces but does not decide.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived every claim on the
  benchmark/decision half of this tranche: confirmed the diff was purely
  additive and router-only with zero visibility change, reproduced the
  benchmark three times (36.3–40.0 iters/sec, 3 `PendingProposal` entries
  every run — matching), independently searched for
  `CompoundChainOutcome::Ready` in test code repo-wide and confirmed zero
  hits at the time (the gap was real, not missed), and checked the one
  other multi-step router test in the suite
  (`test_super_user_repl_builds_6_step_2_branch_2_loop_workflow`),
  confirming it builds via sequential single-span calls and doesn't touch
  the compound path either. Reproduced all four cited verification
  commands exactly. Verdict on that half: accept-with-caveats, two of
  which were textual (disposed by editing — the "61s/30.8/sec" figure
  corrected from an implied citation to explicit self-derived arithmetic;
  the throughput extrapolation past the two measured numbers now
  explicitly labeled as estimate, not measurement) and one procedural
  (the gap needed either real work or Adam's explicit ruling to accept it
  as-is — resolved by Adam ruling to build the structural proof).

  **A second, independent review pass ran after adding
  `no_legal_move_is_anchor_only_bindable_today`**, specifically to
  re-verify the load-bearing empirical claim rather than accept the first
  pass's own reading of the pack. It found real factual errors in this
  receipt's prose, all now corrected above: the capability count (21,
  should be 22), a missed second single-required-argument capability
  (`op.close_parallel_region`, filtered from legal moves as
  `not_representable` so it doesn't change the conclusion), the
  `WorkbookEvidence` variant count (claimed 1, actually 2 —
  `Decision`/`PaletteSelection`), and a mischaracterization of
  `start_workbook` as lacking free-text extraction entirely (it has real
  extraction machinery, exercised elsewhere; `resolve_compound_chain`
  simply never feeds it anything to extract from). It also identified
  that the new test asserts on `legal_moves.rs::position_bound_move`'s
  output, a separate codepath from `start_workbook`'s own binding logic —
  disclosed above, along with the one naming mismatch found while
  verifying that disclosure precisely (`prod.human_review_with_rework`,
  inconsequential to the Complete/Incomplete conclusion since it has 2
  required arguments regardless). Reproduced the test, the full 94-test
  suite, and all workspace/gate checks independently — all passed as
  claimed. Verdict: accept-with-caveats, all caveats now disposed by the
  corrections above, not by argument.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate U3's own text: "Peer review accepts either the bounded black-box
target with evidence of useful throughput, or the explicit deferred
disposition with property-test coverage. No visibility change is
accepted as a substitute for this decision." The deferred disposition is
taken, with real benchmark evidence for why, no visibility change
anywhere, and property-test coverage now genuinely present — including a
new, permanent, durable test pinning a real structural finding discovered
in the course of trying to satisfy this exact gate, not merely a survey
of pre-existing coverage. U4 remains blocked pending a separate product
decision, unchanged by this tranche.
