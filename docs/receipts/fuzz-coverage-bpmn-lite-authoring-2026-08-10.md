# Fuzz coverage — bpmn-lite-authoring (yaml_workflow_parse + zeebe_bpmn_import)

Date: 2026-08-10

Scope: repo-wide fuzz-coverage audit follow-up, third tranche (after
`designer_operation_apply` and `dmn_lite_parse`). Not a
`EOP-PLAN-BPMN-GAMEBOARD-001.md` phase bullet. Closes two of the audit's
three remaining smaller gaps in one workspace, since both live in the same
crate.

## Investigation first: is the SESE-importer gap real?

Before writing anything, checked whether `bpmn-lite-engine/fuzz`'s
`xml_compile` target's call graph actually reaches
`bpmn-lite-authoring::import_zeebe_bpmn` (the audit flagged this as an open
question, not a confirmed gap — "worth confirming... if not, this is a real
(if narrower) gap"). Traced it:

- `bpmn-lite-authoring` is a **dev-dependency only** of `bpmn-lite-engine`
  (`bpmn-lite-engine/Cargo.toml`'s `[dev-dependencies]`), used solely by
  `bpmn-lite-engine/src/tests.rs`'s own cross-crate compatibility tests
  (`bpmn_lite_authoring::parse_workflow_yaml` /
  `compile_program_from_dto`).
- `xml_compile`'s driver (`bpmn-lite-engine/fuzz/src/lib.rs::drive_xml_compile`)
  calls `engine.compile(&xml)`, which per
  `bpmn-lite-engine/Cargo.toml`'s own header comment ("Locked decision...
  the engine does NOT depend on `bpmn-lite-authoring`") goes straight to
  `bpmn_lite_compiler::parse_bpmn` — never through `bpmn-lite-authoring`.
- Every call site of `import_zeebe_bpmn` in the whole repo is inside
  `bpmn-lite-authoring`'s own `src/importer.rs` unit tests and
  `tests/importer_compatibility_tests.rs` — confirmed via a repo-wide grep.

**Conclusion**: the gap is real, not a false positive. `import_zeebe_bpmn`
does its own additional split/join pairing and (when `permissive`)
best-effort topology restructuring on top of the same `parse_bpmn` frontend
`xml_compile` already covers — that extra layer had zero fuzz coverage.

## What changed

New `bpmn-lite-authoring/fuzz/` cargo-fuzz workspace, two targets (both
auto-discovered by `cargo xtask fuzz list`, zero xtask changes needed):

- **`yaml_workflow_parse`**: the audit's YAML-frontend gap. Fuzzes
  `parse_workflow_yaml` (Y-O1: no-panic on any bytes — `serde_yaml`
  deserializers are generally more panic-prone than `serde_json`'s
  hardened path, and this is a third distinct untrusted-text entry point
  into the same `WorkflowGraphDto`/IR pipeline alongside the BPMN-XML and
  S-expression frontends). Defense-in-depth chains a successful parse
  through `compile_program_from_dto` (Y-O2: no-panic only, deliberately
  not gate parity — a grammar-valid YAML document may legitimately fail
  structural/graph validation). Seeded with the exact fixture from
  `parse_workflow_yaml`'s own `test_basic_yaml_parse` unit test.
- **`zeebe_bpmn_import`**: the SESE-importer gap confirmed above. Fuzzes
  `import_zeebe_bpmn(xml, "fuzz-wf", permissive)` directly under BOTH
  `permissive` settings against the same hostile bytes (Z-O1: no-panic —
  `false`/`true` exercise materially different control flow: strict SESE
  rejection vs. best-effort restructuring). Seeded with the exact
  `VALID_SESE_XML`/`INVALID_CROSSING_XML` fixtures from `importer.rs`'s own
  `test_zeebe_import_rejections`/`test_zeebe_permissive_import` tests
  (reproduced verbatim, cross-checked byte-for-byte against the source
  constants before use — not hand-approximated).
- `.gitignore` (`target`/`corpus`/`artifacts`/`coverage`), matching every
  other fuzz workspace's convention.

## Verification

- `cd bpmn-lite-authoring/fuzz && cargo check --bins`: clean, both targets.
- `cargo run -p xtask -- fuzz list`: both auto-discovered
  (`yaml_workflow_parse [seeds: 1]`, `zeebe_bpmn_import [seeds: 2]`).
- `yaml_workflow_parse`: two live-fuzz bursts (30s then a fresh-corpus 90s)
  — 826,277 execs then 2,477,870 execs, cov up to 5949, **0 crashes across
  both**.
- `zeebe_bpmn_import`: two live-fuzz bursts (30s then a fresh-corpus 90s) —
  527,396 execs then 1,609,910 execs, cov up to 2729, **0 crashes across
  both**.
- `cargo check --workspace --all-targets --all-features` (main workspace,
  unaffected by the fuzz-only sub-workspace addition): clean.
- `git status --porcelain`: touches only the new `bpmn-lite-authoring/fuzz/`
  directory.

## What this does not do

- Unlike the previous two tranches, this one found **no crash** — both
  targets ran clean across ~4M total executions. That is itself the
  finding worth recording, not a reason to skip the receipt: the gap is
  now covered going forward (nightly 20-minute live-fuzz + PR-time
  regression replay via existing `nightly-fuzz.yml`/
  `production-gates.yml` auto-discovery), even though this session's
  bursts didn't surface anything.
- Does not touch the audit's remaining named gap (FFI callout-response
  decode in `bpmn-lite-ffi-http`/`bpmn-lite-ffi-grpc`) — next candidate,
  not started here.
- Neither new target is one of the four (`v3_route_admission`,
  `legal_move_enumeration`, `preview_compilation`, `evidence_fusion`) that
  get an additional PR-time live-fuzz smoke pass — same named, unchanged
  asymmetry as the prior two tranches' receipts.
