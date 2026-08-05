# Shared crates programme — `ob-poc` database bootstrap receipt

**Date:** 5 August 2026

**Status:** local persistence/deployment carry-over closed; external promotion still held

## Decision

Phase 8 findings P8-02 and P8-03, carried into Phase 9 as P9-02, are resolved
in `ob-poc` commit `1b852343de5fb1f4dc3e8029f02fabb8d76234aa`
(`build: make database bootstrap reproducible`). The commit is pushed on
`refactor/semantic-policy-consumer`.

Clean installation now has one supported contract: PostgreSQL 18 with
pgvector, bootstrapped from `migrations/master-schema.sql` through
`scripts/bootstrap-database.sh`. The `rust/migrations` directory remains an
incremental historical ledger and is explicitly not represented as a clean
SQLx baseline.

This closes local database reproducibility only. It does not supply the
external registry, deployment target, captured traffic, promotion tolerance,
or dashboard destination still required by Gate 8.

## Changes delivered

- Regenerated `migrations/master-schema.sql` after applying the omitted
  control-plane tail migrations and made `schema_export.sql` byte-identical.
- Added deterministic, single-export schema generation in
  `cargo x schema-export`, excluding SQLx test and bookkeeping objects.
- Added artifact drift checks and a fail-closed, single-transaction clean
  bootstrap command.
- Added a PostgreSQL 18 + pgvector CI gate that proves both clean installation
  and refusal to modify a non-empty database.
- Bound the schema SHA-256 into the release image label and release receipt.
- Updated the compose example to PostgreSQL 18 and the canonical snapshot.
- Deleted the stale PostgreSQL 17 `init_docker.sql`; it is recoverable from Git
  history if forensic comparison is needed.
- Documented the trusted-database requirement for the deterministic `pg_dump`
  restrict key and the upgrade/release discipline.

The canonical schema SHA-256 is
`03cf81f96ee59040bbb608c434be7cbf6e2e44344419c336679d96df09f43a5a`.

## Qualification receipt

An isolated `pgvector/pgvector:pg18` server was used. Results:

- The original empty-database SQLx path was reproduced: migrations `000` and
  `006` applied, then `073_entity_linking_support.sql` failed because
  `"ob-poc".entities` did not yet exist.
- Canonical bootstrap on PostgreSQL `180004` passed in one transaction.
- A second bootstrap against the same database failed closed with 563 user
  relations present.
- Bootstrap followed by schema export reproduced the exact canonical hash.
- The PG18 service-container form used by CI passed against another fresh
  database, and the required control-plane envelope contract was present.
- The production `ob-poc` image started against the clean schema, loaded 14
  packs, 1,253 verbs, model and snapshot inputs, and exposed the web server
  without missing-relation, database, or panic errors.
- `cargo check -p xtask --locked` passed.
- `cargo test -p xtask --locked` passed: 23 passed, 0 failed.
- `cargo check --workspace --all-targets --all-features --locked` passed.
- Targeted Rust formatting, Bash syntax, workflow YAML parsing,
  `git diff --check`, artifact identity, and required-contract checks passed.

Strict `cargo clippy -p xtask --all-targets --locked -- -D warnings` reached a
pre-existing `clippy::derivable_impls` warning in
`rust/src/graph/layout_v2.rs` for `EdgeLayoutConfig`. No unrelated application
source was changed to hide that debt.

## Release binding

`scripts/build-release-candidate.sh` passed and produced:

- image: `ob-poc:rc-1b852343de5f`;
- image ID:
  `sha256:7ea206b2ceba2b9e7a54a381e7305142314cf5a287bfa1d768f681697c744246`;
- size: 40,841,610 bytes;
- application revision:
  `1b852343de5fb1f4dc3e8029f02fabb8d76234aa`;
- shared DSL revision:
  `586431f81e2bb9101578af5167b8a35335f5a09e`;
- BPMN revision: `de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a`;
- schema SHA-256:
  `03cf81f96ee59040bbb608c434be7cbf6e2e44344419c336679d96df09f43a5a`;
- SBOM SHA-256:
  `b0c10cbc17ce39835a34fccfb4583843ea6939e8e9e7e6dd8256ca55779f5534`.

The receipt is at
`target/release-candidate/1b852343de5f/release-receipt.env` in the `ob-poc`
checkout.

## Remaining carry-overs

- Gate 8 external-promotion inputs remain absent (P8-01/P9-01).
- The clean snapshot does not qualify production retrieval ordering; populated,
  redacted traffic replay remains required (P8-05).
- Runtime macro/configuration warnings and missing embedding coverage remain
  visible and require application-owner triage (P8-06).
- The end-to-end deployment dashboard remains undeclared (P8-07).
- Phase 9 compatibility deletion remains blocked until both consumers ship and
  the rollback window closes.
- The strict Clippy warning is resolved by `ob-poc` commit `ccc14fa3`; broad
  formatting debt and the `block 0.1.6` future-incompatibility warning remain.

The pre-existing `.cargo/config.toml.example` edit in `ob-poc` and unrelated
dirty files in the coordinating `bpmn-lite` checkout were not staged or
committed.
