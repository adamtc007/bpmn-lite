# EOP-PLAN-GRAPH-DSL-BRIDGE-001 — B2 receipt

Baseline: Gate B1 accepted at `9d79cb3` (branch
`codex/bpmn-gameboard-refactor`). **Tier: CAREFUL — the keystone tranche;
the equivalence proof is the entire value of the bridge.**

- **Scope delivered:** all five B2 work items — the round-trip proof
  harness over every B0 green fixture, printer/parser identity,
  idempotence, red-side identity preservation, and CI wiring — plus one
  genuine divergence the harness caught on its own first run, disposed as
  a recorded B0-equality amendment with the underlying production
  observation surfaced (not silently absorbed).

## The harness — `designer-graph/src/b2_roundtrip_receipts.rs`

`#![cfg(test)]` receipt module, following the `g2_receipts.rs` cement
precedent. Per green fixture, four proofs in one `assert_roundtrip`:

1. **Witness** — `DslReceipt.graph_state_hash` equals
   `DesignerDag::graph_state_hash(to_ir(dag))` (the content-derived
   identity, never the route-derived server hashes).
2. **Idempotence** — second emission byte-identical.
3. **Reparse identity** — the emitted source re-parses via
   `parse_workflow_str` with zero errors and re-prints byte-identically
   (printer/parser desync detector; the V&S §7 stop condition, now a
   permanent gate for every core shape).
4. **Plan equality** — `compile(emitted, derived_registry)` ≡
   `project_ir(to_ir(dag), wf_id)` under normalized-JSON equality:
   `span` stripped by name (the only excluded field, per B0), and — B2
   amendment, below — `Split.flows` compared as a multiset.

Fixtures: G1 linear-completed; G2 task→message-wait→task; G3 And×2;
G4 And×3; G5 terminate-end (sentinel asserted); G6 nested And blocks;
G7 shared-task_type dedup (`required_symbols == ["cbu.same"]`). Red
side: `red_refusal_leaves_identity_untouched` — a refusing DAG
(`TimerWait`, exact-variant-asserted via downcast) has identical
`graph_state_hash` before and after the attempt; "no partial artifact"
is structural (`Result`).

## Divergence caught by the harness's own first run — G3/G4/G6 red

**`project_ir` writes `SplitExecNode.flows` in petgraph
arena-iteration order** (`edges_directed`, most-recent-first insertion
order) — an *edit-order-derived* order, while emission's flow order is
frozen as edge-id-sorted. Strict JSON array equality failed on every
And-block fixture.

Disposition — **B0 equality-table amendment (recorded here per the
amendment rule): `Split.flows` compares as a multiset** (sorted by
content before comparison). Grounds: And-flow order carries no
**outcome** semantics — the blind review traced every consumer: flows
lower in order into `Instr::V2Fork{targets}`, and the kernel's barrier
reconciles by arrival **count**, never branch index; in-core Parallel
flows carry no conditions/placeholders. (First draft said "no execution
semantics" — the review narrowed it: order DOES feed V2Fork target
order and hence fiber-ID and event-tape ordering, so order-differing
plans are outcome-equivalent but not replay-tape-identical.) The frozen
table ruled the *fields* compared but never ruled flow *order*; and the
alternative — changing `project_ir` to sort — would change the stored
bytes of a shipped production artifact path, which is not this plan's
to decide.

**Surfaced as a standalone observation (outside this plan's scope, for
a ruling if you want one): `project_ir`'s flow order is not
content-canonical.** Two edit orders building `ir_graphs_equivalent`
graphs produce plans with differently-ordered `flows` arrays — i.e.
different stored plan JSON/bytes for the same design content, AND
(per the review's tracing) different lowered `V2Fork` target order,
fiber-ID assignment, and event-tape ordering at runtime. Same family as
the route-derived-vs-content-derived hash trap already documented in
`schema.rs:352-362`, and directly relevant to G6's replay-equivalence
work on this branch. Outcome semantics unaffected; flagged, not fixed.

## Mutation red-trace (harness proven able to fail)

Deliberately corrupted the emitter's End sentinel
(`"terminated"` → `"finished"`) in the working tree:
`g5_terminate_end` **failed** plan-equality with exactly the expected
diff (`"status": "finished"` vs `"status": "terminated"`); reverted;
8/8 green again. The corruption was never committed. Trace:

```
test b2_roundtrip_receipts::g5_terminate_end ... FAILED
assertion `left == right` failed: DSL-compiled plan must equal project_ir plan field-by-field (spans excluded)
  left:  ... "end": {"End": {"id": "end", "status": "finished"}} ...
  right: ... "end": {"End": {"id": "end", "status": "terminated"}} ...
```

## CI wiring

The module rides `.github/workflows/production-gates.yml`'s
"Migrations, RLS, recovery, integration, and property tests" step
(`cargo test --workspace --features postgres,database,embed,candle-probe
-- --test-threads=1`, production-gates.yml:95, workflow triggered
`on: pull_request`) — it runs on every PR, not only under a local
`cargo test`. (First draft cited a step name that doesn't exist —
"Run full test suite" — caught by blind review; command and line were
correct.) Runtime is trivial (8 tests, <10 ms), per the plan's "rides
the existing test job" clause; no dedicated step needed.

## Verification

- `cargo test -p designer-graph --lib`: **80 passed / 0 failed**
  (72 prior + 8 new).
- `cargo test -p bpmn-lite-compiler --lib`: 198/0 (unchanged).
- `cargo check --workspace --all-targets`: clean (same 2 pre-existing
  unrelated warnings).
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass —
  **public API diff: none** (the harness is `#[cfg(test)]`; no baseline
  change this tranche).
- `python3 scripts/check-test-only-pub.py`: `ok: 0`.

- **Refusal catalogue delta vs B0's frozen list: none.** One
  equality-table amendment (flows-as-multiset, above).

- **Known deviations or explicitly parked work:**
  - **Plan item 2 substitution (declared after blind review flagged it
    as undeclared):** the plan says "assert the re-parsed
    `WorkflowSource` equals the emitted AST"; the harness instead
    asserts the print→parse→print **string fixpoint**. Direct AST
    equality is impossible as specified — re-parsed spans are real
    source positions, emitted spans are zero, and the AST types derive
    no `PartialEq` — so byte-fixpoint of the printed form is the
    faithful substitute (any field `ToSexpr` prints is covered;
    plan-equality separately covers every plan-relevant field). Residual
    risk: a future AST field that is neither printed nor plan-relevant
    could diverge silently — accepted, ratify at this gate.
  - The `project_ir` flow-order observation (above) — surfaced for a
    separate ruling, untouched here.
  - Red-side "unchanged identity" is cemented via one representative
    fixture at the layer owning the hash, not per-variant × 24 — the
    per-variant exact-refusal cement lives in B1's `emit.rs` tests; the
    `&IRGraph` signature makes mutation structurally impossible, so one
    identity cement is defence in depth, not the load-bearing proof.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) performed the load-bearing check this
  tranche hinges on — exhaustively tracing every consumer of
  `Split.flows` through frontend lowering into the kernel's
  `V2Fork`/`V2Join` to establish whether the multiset amendment hides a
  semantic divergence (verdict: sound for outcome semantics — barrier
  reconciliation is count-based, no branch indexing anywhere; but order
  feeds fork-target/fiber-ID/tape ordering, see the narrowed wording
  above). Also verified `normalize_plan`'s totality (serde_json without
  `preserve_order` → deterministic stringification; all 8 span fields in
  the plan shape are inside node variants — the strip is complete and
  nothing compared is over-normalized), `parse_workflow_str`'s
  strictness (errs on ANY parse error — the B0 vacuous-test disease
  cannot recur through this entry point), fixture faithfulness to B0's
  G1–G7, CI wiring live on `pull_request`, and reproduced all test runs
  and the mutation red-trace (restoring the file byte-exact). Verdict:
  core claims stand; three findings, all disposed by edits:
  1. "No execution semantics" narrowed to "no OUTCOME semantics" with
     the fork-order/tape-ordering consequence stated (amendment wording
     + observation enriched above).
  2. The plan-item-2 AST-equality→string-fixpoint substitution was an
     undeclared deviation — now declared under known deviations with
     its rationale and residual risk.
  3. The cited CI step name ("Run full test suite") didn't exist —
     corrected to the real step name in receipt and module doc.
  The reviewer also noted the witness proof is near-tautological today
  (same function both sides) — retained deliberately: it cements
  against a future regression to the route-derived hashes, which is
  exactly the naming trap `schema.rs:352-362` documents.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate B2's own text: red→green trace recorded (the mutation trace
above), both must-refuse and must-admit-equivalent fixtures live and
CI-wired, cement-locked hereafter. **U4 of
EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 becomes unblockable once this
gate is accepted** (noted per the plan; U4 is NOT opened by this
receipt). B3 does not start until this gate is accepted.
