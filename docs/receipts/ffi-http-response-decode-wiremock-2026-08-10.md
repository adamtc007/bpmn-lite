# HttpFfiOwner response-body decode — wiremock integration suite

Date: 2026-08-10

Scope: closes the second item Adam asked for alongside CI smoke parity —
real integration coverage for `HttpFfiOwner::invoke`'s live HTTP
response-body decode/error-mapping path, the one thing the FFI
fuzz-coverage tranche deliberately left out (a mock-server round-trip per
libFuzzer iteration would be impractically slow for sustained in-process
fuzzing; see `docs/receipts/fuzz-coverage-ffi-owner-metadata-2026-08-10.md`).

## A second real bug, found by code review before writing any test

Before writing the wiremock suite, read the code this suite would exercise
(`bpmn-lite-ffi-http/src/owner.rs::invoke`, its full ~150-line
response-handling body) end to end. Found `body_excerpt` (used to build
`Incident` messages on non-success status codes):

```rust
fn body_excerpt(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > 256 {
        format!("{}...", &s[..256])   // <- raw byte-index slice
    } else {
        s.into_owned()
    }
}
```

**Same bug class as the `dmn-lite-parser` lexer panic fixed earlier this
session**: `&s[..256]` slices at a raw byte index, which panics whenever
byte 256 lands inside a multi-byte character. Confirmed with a standalone
repro (253 ASCII bytes + a 4-byte `𝄞` straddling the boundary) before
touching production code — real, not speculative.

This one is more severe than the parser bug in one respect:
`FfiExecutionOwner::invoke`'s own documented contract
(`ffi-types/src/owner.rs`) states *"The owner MUST NOT panic; any error
must be reported via `FfiResult::Incident`."* — a documented invariant this
function was already violating on any upstream HTTP service (or a
misbehaving proxy, gateway, or a compromised counterparty) that returns an
error body with a multi-byte character straddling byte 256. It's on the
error-status path (4xx/5xx), so the trigger is exactly the kind of
response most likely to come from an unhealthy or hostile external
service, not routine traffic.

**Fix**: walk back from the byte cap to the nearest real char boundary
(`while !s.is_char_boundary(end) { end -= 1; }`) before slicing, same
approach as the lexer fix. Landed as its own commit
(`6a5084b1c187866f5aefeeaf5e2ec293c1b6f8b8`) with two direct unit tests in
`owner.rs`'s own (new) `#[cfg(test)]` module — red->green proven by
temporarily reverting just the fix and confirming both new tests panic
with the exact original backtrace, then restoring it.

## The wiremock integration suite

`bpmn-lite-ffi-http/tests/response_decode.rs` (new, 12 tests) — `wiremock`
was already a declared dev-dependency, unused anywhere in the crate before
this. Also added `uuid = { workspace = true }` as a dev-dependency
(needed for `Uuid::new_v4()` in test fixtures; wasn't previously declared
here at all).

Drives the real `HttpFfiOwner::invoke` (real `reqwest` client, real HTTP
round-trip to a local `wiremock::MockServer`) across the full response
decision tree in `owner.rs`:

- **Success**: valid JSON object body -> `FfiResult::Success` with the
  correct `output_payload`.
- **NoMatch**: empty body, `"null"` body, `"{}"` body — all three of
  `invoke`'s distinct no-match triggers.
- **Incident/ContractViolation**: non-object JSON body (array) on success
  status; malformed/non-JSON body on success status (never panics); plain
  400.
- **Incident/BusinessRejection**: 404 -> `HTTP_NOT_FOUND`, 409 ->
  `HTTP_CONFLICT`.
- **Incident/Transient**: 500 with `retry_hint_ms: Some(1000)`.
- **The `body_excerpt` regression, through the real HTTP path, not just
  the isolated unit test**: a 400 response whose body has a multi-byte
  character straddling the 256-byte excerpt boundary ->
  `Incident/ContractViolation`, never panics.
- A second boundary case with genuinely invalid UTF-8 (a lone leading byte
  of a 2-byte sequence, truncated) on a 500 response.

Both boundary tests were red->green proven the same way as the unit
tests: temporarily reverted just the `body_excerpt` fix, reran
`incident_never_panics_on_error_body_straddling_the_excerpt_boundary`,
confirmed it panics with the identical backtrace through the real HTTP
round-trip, then restored the fix and reconfirmed all 12 tests pass.

## Verification

- `cargo test -p bpmn-lite-ffi-http --test response_decode`: 12 passed, 0
  failed.
- `cargo test -p bpmn-lite-ffi-http --lib`: 9 passed, 0 failed (5
  pre-existing `template::tests::*` + 4 new `owner::tests::*`).
- Red->green proven twice: once on the direct unit test (prior commit),
  once again here through the real HTTP round-trip via `wiremock`.
- `cargo check --workspace --all-targets --all-features`: clean.
- `git status --porcelain`: touches `bpmn-lite-ffi-http/Cargo.toml` (one
  new dev-dependency line) and the new `bpmn-lite-ffi-http/tests/` file.

## What this does not do

- Does not add an equivalent wiremock suite for `bpmn-lite-ffi-grpc` — its
  owner (`bpmn-lite-ffi-grpc/src/owner.rs`) uses `tonic`'s typed gRPC
  status codes, not raw HTTP response bodies with the same
  `String::from_utf8_lossy` + fixed-byte-slice pattern that caused this
  bug; a quick read of that file found no analogous raw byte-slicing.
  Not audited as exhaustively as `bpmn-lite-ffi-http` was here — a
  candidate for a future pass if warranted, not ruled out, just not done.
- Does not attempt to also fuzz this response-decode path (still correctly
  out of scope per the earlier receipt) — this is the integration-test
  answer to that gap, a distinct and now-complete instrument, not a
  partial substitute for it.
