# EOP-PLAN-CRATE-HYGIENE-001 — H5 receipt

Baseline revision: `89ae3e6` (H0). Prior tranche: H4 (`3c6a357`, `20bf005`,
`9175549`, `c871faa`). This tranche's revisions: `ffbe438`, `8754831`.

- **Scope delivered:**
  1. **`designer-graph::runbook` moved into `bpmn-lite-server-designer`**
     (work item 3). `designer-graph`'s own module-boundary doc comment
     (a prior, dated "pub-scope audit, 2026-07-29") already named exactly
     5 deliberate `pub mod` submodules (`board_candidate`, `ops`,
     `positional`, `productions`, `schema`) — `runbook` was an
     undocumented 6th, confirmed to have exactly one caller anywhere in
     the workspace (`bpmn-lite-server-designer/src/rest.rs`'s
     session-runbook endpoint, `GET /api/dsl/sessions/:id/runbook`).
     `git mv`'d into `bpmn-lite-server-designer/src/runbook.rs` as a
     private module (`mod runbook;`), its 2 functions downgraded
     `pub` → `pub(crate)` per the `unreachable_pub = "deny"` ratchet, its
     own tests moved with it (updated to import
     `designer_graph::{ops, schema}` directly instead of `crate::*`,
     since it's no longer inside that crate).
  2. **`utterance-engine`'s "examples-only" candidates re-verified**
     (work items 1–2), correcting H0's original evidence. H0's sweep used
     crate-qualified-path greps (`utterance_engine::pair`, etc.), which —
     as H4.2 already found once with `Uuid` — silently misses grouped
     imports (`use utterance_engine::{foo, pair, bar};` then bare
     `pair::X` elsewhere). Re-run with bare-identifier greps
     (`\bpair::`, etc.) against the whole workspace:
     - `pair`: H0 called this zero-consumer. **Wrong** — real consumer at
       `bpmn-lite-server-designer/src/rest.rs`, missed by the
       qualified-path grep. Left `pub`, correctly.
     - `bpmn_board`, `disposition`, `exact`: all confirmed real,
       heavily-used consumers in `bpmn-lite-server-designer`
       (`proposal.rs`, `rest.rs`) plus, for `bpmn_board`, a fuzz target
       in `bpmn-lite-server-designer/fuzz`. Left `pub`.
     - `resolver_comparison`'s 11-item and `structured_choice`'s 8-item
       root re-exports: H0 called these zero-consumer "anywhere in the
       compiled workspace." Re-verified: their only real (compiled)
       consumers are `utterance-engine/examples/
       fit_phase6_structured_baseline.rs` and
       `utterance-engine/fuzz/fuzz_targets/model_boundary.rs`.
       `scripts/fixtures/gameboard_api/facade_consumer.rs` (H0's cited
       "only consumer") is confirmed, as H0's own caveat suspected, not
       part of any `Cargo.toml` — dead prose, not a real reference. Since
       the fuzz target (a genuine separate Cargo package) needs these
       items regardless of what happens to the example, **no visibility
       narrowing is actually available here** — left `pub`, correctly,
       but for a different reason than H0 assumed.
     - `contract`, `fixtures`, `trained_ranker`: same pattern —
       H0/the crate's own prior 2026-07-29 audit comment attributed their
       `pub`-ness to examples alone; re-verification found real
       `utterance-engine/fuzz/` consumers for each too
       (`history_belief_state.rs`/`evidence_fusion.rs` for `contract`,
       `phrase_index.rs` for `fixtures`, `v3_route_admission.rs` for
       `trained_ranker`), plus `contract`/`trained_ranker` have real
       `bpmn-lite-server-designer/src/rest.rs` consumers directly (not
       examples-only at all). **None of these 3 can be narrowed by a
       visibility change** — the fuzz dependency alone requires `pub`
       regardless of the examples question.
  3. **Stale doc-comment fixed**: `utterance-engine/src/lib.rs`'s own
     "pub-scope audit" comment still listed `metrics` among the modules
     "staying pub for examples" — stale since H2 already made it
     `#[cfg(test)] mod metrics` (private). Updated to drop it and to note
     the fuzz-target dependency this tranche's re-verification found for
     the other 3.
  4. **Q9/capture feature gating untouched** (work item 4, explicit
     instruction) — confirmed via `scripts/check-q9-capture-gate.sh`
     (unchanged, still passes) and via not editing `capture.rs`,
     `funnel.rs`, or the `q9-capture` feature definition at all.

- **Not attempted this tranche — a fork, not a decision:** H5 work item
  1's literal instruction ("examples that currently force library modules
  public move to xtask or a dedicated application binary") does not have
  a safe, mechanical execution path for `utterance-engine`. Every
  candidate module this instruction could apply to
  (`contract`/`fixtures`/`trained_ranker`, and the `resolver_comparison`/
  `structured_choice` re-exports) is *also* a real dependency of
  `utterance-engine/fuzz/` — a separate Cargo package that must live
  alongside the crate it fuzzes (a `cargo-fuzz` structural requirement,
  not a choice). Moving the *examples* alone would not let any of these
  modules become non-`pub`, since the fuzz targets would still need
  cross-crate access. Closing this for real would mean extracting the
  underlying fixture/corpus-generation logic into a separate crate that
  both `fuzz/` and any relocated example/xtask binary depend on instead
  of depending on `utterance-engine`'s own `lib.rs` — a genuine crate
  split of live, actively-used ML tooling (`corpus_gen.rs`,
  `eval_enrich.rs`, `score_trained_bundle.rs` are described in this
  project's own memory as dormant-but-real, re-run periodically for
  corpus-v2/retrain work, not dead scaffolding). This is a materially
  bigger architectural decision than anything else executed in H1–H5,
  touching tooling Adam actively depends on — surfaced, not decided.

- **Public API before/after (`cargo public-api -p <package> -sss`):**
  - `designer-graph`: **3 removals** — `pub mod runbook`,
    `runbook::render_operation`, `runbook::render_runbook`.
  - `bpmn-lite-server-designer`: **no diff** (`runbook` is `pub(crate)`
    there — the intended migration destination, not a new public
    surface).
  - `utterance-engine`: **no diff** (doc-comment-only change).
  - Both diffs matched exactly what was planned.

- **Removed public items and migrated consumers:** `designer_graph::
  runbook::{render_operation, render_runbook}` → their 1 real caller
  (`bpmn-lite-server-designer/src/rest.rs:3013`) now calls
  `crate::runbook::render_runbook` (the function moved into the same
  crate as its only consumer).

- **Focused tests:**
  - `cargo test -p designer-graph --lib`: 71 passed, 0 failed.
  - `cargo test -p bpmn-lite-server-designer --lib`: 93 passed, 0 failed
    (91 pre-existing + 2 moved `runbook` tests).

- **Workspace checks:**
  - `cargo check --workspace --all-targets`: clean, exit 0. Same 2
    pre-existing unrelated `bpmn-lite-server-designer` warnings as every
    prior tranche.
  - `cargo test --workspace --lib --bins`: 47/47 binaries green
    (unchanged count).
  - **Feature-gated build matrix** (H5's own required-tests item):
    `cargo check -p utterance-engine --all-targets` with default,
    `--features q9-capture`, and `--features candle-probe` — all 3
    clean, exit 0.
  - `scripts/check-q9-capture-gate.sh`: passes, unchanged from before
    this tranche.

- **Known deviations or explicitly parked work:** the utterance-engine
  fuzz/examples restructuring question above — surfaced, not decided.

### Ruling (Adam, post-receipt)

Deferred to the backlog — same disposition as H4's `ProcessInstance`/
`Fiber`/`ConcurrencyRecord` finding. Not fixed in this plan's execution,
not ruled out-of-scope either; the fuzz/examples crate-split for
`utterance-engine` is tracked as future work, separate from
EOP-PLAN-CRATE-HYGIENE-001.

- **Blind peer-review findings and dispositions:** not yet run — this
  receipt is the input to that review, not its output.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per R8 and Gate H5's own text ("developer tooling no longer defines the
production crate API. Every remaining public utterance/designer module
has a named capability, supported consumer, and crate-surface test"),
**H6 does not begin until this receipt is reviewed and accepted.**
