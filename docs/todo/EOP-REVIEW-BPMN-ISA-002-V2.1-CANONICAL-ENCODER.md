# Adversarial Review — V2.1 Canonical Binary Encoder (D3 Ring 2 hash domain)

**Scope reviewed:** `bpmn-lite-types/src/canonical.rs`, `bpmn-lite-types/src/persistence.rs`
(`PersistedSnapshotState::try_canonical_hash_bytes`, `SnapshotEnvelope::state_hash`).
**Reviewed against:** commit `992b001` on `main` ("ISA-002: V2 — canonical encoding & D3
integrity rings (full-supersession encoder)").
**Review mode:** authorship-blind — no design discussion consumed, diff read cold.
**Status:** this is the outstanding CAREFUL-tier second-reviewer pass flagged in
`EOP-PLAN-BPMN-ISA-002.md`'s V2 tranche gate ("Blind review of the complete composed
diff... is still outstanding"). This document discharges that flag with findings, not
a rubber stamp.

**Claim under test:** this encoder is the sole basis for the system's corruption-detection
integrity model — every downstream guarantee (frame hashing, journal chain verification,
replay divergence detection) assumes it produces identical bytes for identical logical
state, with no other source of nondeterminism. The review below treats every ambiguity
as a finding, not a benefit of the doubt.

---

## 1. Single encoder, no residual JSON in the hash path — VERIFIED

`SnapshotEnvelope::state_hash()` (`persistence.rs:183-191`) calls only
`self.state.try_canonical_hash_bytes()`, `.concurrency_table.to_canonical_bytes()`,
`.pending_effects.to_canonical_bytes()`, plus raw `u64`/`[u8;32]` bytes.
`PersistedSnapshotState::try_canonical_hash_bytes()` (`persistence.rs:106-125`) and
`ProcessInstance::try_canonical_hash_bytes()` (`canonical.rs:897-938`) contain zero
`serde_json` calls. Traced every store-layer caller of `.state_hash()`
(`store_postgres.rs:1183-1212, 2351, 2654, 5026-5027, 8398`) — all consume the returned
`[u8;32]`, none re-derive a hash via JSON. `serde_json` remains only in
`SnapshotEnvelope::decode`/`canonical_bytes` (`persistence.rs:161-176`) and
`JournalRecord`'s equivalent — the on-disk envelope wire format, outside the hash path
as claimed.

## 2. BTreeMap-only / no-HashMap enforcement — CONCERN

True today by inspection (`ProcessInstance.flags/counters/join_expected` are `BTreeMap`
at `types.rs:466-470`), but **not enforced** at type or lint level. `git log --all --
scripts/check-canonical-invariant.sh` shows this script — the mechanism that was
supposed to grep-guard exactly this invariant — was authored, then deleted, and **never
landed in a single commit**. No CI step, no `debug_assert`, no wrapper type prevents a
future `HashMap` swap. The encoder's own module doc (`canonical.rs:33-35`) states the
entire canonicity model rests on "`BTreeMap`/`BTreeSet` iteration is already sorted" —
a load-bearing assumption with zero enforcement. `write_seq(self.flags.iter(), ...)`
(`canonical.rs:916-919`) would compile identically and silently produce nondeterministic
bytes if `flags` were ever retyped to `HashMap`. Same gap applies to `serde_json::Map`'s
`preserve_order`-off assumption (`canonical.rs:832-834`) — true only because no crate in
the workspace enables that feature (confirmed via `Cargo.lock`'s absence of `indexmap`
under `serde_json`), also unenforced.

## 3. Explicit enum tag scheme — VERIFIED

Every enum (`RecordKind`, `RecordState`, `Value`, `WaitState`, `ProcessState`,
`ErrorClass`, `SessionWorkspaceKind`, and the JSON `Value` shim) encodes via
hand-written `match` with hardcoded `u8` literals and a documented tag table in a
preceding doc comment (e.g. `canonical.rs:411, 449-450, 547-549, 623, 727`). No
`#[repr]`/derive-discriminant is used anywhere for wire encoding.

## 4. Order-independence — VERIFIED, contingent on #2

All genuinely-unordered collections (`ConcurrencyTable`, `flags`, `counters`,
`join_expected`, `fibers`, `incidents`, `join_counts`, `members: BTreeSet`) go through
`BTreeMap`/`BTreeSet` iteration. The `Vec`-typed fields that preserve insertion order
(`Fiber.stack`, `Fiber.regs` (fixed array), `Fiber.control_stack`,
`SessionStackState.workspace_stack`) are all semantically ordered structures (execution
stack, register file, control stack, frame stack) where order-sensitivity is correct,
not accidental. No unordered `Vec` found. This holds only as long as #2's unenforced
assumption keeps holding.

## 5. Float → f64 → IEEE-754 bit pattern — VERIFIED

`encode_canonical_json`, `canonical.rs:851-852`:
`w.write_u64(n.as_f64().unwrap_or(0.0).to_bits())`. Actually implemented and exercised
by `domain_payload_with_ordinary_float_is_accepted_and_deterministic_across_text_forms`
(`canonical.rs:1112-1125`), which proves `1.0`/`1.00`/`1e0` converge to identical bytes.

## 6. NaN/Infinity rejection — CONCERN

The rejection is **not enforced at the point of encoding**. `encode_canonical_json`
(`canonical.rs:836`) performs no finiteness check at all — its doc comment states it
"assumes pre-validated input and does not re-check" (`canonical.rs:835`). The guarantee
exists only because every current call site happens to run `validate_finite_json` first,
by hand, a few lines earlier in `try_canonical_hash_bytes` (`canonical.rs:900-906`). But
`encode_canonical_json` is `pub fn` in a `pub mod canonical` (`lib.rs:18`) with no
re-export gate — any caller, anywhere in the crate or workspace, can call it directly on
unvalidated JSON and produce a "canonical" encoding of a non-finite float with zero
compile-time or run-time protection. There is no `Validated<Value>` wrapper or private
constructor forcing the check to have happened; it's a doc-comment convention only.
Separately: the fallback `unwrap_or(0.0)` at line 852 is itself a silent substitution —
if `Number::as_f64()` ever returns `None` for a legitimately-constructed number (e.g. a
future `arbitrary_precision` big-int), this code writes `0.0` instead of erroring,
silently corrupting the hash rather than rejecting. The specific `1e400` bypass this was
built for is confirmed unreachable via `serde_json`'s public API today (proven by
`domain_payload_with_huge_exponent_is_a_typed_parse_error_not_silent_infinity`,
`placeholder_values_cannot_carry_a_non_finite_float_via_serde_json_public_api`) — but
that just means the *only* currently-provable protection against non-finite floats
reaching the hash is `serde_json`'s own parser behavior, not this encoder's stated
contract.

## 7. Round-trip law as property test — CONCERN, coverage gap

`proptest_round_trip` (`canonical.rs:1177-1265`) exists and is real — but it covers
**only** `ConcurrencyTable`/`ConcurrencyRecord`/`RecordKind`/`RecordState`, the
pre-existing V1.2-era surface. None of the types added in this diff — `Value`,
`WaitState`, `ProcessState`, `ErrorClass`, `Incident`, `Fiber`, `SessionScopeState`,
`SessionWorkspaceKind` — have a `proptest!` generator. `Value` gets a 6-example
fixed-list test (`value_round_trips_byte_identically`, not a property test).
`Fiber::canonical_decode` — the most structurally complex new impl, including the
fixed-array `try_into` fallback branch its own comment calls "cannot actually fail"
(`canonical.rs:703-711`) — is **never called by any test, anywhere in the repository**
(confirmed by grep: no test constructs a `Fiber`, encodes it standalone, and decodes it
back). Same for `WaitState::canonical_decode`, `ProcessState::canonical_decode`,
`Incident::canonical_decode`, `SessionScopeState::canonical_decode`,
`SessionWorkspaceKind::canonical_decode` — zero standalone round-trip tests. The one
place `Fiber`'s encode path is exercised
(`golden_bytes_process_instance_and_fiber_canonical_hash_domain` in `persistence.rs`)
only calls the fallible **encode-only** `try_canonical_hash_bytes` composition — it
never calls `Fiber::canonical_decode` at all, since that path is one-way. The round-trip
law is therefore asserted by doc comment for most of this diff's actual surface, not
tested.

## 8. Golden-bytes fixtures — VERIFIED for what exists, CONCERN on coverage

Fixtures are real, exact, and diffed in CI: `golden_bytes_concurrency_table` (hardcoded
byte vec, `canonical.rs:1011-1051`), `golden_bytes_value_variants`
(`canonical.rs:1055-1070`), and `golden_bytes_process_instance_and_fiber_canonical_hash_domain`
(`persistence.rs`, `include_bytes!` against a committed `.bin`). `cargo test --workspace`
runs in `layering.yml:55`, `nightly-chaos.yml:44`, `production-gates.yml:54` — genuinely
wired into CI, not illustrative-only. But the one `ProcessInstance`/`Fiber` fixture uses
a degenerate sample (`WaitState::Running`, `ProcessState::Running`,
`SessionStackState::default()` with `scope: None, active_workspace: None`) — it
exercises **one** point in the variant space and never golden-byte-locks
`WaitState::Timer/Msg/Job/Effect/Join/Race/Incident`,
`ProcessState::Completed/Cancelled/Terminated/Failed/WaitingOnSubmission/WaitingOnInvocation`,
`ErrorClass::*`, or a populated `SessionScopeState`/`SessionWorkspaceKind`. A tag-byte or
field-order regression in any of those unexercised variants would not be caught by any
committed fixture.

## 9. Panics/unwraps reachable from adversarial input — VERIFIED clean on the
   decode-bounds-checked path; CONCERN on the untested branch

`CanonicalReader::read_u64`/`read_bytes_fixed` (`canonical.rs:174, 190`) contain
`.unwrap()` on `try_into()`, but both are provably safe: `take(n)` (`canonical.rs:149-156`)
bounds-checks and returns `Err` before either unwrap can execute, so length is
guaranteed. All decode entry points return `Result`, never panic on truncated/malformed
bytes (confirmed by `truncated_bytes_are_a_typed_decode_error_not_a_panic`,
`unknown_record_kind_tag_is_a_typed_decode_error`). However, `Fiber::canonical_decode`'s
`regs_vec.try_into()` fallback (`canonical.rs:708-711`) is claimed infallible by comment
but is — per finding #7 — never exercised by any test, so that claim is unverified, not
proven. Two unrelated truncation risks, both silent rather than panicking: `write_seq`/
`write_bytes`/`encode_canonical_json`'s Object branch cast `usize as u32` for lengths
(`canonical.rs:82, 101, 865`) with no bounds check — a collection or string exceeding
`u32::MAX` silently wraps rather than erroring, a latent (if practically unreachable
today) non-injectivity risk in a system whose entire correctness claim is "identical
logical state ⇒ identical bytes, no exceptions."

---

## Summary

Items 1, 3, 5 are solid. Items 4 and 8 are correct in what they cover but rest on
unverified/unenforced assumptions (2) or incomplete variant coverage. Items 2, 6, 7, and
9 are real gaps: the BTreeMap invariant this whole design depends on has no enforcement
mechanism at all (and the one that was built was deleted before ever being committed),
the NaN/Infinity guarantee is caller-discipline rather than encoder-enforced with a
public bypass, and the round-trip law — the core claim of this module — is untested for
the majority of what this diff actually added, including the one type (`Fiber`) whose
decode path has genuinely nontrivial logic.

## Recommended disposition (not yet actioned — for Adam's review)

1. **2 (BTreeMap enforcement):** re-author and land a lint (the deleted
   `check-canonical-invariant.sh` or a `syn`-based proc-macro check) as an actual CI
   step before this module is relied on by V3+. It was proposed once and dropped before
   commit — close that loop rather than re-opening it later.
2. **6 (NaN/Infinity):** either make `encode_canonical_json` fallible and check
   finiteness inline (removing the caller-discipline dependency), or make it
   `pub(crate)`/private and force all external construction through a validating
   constructor. Replace `unwrap_or(0.0)` with a typed error on `as_f64() == None`.
3. **7 (round-trip coverage):** add `proptest!` generators for `Value`, `WaitState`,
   `ProcessState`, `ErrorClass`, `Incident`, `Fiber`, `SessionScopeState`,
   `SessionWorkspaceKind` — at minimum a standalone `Fiber` round-trip test, since it is
   currently the only nontrivial decode path with zero test executions of any kind.
4. **8 (golden-bytes coverage):** extend the `ProcessInstance`/`Fiber` fixture (or add
   siblings) to cover at least one non-default value per enum variant.

This document is a review artifact, not an implementation task list — items 1-4 above
should be turned into explicit V2.1-follow-up tranche items (or a V2.1h) if Adam
concurs, rather than actioned silently.
