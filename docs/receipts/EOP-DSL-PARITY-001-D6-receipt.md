# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D6: Inclusive alignment (fork D / P1½)

**Status:** Blind-reviewed — ACCEPT, no corrections needed. **Accepted by
Adam.** This is the **final tranche** in the ratified
`EOP-PLAN-DSL-PARITY-001` programme — the programme (D0→D6) is now closed.

## What was built

Per the D6.0 design note (`EOP-DSL-PARITY-001-D6.0-design.md`), Adam ruled
**option (a)**: refuse Diverging `GatewayInclusive` emission unconditionally,
mirroring D5's `GatewayXor` disposition exactly. No new IR field, no
mechanical blast radius — `gateway_pairs`/`project_ir` already supported
`GatewayInclusive` fully before this tranche (its `direction` field predates
this session). The only gap was `emit_dsl`'s catch-all refusal.

1. **`bpmn-lite-compiler/src/dsl/emit.rs`**: `GatewayInclusive` moved out of
   the out-of-core catch-all into its own match arm, structurally identical
   to `GatewayXor`'s D5 arm — Converging emits a `JoinAst { mode: Or, .. }`
   via the existing `join_to_split`/`shared_joins` dedup machinery (unchanged
   from And/Xor); Diverging returns a new unconditional-refusal error,
   `DslEmitError::GatewayInclusiveSplitUnrepresentable`. `project_ir` is
   untouched — no grammar dependency on that direction, same as Xor.
   - New error variant added, documented with the same rationale trail as
     D5's `GatewayXorSplitUnrepresentable` (`parse_split` mandates `:plug`
     for `split-or`, `IRNode::GatewayInclusive` has no field to source one).
   - `UnmatchedGateway`'s message extended to name both Xor and Inclusive's
     asymmetry (Diverging always refuses earlier; only Converging can reach
     `UnmatchedGateway`).
   - Catch-all arm's comment/count updated: 4 kinds remain out of core
     (`HumanWait`, `DataObject`, `FfiServiceTask`, `SendTask`).
   - `red_unsupported_node_all_remaining_kinds`: removed the
     `GatewayInclusive` fixture entry (cement update, named not silent — it
     joined the core, like `GatewayXor` did at D5), comment corrected from
     the stale "7 kinds" to the now-accurate 4.
   - `stage1_order_kind_before_token_and_degree_before_pairing`: its
     kind-gate-before-token-check fixture switched from `GatewayInclusive`
     (now in-core) to `HumanWait` (still out-of-core) — same pattern D5 used
     when it moved `GatewayXor` out of this fixture's role.

2. **`designer-graph/src/b2_roundtrip_receipts.rs`**: added `inclusive_gw`
   helper (mirrors `xor_gw`) and three new fixtures, structurally identical
   to D5's g18/g18b/g19:
   - `inclusive_block_conditioned` — shared builder: start → Inclusive-split
     (cond-A / default-B) → both branches → Inclusive-join → end.
   - `g20_inclusive_split_refuses_at_emission` (RED): asserts
     `dag.emit_dsl(...)` errors with message containing "split1" and
     "GatewayInclusive".
   - `g20b_inclusive_split_still_projects_to_plan` (GREEN): calls
     `dag.to_ir()` then `project_ir` directly, asserts success and correct
     `Split { mode: Inclusive, join: "join1", flows.len() == 2 }`.
   - `g21_unmatched_converging_inclusive_gateway_refuses_at_emission` (RED):
     graph with a lone Converging `GatewayInclusive` and no diverging one
     anywhere, isolating the Converging arm's own `UnmatchedGateway` path
     from the Diverging arm's unconditional refusal.

3. **Public-API baseline**: `docs/generated/public-api-baselines/bpmn-lite-compiler.txt`
   regenerated. Diff confirmed purely additive:
   ```
   +pub bpmn_lite_compiler::dsl::DslEmitError::GatewayInclusiveSplitUnrepresentable
   +pub bpmn_lite_compiler::dsl::DslEmitError::GatewayInclusiveSplitUnrepresentable::id: alloc::string::String
   ```
   +2/−0. Header updated to name D6 as the producing tranche.

## Red→green trace

| Fixture | Axis | Expected |
|---|---|---|
| `g20_inclusive_split_refuses_at_emission` | Diverging Inclusive can never satisfy `split-or`'s mandatory `:plug`/`:condition` | RED — `GatewayInclusiveSplitUnrepresentable` |
| `g20b_inclusive_split_still_projects_to_plan` | Same graph, `project_ir` path only | GREEN — `Split{mode: Inclusive, join: "join1", flows: 2}` |
| `g21_unmatched_converging_inclusive_gateway_refuses_at_emission` | Converging Inclusive with no paired diverging node | RED — `UnmatchedGateway` (isolates the Converging arm's own pairing-refusal path) |
| `red_unsupported_node_all_remaining_kinds` | `GatewayInclusive` no longer in the out-of-core set | cement update — entry removed, count corrected 7→4 |
| `stage1_order_kind_before_token_and_degree_before_pairing` | kind-gate-before-token-check still holds for an out-of-core kind | fixture switched to `HumanWait`, still passes |

## Mechanical blast radius

Zero new IR fields, zero struct-literal/match-exhaustiveness breakage —
the smallest D-gate this programme has run, exactly as the D6.0 design note
predicted for option (a). Touched files: `emit.rs` (arm + error variant +
two test fixups), `b2_roundtrip_receipts.rs` (three new fixtures + one
helper), the public-API baseline.

## Verification sweep

- `cargo build -p bpmn-lite-compiler -p designer-graph` — clean.
- `cargo test -p bpmn-lite-compiler -p designer-graph`: 228 + 99 passed, 0
  failed (designer-graph +3 vs D5's 96, for g20/g20b/g21).
- `cargo check --workspace --all-targets` — clean (two pre-existing,
  unrelated warnings in `bpmn-lite-server-designer`, untouched by this
  tranche).
- `cargo test --workspace` — every reported `test result:` line is `ok`, 0
  failed, across all crates and doc-tests.
- `python3 scripts/check-semantic-gameboard-boundaries.py` — exit 0, clean
  after baseline regeneration (confirmed +2/−0 diff before regenerating).
- `python3 scripts/check-test-only-pub.py` — exit 0, `0 #[cfg(test)] pub
  item(s)`.

## Programme closure note

This closes the last tranche (`D6`) in `EOP-PLAN-DSL-PARITY-001`'s ratified
sequence (`D0`→`D6`). Two items were explicitly surfaced as out of this
programme's scope during D5/D6 and remain open for future, separately-ruled
work (not blocking this gate's closure):
- Named-subset condition semantics for OR gateways (CLAUDE.md's settled
  target architecture) — the DSL grammar's `ConditionAst` remains Eq-only
  for both Xor and Inclusive; no multi-value named-subset shape exists yet.
- `bpmn-lite-authoring/src/importer.rs`'s `find_corresponding_join` — a
  second, independently-maintained gateway-pairing algorithm for the
  XML-import path, never unified with `gateway_pairs` (flagged at D5,
  restated here as still open).

## Blind-review disposition

Dispatched via the Agent tool (`general-purpose`, authorship-blind — no
prior context, told to re-derive every claim from source rather than trust
the receipt). The reviewer independently:
- Re-read `parse_split` in `parser.rs` directly and confirmed the `:plug`
  requirement is unconditional for non-`And` modes (the load-bearing claim).
- Read the full `GatewayInclusive` match arm byte-for-byte against the
  `GatewayXor` arm above it, confirmed exactly one live arm (grepped for
  duplicates), confirmed removal from the catch-all and the corrected
  4-kind count.
- Confirmed `stage1_order_kind_before_token_and_degree_before_pairing`'s
  substitution (`GatewayInclusive` → `HumanWait`) is sound — `HumanWait` is
  genuinely still out-of-core and the test still exercises the ordering it
  claims to.
- Confirmed via `git diff` that only `emit.rs`, `b2_roundtrip_receipts.rs`,
  and the public-API baseline were touched — `ir_plan.rs`'s `GatewayInclusive`
  arm and `lowering.rs`'s `gateway_pairs` match predate this tranche
  entirely, substantiating the "zero mechanical blast radius" claim.
- Traced each of g20/g20b/g21's actual graph shapes node-by-node, confirmed
  g20's pair is properly matched (so the diverging refusal is genuinely the
  first failure), g20b asserts `SplitMode::Inclusive` specifically (not
  copy-paste residue from Xor), and g21 has no diverging Inclusive node
  anywhere, correctly isolating the Converging arm's `UnmatchedGateway` path.
- Ran `cargo test -p bpmn-lite-compiler -p designer-graph` live: 228 + 99
  passed, 0 failed — matched the receipt exactly.
- Ran the public-API boundary gate live, confirmed exit 0 and the exact
  +2/−0 diff via `git diff`.
- Grepped the whole repo for other `GatewayInclusive` producers/consumers
  (`ops.rs`, `verifier.rs`, `parser.rs`, `ir_to_dto.rs`, `importer.rs`,
  `dto_to_ir.rs`, `runbook.rs`) and confirmed none needed changes — consistent
  with option (a)'s design note (no plug-authoring surface exists anywhere
  for Inclusive, so nothing downstream is affected by a refusal-only change).

**Verdict: ACCEPT.** No blocking or non-blocking issues found. The design
doc's option (a) was confirmed shipped precisely — no leakage of option (b)
or (c) — and every fixture was confirmed to test what it claims to test.

## STOP

Blind-reviewed, ACCEPT, no corrections applied. D6 — and with it, the
`EOP-PLAN-DSL-PARITY-001` programme — **accepted by Adam.** Programme closed:
D0 through D6, no further tranche defined in the ratified plan doc. The two
items noted in "Programme closure note" above (named-subset OR condition
semantics; unification of `gateway_pairs` with the XML importer's
`find_corresponding_join`) remain open, out-of-scope items for future,
separately-ruled work.
