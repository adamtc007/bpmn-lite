# Shared crates standalone remediation — Phase 8 receipt

**Date:** 5 August 2026  
**Status:** local release qualification complete; external promotion held  
**Shared release:** `v0.2.1` / `586431f81e2bb9101578af5167b8a35335f5a09e`

## Decision

The release mechanics and local non-production rehearsal are implemented. Both
consumers now build hermetic, revision-labelled application images with
CycloneDX SBOMs and exact shared-source receipts. BPMN passed strict semantic
replay across upgrade and rollback. `ob-poc` passed persisted-session and ACP
policy rollback compatibility, and a deployment-discovered runtime packaging
defect was fixed before the final candidate was cut.

Gate 8 remains held at the external-promotion boundary. No container registry,
non-production cluster, captured production traffic corpus, owner-approved
tolerance policy, or dashboard destination is declared in the repositories or
session brief. A local container build is not represented as production
deployment.

## Repository ledger

| Repository | Branch | Phase 8 start | Phase 8 end |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `main` | `586431f81e2bb9101578af5167b8a35335f5a09e` | unchanged; annotated tag `v0.2.1` |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/bpmn-semantic-pack` | `de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a` | `d598d7e3c0eda7bac1e1379af2d635bca7bfeca2` |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-policy-consumer` | `e51dda11a02f4993e8023b934c4fd1df4293b983` | `40587c9863c7bcccb427cef3600033768816fcf5` |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `5cb176f` | this receipt commit |

All implementation commits in the two consumer repositories were pushed before
this receipt was written.

## Release-candidate artifacts

### BPMN designer

- Image: `bpmn-lite-designer:rc-d598d7e3c0ed`
- Image digest: `sha256:c932debb00fe0434479b50fdafcc10aa330da9c3936d2ac4f61ef1224d4f7847`
- Image size: 14,993,403 bytes
- Application revision label: `d598d7e3c0eda7bac1e1379af2d635bca7bfeca2`
- Shared DSL revision label: `586431f81e2bb9101578af5167b8a35335f5a09e`
- SBOM: 235 components; SHA-256
  `ee623f7fe88232abd79c4a472796c043088fac9b3754e784bff62cf2ca618e9d`
- Runtime RSS after health and PostgreSQL recovery: 6.9 MiB
- SBOM contains no `pgvector` package. SQLx is present only because this
  candidate deliberately enables the designer's PostgreSQL durability feature,
  not through the shared embedding boundary.

Receipt:
`target/release-candidate/d598d7e3c0ed/release-receipt.env` in the qualified
BPMN checkout.

### `ob-poc`

- Image: `ob-poc:rc-40587c9863c7`
- Image digest: `sha256:5aa1e4b6bb2f2839938ff246b5469b313914efc4133cced59651043296d867d7`
- Image size: 40,831,766 bytes
- Application revision label: `40587c9863c7bcccb427cef3600033768816fcf5`
- Shared DSL revision label: `586431f81e2bb9101578af5167b8a35335f5a09e`
- BPMN bus dependency revision label:
  `de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a`
- SBOM: 554 components; SHA-256
  `4ba0721053a6355ed0b216ed5748fac077edaa875ada30abe5c2d8723f543881`
- Runtime RSS after model, pack, snapshot, and API initialization: about
  310–314 MiB
- Model: `BAAI/bge-small-en-v1.5`, pinned revision
  `5c38ec7c405ec4b44b94cc5a9bb96e735b38267a`, 384 dimensions; observed
  initialization 175–202 ms after the image repair.

Receipt:
`target/release-candidate/40587c9863c7/release-receipt.env` in `ob-poc`.

Both binaries were built with `cargo-auditable 0.7.5`; Docker Scout produced
the CycloneDX receipts. Build scripts fail if tracked source is dirty, validate
the image revision labels, and derive artifact paths from the exact HEAD.

## Phase 8 implementation commits

### BPMN

1. `f5d10f4` — `build: add hermetic designer release candidate`
2. `16eb255` — `build: embed auditable Rust dependency metadata`
3. `d598d7e` — `build: enable durable designer rollback qualification`

The implementation adds `Dockerfile.designer`, `.dockerignore`, and
`scripts/build-designer-release-candidate.sh`.

### `ob-poc`

1. `e5722e2a` — `build: make ob-poc release image hermetic`
2. `070aa632` — `build: embed auditable Rust dependency metadata`
3. `40587c98` — `fix: package runtime semantic inputs`

The implementation repairs the stale Docker build boundary, adds
`scripts/build-release-candidate.sh`, labels all exact dependency revisions,
honours `OBPOC_PACKS_DIR` in the production journey-pack loader, and packages
the checked-in entity and lexicon snapshots.

## BPMN shadow and rollback result

An isolated PostgreSQL 16 database and Docker network were used. The current RC
ran with the PostgreSQL store and `BPMN_MAPPER_ROLLOUT=shadow`. A persisted
graph-backed session contained `Start -> review_documents -> End` and was
probed with:

1. `Places a node on an existing route, after the selected node called collect_documents`
2. `set the guard budget`
3. `zzz qqq xyzzy`

The normalized receipts included the complete ordered ranking, lane scores,
final scores, disposition, evidence trace, action producer, board/retrieval/
model/policy/context hashes, and decision hash. Key results were:

| Probe | Top result | Disposition |
|---|---|---|
| insert-after utterance | `op.insert_after` at `0.63636` | `Candidate(op.insert_after)` |
| guard utterance | abstain / guard candidates tied at `0.5` | `OutOfScope` |
| hostile nonsense | abstain at `0.99` | `OutOfScope` |

The rollback artifact used application revision
`de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a` and shared revision
`586431f81e2bb9101578af5167b8a35335f5a09e`; its image digest was
`sha256:c8305be4670a6e70e78fbdfb274bfc2bf6ecc4dfe1631c0c2d3339fa5473d2b7`.
It reopened the same persisted
session, replayed the same three utterances, and produced an empty semantic
diff: **three of three decisions identical**. Current request latencies were
3.9–8.6 ms; rollback latencies were 4.6–5.7 ms. Health remained `status=ok`,
and the final persisted session contained eight events.

The mapper health receipt also records that suggestions/workbooks and automatic
application were disabled, ratification was required, and no Tier-1 bundle was
loaded. Therefore this rehearsal qualifies deterministic shadow behaviour, not
a promoted Tier-1 model deployment.

## `ob-poc` shadow and rollback result

The final RC was started against an isolated PostgreSQL 18 + pgvector database.
It loaded:

- 14 journey packs from `/app/config/packs`;
- 1,253 configured verbs across 136 domains;
- entity snapshot version 1, 1,452 entities, hash prefix `c26d8e268e7b`;
- lexicon snapshot hash
  `22265ec0851528119fd5e852730b5c501825acb8f1947c823ac6cc252bec9958`;
- the pinned BGE model described above.

Selecting the SemOS Maintenance workspace produced the deterministic candidate
`semos-maintenance` with score `0.0`, then persisted the selected in-pack state
in `"ob-poc".repl_sessions_v2`. On a schema-only database, later verb
utterances correctly remained recoverable no-match outcomes because the
database contains no production semantic seed rows. Those outcomes are not
used as a claim about production retrieval quality.

The exact prior RC, `ob-poc:rc-070aa6327d96` at application revision
`070aa6327d96ab7fb29249f70774c4709ed2af4f` and image digest
`sha256:c1895bf55c6f8eb6ed335fbc3136a8d559c8847eeb3ddcbb1dc677f1580d6f12`,
was then started against the same database. Results:

- canonical ACP policy JSON: **identical**;
- persisted V2 session semantics: **identical** after excluding timestamps;
- wire-only difference: PostgreSQL 18/current serialized nanosecond fractional
  precision while the rollback reader emitted microsecond precision;
- prior artifact could read the record but, as expected from the discovered
  defect, could not load packs/snapshots for new work.

Observed request latencies on the fixed RC were 4 ms for session creation,
19 ms for workspace selection, 6–296 ms for the initial probe set, and
255–271 ms for model-backed in-pack no-match probes. A marker-bounded
PostgreSQL log window proved that the canonical ACP policy request executes
**zero database statements**. The control-plane metrics endpoint executes
**seven prepared read statements** and exposes distinct gate outcomes,
provenance/path, shadow divergence, envelope state, and write-attestation
breach categories.

## Verification commands and outcomes

- `cargo check -p bpmn-lite-server-designer --all-targets --locked` — pass.
- `cargo check -p ob-poc-web --all-targets --locked` — pass after temporarily
  removing and restoring the developer's ignored Cargo patch config.
- `scripts/build-designer-release-candidate.sh` — pass; image, labels, inspect
  receipt, and SBOM emitted.
- `scripts/build-release-candidate.sh` — pass for both the initial and repaired
  `ob-poc` RCs; final image, labels, inspect receipt, and SBOM emitted.
- BPMN PostgreSQL persisted replay — 3 pass, 0 divergence.
- BPMN rollback record read — pass.
- `ob-poc` ACP policy rollback comparison — pass, strict canonical equality.
- `ob-poc` persisted V2 session rollback comparison — pass, semantic equality.
- `ob-poc` control-plane metrics — pass only after applying the omitted tail
  migrations in the isolated database.
- `cargo fmt --all -- --check` in `ob-poc` — fail due broad pre-existing
  formatting drift outside the two edited files; no bulk formatting rewrite was
  made.

## Deployment findings and carry-overs

### P8-01 — external promotion inputs are absent

No registry, non-production deployment target, captured traffic source,
promotion policy, owner-approved tolerance, or dashboard destination is
declared. **Owner:** release/platform owner. **Target:** before Gate 8 sign-off.

### P8-02 — `ob-poc` cannot bootstrap a clean database through SQLx migrations

`cargo sqlx migrate run --source rust/migrations` applied migrations `000` and
`006`, then failed at `073` because `"ob-poc".entities` did not exist. The
canonical schema dump requires PostgreSQL 18 and pgvector; PostgreSQL 16 fails
on `transaction_timeout`, then `uuidv7()`. A supported bootstrap/migration
contract and tested database image are required. **Owner:** `ob-poc` persistence
owner. **Target:** next deployment-hardening release, before external shadow.

### P8-03 — canonical `ob-poc` schema artifacts have drifted

`migrations/master-schema.sql` and `schema_export.sql` are not byte-identical,
despite repository guidance. The canonical dump also omitted
`control_plane_audit`, envelope `entry_id`, shadow `execution_path`, and shadow
`decision_id`; the metrics endpoint returned HTTP 500 until migrations
`20260713_control_plane_audit`,
`20260713_control_plane_envelopes_entry_id`,
`20260713_control_plane_shadow_decisions_execution_path`, and
`20260714_control_plane_shadow_decisions_decision_id` were applied manually in
the isolated database. **Owner:** `ob-poc` schema owner. **Target:** same as
P8-02.

### P8-04 — the prior `ob-poc` RC is read-compatible but not new-work capable

The prior exact artifact reads current persisted sessions and returns the same
ACP policy, but does not contain runtime packs or snapshots. It is valid only as
a read/emergency rollback, not as a target for accepting new semantic sessions.
Retain the fixed RC and create the next rollback baseline from it after external
promotion. **Owner:** `ob-poc` release owner. **Target:** first external shadow.

### P8-05 — populated traffic replay remains required for `ob-poc`

The schema-only isolated database has no production client groups or semantic
pattern rows. It can prove startup, configuration loading, policy identity,
model identity, persisted-state readability, query count, and error typing; it
cannot qualify production candidate ordering or retrieval scores. **Owner:**
product/release owner. **Target:** external shadow with a redacted captured
traffic/database fixture.

### P8-06 — runtime warning debt remains visible

`ob-poc` reports missing handler metadata for a set of configured verbs,
safe-harbor harm-class gaps, several macro parse warnings, and a missing
EntityGateway YAML path. These predate the shared-crate release and were not
silenced. **Owner:** `ob-poc` application owners. **Target:** triage before
production promotion.

### P8-07 — operational dashboard gate is only partially met

`ob-poc` exposes structured control-plane gate/path/divergence/attestation
metrics. Neither repository declares the deployment dashboard that separates
contract-version, model, and host-adapter failures end to end. **Owner:**
observability/platform owner. **Target:** before Gate 8 sign-off.

## Safety and workspace integrity

All databases, networks, and containers used names beginning `phase8-` and
contained only disposable qualification data. No host database or external
environment was modified. The pre-existing changes in the coordinating
`bpmn-lite` checkout (`.DS_Store`, `bus_runtime.rs`, and training receipts) and
the pre-existing `ob-poc/.cargo/config.toml.example` modification were not
staged or committed.

## Promotion decision

The shared source release `v0.2.1` remains immutable and locally qualified.
The application artifacts are suitable for publication to an owner-selected
registry and for external shadow deployment. Production promotion is **not
authorised by this receipt**. Gate 8 can close after P8-01, P8-02/P8-03,
P8-05, and P8-07 have named environments and passing evidence.
