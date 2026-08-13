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

**Gap found, flagged rather than rushed**: no existing test — at any
layer — exercises `resolve_compound_chain`'s full **positive** 2-span
success path (`CompoundChainOutcome::Ready`): span 1 detected, resolved,
materialised via `start_workbook`, `resolve_hypothetical_chain` succeeds,
span 2 resolves against the resulting hypothetical position, and a bound
`ResolvedChain` with both moves comes back. The existing negative test
proves ambiguity is refused; nothing proves the success path actually
completes end-to-end. Constructing a genuinely resolvable 2-span
compound utterance requires phrase-corpus-level knowledge (governed exact
match against two distinct, sequentially-legal candidates) I did not have
verified confidence in within this tranche's scope, and I chose not to
hand-roll a test whose pass/fail might hinge on an unverified assumption
about phrase resolution — that would risk exactly the "verify, don't
infer" failure this session has caught in itself twice already (U0's two
self-corrections). **This is a real, named gap in "focused server
property tests," not a closed item** — flagged for Adam's ruling at the
STOP-gate below: invest in constructing this test now (a genuine chunk of
work, needing semantic-pack phrase-corpus research), or accept the
existing G2-layer + negative-path + single-span-success coverage as
sufficient given the composition boundary's disposition is driven by
*throughput*, not by an unverified logic gap.

- **Target(s) and owner crate:** none added — this tranche is a decision,
  not a target. The benchmark harness itself (`u3_router_level_benchmark`)
  is `#[ignore]`d and not part of the regular test suite.

- **Public API diff:** none. `python3 scripts/check-semantic-gameboard-boundaries.py`:
  `{"status": "pass", ...}`, identical to U2. `python3 scripts/check-test-only-pub.py`:
  `ok: 0 #[cfg(test)] pub item(s)` — the new benchmark function carries no
  `pub` qualifier at all (private to the test module), consistent with
  the prohibited-shortcut constraint.

- **Focused checks:**
  - `cargo test -p bpmn-lite-server-designer --lib rest::tests::u3_router_level_benchmark -- --ignored --nocapture`:
    1 passed, real numbers captured above.
  - `cargo check --workspace --all-targets`: clean, same 2 pre-existing
    unrelated `bpmn-lite-server-designer` warnings as every prior
    tranche.
  - `python3 scripts/check-semantic-gameboard-boundaries.py`,
    `python3 scripts/check-test-only-pub.py`: both clean.

- **Known deviations or explicitly parked work:**
  - The positive-2-span-success property-test gap named above — the one
    open item at this gate.
  - Same repo-wide `cargo fmt` drift and `bpmn-lite-engine::xml_compile`
    pre-existing compile failure documented in prior tranches — unchanged,
    unrelated, not touched.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived every claim: confirmed the
  69-line diff is purely additive and router-only with zero visibility
  change, reproduced the benchmark three times (36.3–40.0 iters/sec, 3
  `PendingProposal` entries every run — matching), independently searched
  for `CompoundChainOutcome::Ready` in test code repo-wide and confirmed
  zero hits (the flagged gap is real, not missed), checked the one other
  multi-step router test in the suite
  (`test_super_user_repl_builds_6_step_2_branch_2_loop_workflow`) and
  confirmed it builds via sequential single-span calls, not the
  `;`-delimited compound path — so it doesn't close the gap either.
  Reproduced all four cited verification commands exactly. Verdict:
  accept-with-caveats.
  - **Finding 1, disposed**: the "61s (≈30.8/sec)" framing read as a
    literal citation from U1's receipt when it was this tranche's own
    arithmetic from U1's stated "60-second... 1878 executions." Corrected
    above to state this explicitly.
  - **Finding 2, disposed**: everything past the two measured numbers
    (38.7/sec uninstrumented, U1's cited 60s/1878-execs instrumented) was
    stated in the same declarative tone as the measurements themselves,
    without marking it as estimated. Corrected above — the estimate
    section is now explicitly labeled as inference, not derived
    arithmetic, and cargo-fuzz-specific costs the estimate doesn't
    itemize (JSON-from-bytes construction, driving tokio from a
    normally-synchronous libFuzzer harness) are now named as additional,
    unquantified overhead.
  - **Finding 3, not disposed by editing — requires Adam's ruling**: the
    reviewer's central point is procedural, not textual — per Gate U3's
    own text, the deferred disposition is conditioned on "property-test
    coverage" being *present*, and work item 4 says to *add* focused
    property tests, not merely survey existing coverage. This receipt
    audits and discloses a real gap rather than closing it. The
    reviewer's own assessment: this is the *correct* procedural move
    under this project's "surface forks, don't decide them" and "verify,
    don't infer" discipline (fabricating an unverified resolvable 2-span
    phrase would repeat a mistake this session already self-corrected
    twice in U0) — but it does mean **U3 is not actually closed on its
    own terms yet**, only its benchmark/decision half is. See the
    STOP-gate line below, which reflects this rather than papering over
    it.

- **STOP-gate decision: blocked — awaiting peer review of this receipt,
  and Adam's ruling on the flagged property-test gap.**

Per Gate U3's own text: "Peer review accepts either the bounded black-box
target with evidence of useful throughput, or the explicit deferred
disposition with property-test coverage. No visibility change is
accepted as a substitute for this decision." The deferred disposition is
taken, with real benchmark evidence for why, no visibility change
anywhere, and a fully honest accounting of what property-test coverage
exists versus what's still missing — the missing piece is named, not
hidden. U4 remains blocked pending a separate product decision, unchanged
by this tranche.
