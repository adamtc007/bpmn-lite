# BL-T2 receipt — workspace-kind cut-over, O3 as ruled (EOP-PLAN-CA-REUSE-001)

**Ruling applied:** O3 cut-over. No `Legacy` variant, no frozen-constant table, no grep
carve-out. Canonical tag bumped; old tags rejected typed; running instances drained/wiped.

## Environment enumeration (the ruling's precondition — done FIRST, before any wipe)

| environment | instances found | classification |
|---|---|---|
| localhost:5432 `bpmn_lite` | 1 | dev (docker-compose demo target; `docker-compose.yml:69`) |
| localhost:5432 `bpmn_lite_spike` | 6 | dev spike |
| localhost:5432 `bpmn_lite_spike2` | 0 | dev spike |
| localhost:5432 `bpmn_lite_test` | 1 | test DB (test-truncated) |
| localhost:5432 `bpmn_lite_runtime` | no `workflow_instances` table | dev artifact, not wiped |
| docker volumes (`bpmn-lite-*postgres*`) | containers all `Exited` 2–3 months | dev demo stack, docker-internal binds only |
| remote | **none** — every configured URL in the repo is localhost or docker-internal (`docker-compose.yml`, `scripts/reset-demo.sh`) | — |

All dev; no stop condition triggered. Note: the stopped docker volumes still hold
pre-cutover state; they are unreachable while stopped and must be re-provisioned
(`scripts/reset-demo.sh`) before any future use — old envelopes in them will decode to
the typed `UnsupportedWireVersion` rejection, never a value.

## Wipe record

`scripts/cutover-wipe.sql` run 2026-09-09 with `ON_ERROR_STOP=1` against `bpmn_lite`,
`bpmn_lite_spike`, `bpmn_lite_spike2`, `bpmn_lite_test`. Post-wipe verification:
`workflow_instances=0`, `job_queue=0` in all four. (`compiled_programs` also truncated
per the script — artifact corpus recompiles on next use.)

## What changed

- `bpmn-lite-types/src/session_stack.rs` — `SessionWorkspaceKind` (closed 11-variant
  enum, ten host-domain nouns) **deleted**. Replacement: `WorkspaceKind`, an opaque
  validated string — 1–64 bytes, ASCII alphanumeric plus `- _ . :`; typed
  `WorkspaceKindError` (`Empty`/`TooLong`/`IllegalByte`); serde via
  `try_from = "String"` so deserialization validates. bpmn-lite assigns the value no
  semantics.
- `bpmn-lite-types/src/canonical.rs` — `WorkspaceKind` encodes as tag `0x0B` + string.
  Decode: `0x0B` → validated value; **`0x00–0x0A` (the retired closed-enum band) →
  new typed `CanonicalDecodeError::UnsupportedWireVersion`** — a distinct error from
  `UnknownTag` (`0x0C+`), proving the retired band is a deliberate rejection, never a
  silent default. New `CanonicalDecodeError::InvalidValue` carries validation failures.
- Proptest strategy regenerated over the legal string alphabet; golden-bytes test
  updated (`WorkspaceKind::new("wk")` → `[0x0B, 02 00 00 00, 'w', 'k']`).
- Test fixtures across engine/store/store-postgres moved to neutral values
  (`workspace-a/b/c`), preserving each test's before/after distinctness.
- CI gate: `scripts/check-domain-nouns.sh` — whole-word, case-insensitive
  `kyc|cbu|onboarding|ca` over all of `bpmn-lite-types` (`.rs`/`.toml`/`.md`), **no
  allowlist** — wired into `.github/workflows/production-gates.yml` next to the other
  guards. Proof it bites: its first run failed on two of this very tranche's doc
  comments (the programme id contains "CA"); they were reworded, not exempted.

## Acceptance receipts

- **Zero domain nouns, grep in CI:** `check-domain-nouns.sh` → OK; wired in CI.
- **New-tag round-trip:** `workspace_kind_round_trip_is_a_fixed_point` (property, legal
  alphabet) + golden-bytes assertion — pass.
- **Old-tag typed rejection:** `workspace_kind_retired_tags_are_unsupported_wire_version`
  — every tag `0x00–0x0A` ⇒ `UnsupportedWireVersion`; `0x0C` ⇒ `UnknownTag` — pass.
- **External-crate start with a kind not in the old list:**
  `test_start_with_arbitrary_workspace_kind_runs_to_completion` (bpmn-lite-engine,
  external to bpmn-lite-types): starts with `orbital-survey.turn:v2`, runs to
  `Completed`, reads the value back verbatim — pass.
- **Cutover script:** run and verified (tables above).
- **`cargo public-api` diff:** full diff in `ca-reuse-001-BL-T2-public-api-diff.txt`
  (230 lines): removed `SessionWorkspaceKind` + 11 variants + impls; added
  `WorkspaceKind` (`new`/`as_str`/`MAX_LEN`/`Display`/`TryFrom<String>`/`From<…> for
  String`/canonical impls) + `WorkspaceKindError`;
  `SessionStackState.active_workspace: Option<WorkspaceKind>`.
- **Suites:** `cargo test --workspace` — 126 result lines, all ok, 0 failures
  (includes bpmn-lite-types 119/0); `cargo test -p bpmn-lite-store-postgres
  -- --test-threads=1` — 95/0 against live `bpmn_lite_test`.

## Boundary note

Serde stays string-shaped: an existing gRPC/REST client sending `"bpmn"` (or any legacy
snake_case token ≤64 legal bytes) still round-trips — as an opaque value. Only the
*canonical wire* band `0x00–0x0A` is retired, and only wiped dev state ever used it.
