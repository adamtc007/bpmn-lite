# Fuzz coverage — FFI owner_metadata decode (HTTP + gRPC)

Date: 2026-08-10

Scope: repo-wide fuzz-coverage audit follow-up, fourth and final tranche
(after `designer_operation_apply`, `dmn_lite_parse`, and
`yaml_workflow_parse`/`zeebe_bpmn_import`). Not a
`EOP-PLAN-BPMN-GAMEBOARD-001.md` phase bullet. Closes the audit's last
named gap, rated lowest severity of the five.

## Why this, and why last

The audit named `bpmn-lite-ffi-http`/`bpmn-lite-ffi-grpc`'s decode of
external-controlled bytes (`owner_metadata`, and the HTTP owner's live
callout response body) as a real gap but explicitly the lowest-severity
one: "lower severity than #1/#2... JSON syntax panics are less likely given
`serde_json`'s maturity, and the blast radius is a single FFI call." Picked
up last per that ordering, after the three higher-severity gaps were
closed.

## Scope decision: owner_metadata only, not the live response body

Two decode sites exist in `bpmn-lite-ffi-http/src/owner.rs`: the FFI call's
`input_payload` (`serde_json::from_slice::<serde_json::Value>`, line ~145)
and the live HTTP response body (same pattern, line ~307) inside
`HttpFfiOwner::invoke`. Both decode into a raw `serde_json::Value` — one of
the most heavily fuzzed decode paths in the entire Rust ecosystem upstream,
and neither is reachable without either fabricating an `FfiCall` against a
live `reqwest` client or mocking a real HTTP server per fuzz iteration
(confirmed `wiremock = "0.6"` is already a dev-dependency here, used by
existing integration-style tests — but a mock-server round-trip per
libFuzzer iteration would run orders of magnitude slower than in-process
fuzzing, the same "impractical for sustained fuzzing" judgment call made
for `legal_move_enumeration.rs`'s `MAX_ENUMERATION_CANDIDATES` zone in an
earlier tranche).

`HttpTemplateConfig::from_owner_metadata` / `GrpcTemplateConfig::
from_owner_metadata`, by contrast, are pure, in-process, zero-I/O functions
that decode externally-controlled bytes (a published FFI template's
`owner_metadata`) into validated configs — genuinely the same
external-trust-boundary shape the audit flagged, reachable at full
in-process fuzzing speed. This tranche covers those two; the live
response-body decode remains explicitly out of scope, same category as the
Postgres-backed store paths (not unsuited to correctness testing, but
unsuited to *this* tool — a live-round-trip integration/property test would
be the right instrument if deeper coverage is wanted there).

## What changed

Two new one-target cargo-fuzz workspaces (both auto-discovered by `cargo
xtask fuzz list`, zero xtask changes needed):

- **`bpmn-lite-ffi-http/fuzz`** (`owner_metadata_decode`): fuzzes
  `HttpTemplateConfig::from_owner_metadata(bytes, HttpIdempotency::
  Idempotent)`. Oracle: no-panic — any bytes either decode + validate
  (URL parses, every `path_param` has a matching `{}` placeholder,
  `success_status_codes` non-empty) or return a typed error. `idempotency`
  fixed to `Idempotent` — it's stored as-is and never branches
  decode/validation logic. Seeded with the exact fixture from
  `parse_post_with_path_param` (the richest branch: URL + method +
  path-param validation together).
- **`bpmn-lite-ffi-grpc/fuzz`** (`owner_metadata_decode`): fuzzes
  `GrpcTemplateConfig::from_owner_metadata(bytes)`. Oracle: no-panic — any
  bytes either decode + validate (non-empty endpoint) or return a typed
  error. Seeded with the exact fixture from `parse_custom_timeout`.
- Both `.gitignore` (`target`/`corpus`/`artifacts`/`coverage`), matching
  every other fuzz workspace's convention.

## Verification

- `cd bpmn-lite-ffi-http/fuzz && cargo check --bins`: clean.
- `cd bpmn-lite-ffi-grpc/fuzz && cargo check --bins`: clean.
- `cargo run -p xtask -- fuzz list`: both auto-discovered, `seeds: 1` each.
- `cargo run -p xtask -- fuzz run --target owner_metadata_decode --time 30`:
  both project's identically-named targets ran (xtask matches target names
  per-project, not globally, so this correctly exercises both without
  ambiguity) — gRPC: 2,552,583 execs, cov 1174; HTTP: 1,847,725 execs, cov
  3112. **0 crashes across both.**
- `cargo check --workspace --all-targets --all-features` (main workspace,
  unaffected by the fuzz-only sub-workspace additions): clean.
- `git status --porcelain`: touches only the two new
  `bpmn-lite-ffi-{http,grpc}/fuzz/` directories.

## Audit closure

This closes the last of the five gaps named in the 2026-08-10 repo-wide
fuzz-coverage audit
(`docs/receipts/fuzz-coverage-designer-operation-apply-2026-08-10.md`'s
originating context):

1. `designer_operation_apply` — closed.
2. `dmn_lite_parse` — closed, found and fixed a real UTF-8 char-boundary
   panic on the first burst.
3. `yaml_workflow_parse` — closed.
4. `owner_metadata_decode` (HTTP + gRPC) — closed (this receipt), scoped
   deliberately to the pure decode functions, not the live network
   response path.
5. `zeebe_bpmn_import` (the SESE-restructuring layer, confirmed genuinely
   uncovered by `xml_compile`'s call graph) — closed.

All five ran clean except #2, which found one real bug. Every new target is
auto-discovered by the existing `nightly-fuzz.yml`/`production-gates.yml`
wiring with no workflow edits required.
