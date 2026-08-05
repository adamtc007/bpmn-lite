# Shared crates programme — `ob-poc` strict Clippy receipt

**Date:** 5 August 2026

**Status:** strict workspace Clippy restored; formatting and dependency carry-overs remain

**Subsequent closure:** the `block 0.1.6` carry-over was removed in shared
release `v0.2.2`; see
`shared_crates_v0_2_2_candle_release_receipt_2026-08-05.md`. The formatting
carry-over remains.

## Decision

The `ob-poc` strict lint gate is green at commit
`ccc14fa37d7c6abdd3c1f621577e848835c36892` on
`refactor/semantic-policy-consumer`. The commit is pushed.

This closes the strict-Clippy part of P9-09. It does not close the broad
pre-existing formatting baseline or Cargo's future-incompatibility warning for
transitive `block 0.1.6`.

## Changes

The tranche removed 24 Clippy findings across production and test code,
including:

- a derived `Default` implementation and idiomatic map/value iteration;
- allocation-free test arrays, `sort_by_key`, `to_vec`, and direct map-key
  checks;
- clearer panic paths that retain the failing verb FQN and error;
- equivalent monotonic counter assertions without `+ 1` arithmetic;
- focused placement allowances for intentionally colocated test modules; and
- replacement of synchronous environment mutexes held across database awaits
  with Tokio mutexes in both control-plane test modules.

The async-lock change preserves process-global environment serialization while
avoiding a blocking `std::sync::MutexGuard` across an await point.

## Verification

- `cargo check --workspace --all-targets --all-features --locked` — pass.
- Strict workspace Clippy over all targets/features with `-D warnings` — pass.
- KYC substrate slice — 19 passed, 0 failed.
- Full `ob-poc` library suite — 1,818 passed, 0 failed, 214 explicitly ignored
  by their test declarations.
- Domain-pack configuration qualification — 8 passed, 0 failed.
- KYC M3 remediation — 7 passed, 0 failed.
- KYC verb coverage — 16 passed, 0 failed.
- `ob-poc-web` startup decision tests — 5 passed, 0 failed.
- `git diff --check` — pass.

The exact-Git checks temporarily disabled and restored the checkout-local Cargo
patch configuration. No local path entered the committed dependency graph.

## Remaining hygiene carry-overs

- `cargo fmt --all -- --check` remains red on a broad pre-existing baseline
  (7,704 reported diff lines). No repository-wide formatting rewrite was mixed
  into this behavioral/lint commit.
- Cargo still warns that transitive `block 0.1.6` contains code rejected by a
  future Rust version. Its dependency chain and upgrade path require a separate
  compatibility review.

The pre-existing `.cargo/config.toml.example` modification in `ob-poc` and all
unrelated dirty files in the coordinating `bpmn-lite` checkout remain
untouched.
