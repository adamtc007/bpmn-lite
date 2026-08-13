# EOP-PLAN-CRATE-HYGIENE-001 — H0 receipt

Baseline revision: `89ae3e6` (branch `codex/bpmn-gameboard-refactor`).

- **Scope delivered:** H0 work items 1–4 in full. Baseline/evidence only — zero
  production code or manifest changes, per H0's "Production changes: forbidden."

- **Files/packages changed:** none in `src/**` or `Cargo.toml`. New evidence artifacts
  only:
  - `docs/generated/public-api-baselines/*.txt` (32 files, one per library package) +
    `BASELINE_REVISION.txt`
  - `docs/generated/h0-public-surface-inventory.md`
  - `docs/generated/h0-test-topology-inventory.md`
  - this receipt

- **Public API before/after (`cargo public-api`):** N/A for this tranche — H0 records
  the *baseline*, there is no "before." Command used: `cargo public-api -p <package>
  -sss`, exactly matching R7 and the existing `check-semantic-gameboard-boundaries.py`
  CI gate. Verified reproducible: regenerated 3 representative crates
  (bpmn-lite-types, designer-graph, dsl-manifest) from a clean detached-HEAD git
  worktree at `89ae3e6` and diffed byte-for-byte against the committed baseline —
  identical. Worktree removed after verification.

- **Removed public items and migrated consumers:** none — H0 makes no removals.

- **Added public items and capability justification:** none.

- **Test classification changes:** none executed — classification is recorded as
  evidence (`docs/generated/h0-test-topology-inventory.md`), no test file was moved.

- **Focused tests:** N/A — no code changed.

- **Workspace checks:** N/A — no code changed. (Full `cargo check --workspace
  --all-targets` was not re-run in this tranche since no source file was touched; the
  last-known-green baseline is `89ae3e6`, the same revision the public-api baselines
  were generated from.)

- **Known deviations or explicitly parked work:**
  - The public-item counts in plan §1.2 were independently re-derived via grep for 7/7
    priority crates: 5 crates match exactly (bpmn-lite-types, dmn-lite-types,
    designer-graph fields, bpmn-lite-authoring, bpmn-lite-compiler fields), the
    remainder are within single-digit deltas attributable to multi-line signatures —
    the evidence table in §1.2 is trustworthy.
  - Two factual corrections already applied to the plan doc itself in the prior
    peer-review pass (before H0 began): `bpmn_lite_vm::compute_hash` and
    `build_demo_plan`/`demo_initial_vars` both have real production consumers, not "no
    consumer" as originally drafted. H2's work items already reflect the fix.
  - **New finding, not in the plan's original evidence**: `bpmn-lite-engine/src/tests.rs`
    imports `bpmn_lite_authoring` via a dev-dependency, directly contradicting the
    crate's own documented "does NOT depend on `bpmn-lite-authoring`" Phase-0 boundary.
    Recommend adding to the H1 migration list.
  - **New finding**: `designer-graph` has an undocumented 6th public module
    (`runbook`) not covered by its own "5 deliberate pub-mod" audit comment; single
    call site in `bpmn-lite-server-designer`.
  - **New finding**: `utterance-engine` has 19 combined root-re-exported items
    (`resolver_comparison::*`, `structured_choice::*`) plus `fixtures`/`funnel`/`pair`
    with zero consumers anywhere in the compiled workspace — their only "consumer" is
    an uncompiled fixture file not wired into any build target.
  - **New finding**: `bpmn-lite-compiler::dsl` — 9 of 15 submodules (~72% of dsl-tree
    pub items) have zero external consumers, giving H4.1 a concrete number behind the
    plan's "broad implementation tree" claim.
  - **New finding**: `bpmn-lite-store` has two genuine R5 violations
    (`DesignSessionRecord.events`, `TransactionContext.ops` — public mutable fields
    bypassing documented invariant-preserving accessor methods) and three-way
    module/root/named-list path drift for the same items.
  - Two of the plan's 7 priority crates (`designer-graph`, `dmn-lite-types`) are
    **not** opted into `[lints] workspace = true` and are therefore currently
    unprotected by the `unreachable_pub` ratchet this plan leans on — full opt-in
    audit covers all 33 crates, 8 are not opted in (see inventory doc).

- **Blind peer-review findings and dispositions:** not yet run — this receipt is the
  input to that review, not its output.

- **STOP-gate decision: blocked — awaiting peer review.**

Per R8 ("No tranche begins until the preceding tranche's receipt... have been accepted")
and Gate H0's own text ("Peer review ratifies the decisions in §3... approves the H1
migration list. No visibility is reduced yet."), **H1 does not begin until this receipt,
the two evidence documents, and the new findings above are reviewed and accepted.**
