# Fuzz coverage — designer_operation_apply

Date: 2026-08-10

Scope: repo-wide fuzz-coverage audit follow-up. Not a `EOP-PLAN-BPMN-GAMEBOARD-001.md`
phase bullet — the audit spans the whole workspace (7 fuzz sub-workspaces, 31
targets going in), not just the gameboard plan's utterance-engine surface.

## Why this, and why first

A full fuzz-coverage audit (dispatched as an independent research agent, not
self-certified) inventoried all 31 existing targets across 7 cargo-fuzz
workspaces and cross-referenced them against every parsing/decode/state-
transition surface in the repo. It found two structural gaps, not gold-
plating: `dmn-lite-parser`'s hand-written DSL frontend (no target at all),
and `bpmn-lite-server-designer/src/rest.rs`'s `Vec<Operation>` JSON decode
feeding straight into `apply_production` (the untrusted-network entry point
for the AST-mutator architecture CLAUDE.md names as this codebase's core
correct-by-construction guarantee). Adam picked the second to implement
first: it is the live, internet-facing surface — the designer REST server
accepts this exact payload shape both as a stored session replay
(`reconstruct_designer_dag`, `rest.rs:2909`) and as the live write path
(`SessionGraphEditBody.operations`, `session_graph_edit_endpoint`,
`rest.rs:3169`) — and it had zero fuzz coverage despite `apply_production`
being the single production-plumbed function the whole mutator design rests
on being correct-by-construction under hostile input, not just well-formed
input.

## What changed

- **`bpmn-lite-server-designer/fuzz/fuzz_targets/designer_operation_apply.rs`**
  (new): `libfuzzer_sys::fuzz_target!` over raw bytes. Decodes
  `serde_json::from_slice::<Vec<designer_graph::ops::Operation>>` (the exact
  type and decode call both real call sites use), seeds a fresh
  `DesignerDag` with a single `Start` node (mirroring
  `reconstruct_designer_dag`'s own seeding, same fixed `NodeKey` derivation),
  applies via `apply_production`, then calls `StagedCandidate::admit()` on
  any successfully staged candidate — exercising the full
  `to_ir`/verify/lower theorem chain the live endpoint relies on before ever
  persisting anything. Oracle: no panic anywhere in decode → stage → admit,
  for any byte sequence; a hostile-but-well-formed op tape must come back a
  typed `Result::Err`, never crash the session it's staged against. Caps
  input at 32KB (consistent with the resource-limit ceilings ratified this
  session for the gameboard side) to keep sustained fuzzing throughput
  practical.
- **`bpmn-lite-server-designer/fuzz/Cargo.toml`**: registered the new
  `[[bin]]`; added `serde_json = "1"` as a direct dependency (previously
  only pulled in transitively — the harness needs it explicitly for the
  decode call).
- **`bpmn-lite-server-designer/fuzz/seeds/designer_operation_apply/admitted-linear.json`**
  (new): a real, proven-admitting two-operation tape (`start -> t1 -> end`
  via two `InsertAfter`s), generated and verified by a throwaway `#[ignore]`
  test added temporarily to `designer-graph/src/productions.rs` (ran once
  under `cargo test -p designer-graph ... -- --ignored --nocapture`, printed
  the serialized JSON, confirmed `staged.candidate.admit()` succeeds on it,
  then the test was deleted — `git diff` on `productions.rs` is empty).
  Hand-writing this JSON blind against the `Operation`/`IRNode`/`NodeKey`
  serde shapes (externally-tagged enums, transparent newtype `NodeKey`)
  would have been guessable but unverified; generating it from the real
  types and proving it admits is the same discipline the gameboard fuzz
  targets already use for their reference-model fixtures.

## Verification

- `cd bpmn-lite-server-designer/fuzz && cargo check --bin
  designer_operation_apply`: clean.
- `cargo run -p xtask -- fuzz list`: auto-discovered with no xtask changes
  needed (fork F-C's directory-scan discovery), `seeds: 1` confirms the
  admitting seed file is picked up.
- Unseeded 30s live burst: 2,087,996 execs, cov 1372, 0 crashes.
- Seeded 30s live burst (corpus cleared first, re-run from the admitting
  seed): 1,939,207 execs, **cov 4624** — the coverage jump (1372 -> 4624)
  confirms the harness is actually reaching deep into
  `apply_production`/`admit`'s branches (successful staging, structural
  verification, IR lowering), not just exercising the early
  decode-failure return path. 0 crashes across both bursts (~4M execs
  total).
- `cargo check --workspace --all-targets --all-features` (main workspace,
  unaffected by the fuzz-only sub-workspace change): clean.
- `git status --porcelain`: touches exactly
  `bpmn-lite-server-designer/fuzz/{Cargo.toml,Cargo.lock}`, the new
  `fuzz_targets/designer_operation_apply.rs`, and the new
  `seeds/designer_operation_apply/` — `designer-graph/src/productions.rs`
  confirmed clean (the scratch test left no trace).

## What this does not do

- Does not fuzz `resolve_direct_edit` (the semantic-equivalence recovery
  path in `session_graph_edit_endpoint`) — that function requires a live
  board/position/workbook construction chain far beyond a hostile-bytes
  harness; it's already exercised by
  `test_session_graph_edit_admits_and_persists` and siblings in `rest.rs`'s
  own test module, and by the `bpmn_binding_extract` fuzz target for the
  lexical-retrieval half of that chain.
- Does not cover the other ~14 `Json<T>` axum extractors in `rest.rs`
  (`CompilePreviewBody`, `DmnPreviewRequest`, `MacroApplyRequest`, etc.) —
  named in the audit as part of the same gap category but out of scope for
  this tranche; `Operation`/`apply_production` was the highest-severity
  single item.
- Does not touch `dmn-lite-parser` (the audit's other "big deal" gap, same
  severity tier) — a separate, unrelated crate with its own lexer/parser
  pair; tracked as the next candidate, not started here.
- The new target gets the standard nightly 20-minute live-fuzz run and
  PR-time regression-corpus replay via the existing `nightly-fuzz.yml`/
  `production-gates.yml` wiring (auto-discovered, no workflow edits
  needed) — but per the audit's CI-asymmetry finding, it is not one of the
  four targets (`v3_route_admission`, `legal_move_enumeration`,
  `preview_compilation`, `evidence_fusion`) that additionally get a
  PR-time live-fuzz smoke pass; that asymmetry is unchanged by this
  receipt, not decided one way or the other here.
