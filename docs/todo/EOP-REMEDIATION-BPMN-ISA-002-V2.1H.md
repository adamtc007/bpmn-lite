# Remediation Report — Tranche V2.1h (Encoder Remediation)

**Source finding:** `EOP-REVIEW-BPMN-ISA-002-V2.1-CANONICAL-ENCODER.md` (adversarial blind
review of commit `992b001`).
**Disposition:** Adam, 2026-07-21 — three findings (#2, #6, #7) declared BLOCKING for V3
entry; two (#8, #9) declared cleanup. Full text of the disposition is reproduced in
`docs/todo/EOP-PLAN-BPMN-ISA-002.md`'s Tranche V2.1h section. After confirmation
re-review of h.1–h.5, Adam added **h.6** (BLOCKING) to structurally close the NEW
FINDING this report originally left as a documented-but-unenforced residual risk — see
that section below for why, and for a correction to this report's own earlier claim.
**Status:** all six items closed. **Not yet committed** — sitting uncommitted on top of
`992b001` on `main`'s working tree, held for the confirmation re-review requested before
V3 begins. This document exists so that re-review, and it does not need to re-derive the
diff from scratch.

**How to use this document:** each section names the original finding, states the claim
this remediation makes, and gives file/line/test-name evidence a reviewer can check
directly rather than take on faith — the same standard the original review applied. h.6
below is flagged **NEW FINDING, THEN CORRECTED** — h.2's original NEW FINDING claimed
"zero `serde_json::to_value` call sites in the workspace," which was **false**; building
h.6's guard found six live ones. That correction is documented in place, not silently
fixed.

---

## h.1 — Finding #2 (BTreeMap invariant unenforced) — CLOSED

**Original finding:** the canonicity model depends entirely on `BTreeMap`/`BTreeSet`
sorted iteration, with no lint or type-level enforcement; the one lint that was supposed
to guard it (`check-canonical-invariant.sh`) was authored and then deleted before it was
ever committed.

**Remediation:** `scripts/check-canonical-invariant.sh` (new file, committed this time).

- Extracts the brace-delimited body of each hash-domain type (`Value`, `WaitState`,
  `ProcessState`, `ErrorClass`, `Incident`, `Fiber`, `ProcessInstance`,
  `SessionStackState`, `SessionScopeState`, `SessionWorkspaceKind`, `ConcurrencyRecord`,
  `ConcurrencyTable`, `RecordKind`, `RecordState`, `RecordCounters`,
  `PersistedSnapshotState`) via brace-depth-tracked `awk`, then greps for
  `\bHashMap\b|\bHashSet\b`. Scoped extraction avoids false-positiving on unrelated
  `HashSet` usage elsewhere in the same files (e.g. `ArtifactMetadata::write_set`).
- Separately checks `serde_json`'s own `[[package]]` stanza in `Cargo.lock` for an
  `indexmap` dependency — the `preserve_order` tell. (First draft of this check grepped
  the whole lockfile for `indexmap` and false-positived on `petgraph`/`sqlx-core`/`rkyv`/
  `h2`/`serde_yaml`/`toml_edit`/`tower`, which depend on `indexmap` independently of
  `serde_json` — corrected to scope to `serde_json`'s own dependency list specifically.)
- Self-test fixture: `scripts/fixtures/canonical_invariant_violation.rs` — a `Fiber`
  struct with a deliberately injected `HashMap<u32, u32>` field.

**Verification a reviewer can re-run:**
```
bash scripts/check-canonical-invariant.sh --self-test   # expect: OK — lint correctly fired
bash scripts/check-canonical-invariant.sh                # expect: OK — no violations
```
Wired into `.github/workflows/layering.yml`'s `guard` job as two steps (self-test, then
real scan), immediately after the existing glossary-guard steps.

---

## h.2 — Finding #6 (NaN/Infinity enforced at caller, not encoder) — CLOSED

**Original finding:** `encode_canonical_json` was `pub`, performed no finiteness check,
and documented that it trusted a separate `validate_finite_json` pre-pass the caller was
responsible for invoking — a bypass any current or future caller could exploit or simply
forget. Separately, `unwrap_or(0.0)` on the `as_f64()` fallback was itself a silent
substitution.

**Remediation:** `bpmn-lite-types/src/canonical.rs`.

- `encode_canonical_json`'s signature changed from `fn(...)` to
  `fn(...) -> Result<(), CanonicalJsonError>`; the finiteness check
  (`f.is_finite()`) now runs inline, in the same function that writes the bytes, not in a
  separate function a caller could skip.
- `validate_finite_json` — the old two-pass design's pre-check function — is **deleted
  outright**, not merely superseded. There is no longer a "the check ran somewhere else"
  story to audit; there is one function, and it cannot produce non-finite-float bytes
  without returning `Err`.
- `unwrap_or(0.0)` deleted; replaced with
  `n.as_f64().ok_or(CanonicalJsonError::NonFiniteFloat)?` followed by an explicit
  `is_finite()` check, both typed-error paths.
- `CanonicalWriter::write_option` takes an infallible closure
  (`impl FnOnce(&mut Self, &T)`), so it can no longer carry the now-fallible
  `encode_canonical_json` for `ProcessInstance::placeholder_values`. That one field's
  presence tag (`0x00`/`0x01`) is now written by hand with the same wire shape, and the
  `Some` arm propagates `?`. `encode_session_stack` was similarly converted to return
  `Result` and hand-rolls its `workspace_stack` length/loop instead of using the
  (infallible) `write_seq`.

**Verification a reviewer can re-run:**
```
cargo test -p bpmn-lite-types canonical::tests::encode_canonical_json_is_callable_standalone_and_self_validating
cargo test -p bpmn-lite-types canonical::tests::domain_payload_with_huge_exponent_is_a_typed_parse_error_not_silent_infinity
cargo test -p bpmn-lite-types canonical::tests::placeholder_values_cannot_carry_a_non_finite_float_via_serde_json_public_api
```

**NEW FINDING (discovered while remediating, not part of the original review):**
`serde_json::to_value(f64::NAN)` does **not** return `Err` — it silently returns
`Ok(Value::Null)`. Same for `f64::INFINITY`/`f64::NEG_INFINITY`. This is a *different*
bypass from the one the plan document originally hypothesized (a huge exponent like
`1e400` silently becoming `f64::INFINITY` at parse time — confirmed false; `serde_json`
rejects that at parse time with a real `Err`). The `to_value` bypass is structurally
uncatchable by anything in `canonical.rs`: by the time a `Value::Null` produced this way
reaches `encode_canonical_json`, the information that it was ever a NaN is already gone
— a `Value::Null` from `to_value(NaN)` is bit-for-bit indistinguishable from an
intentional `null` in the source data.

Grep-verified: **zero** call sites of `serde_json::to_value` exist anywhere in this
workspace as of this remediation. `ProcessInstance::domain_payload` is populated only via
`serde_json::from_str` (text parsing); `placeholder_values` is populated only by cloning
an already-parsed sub-`Value` out of `domain_payload`'s own parse tree
(`ProcessInstance::bind_placeholder_from_payload`, `types.rs`) — never by serializing a
native `f64`. So this bypass is currently dead, not live. It is recorded in code as a
doc-comment on `serde_json_to_value_silently_coerces_non_finite_floats_to_null_not_an_error`
(test) and as a comment on the `CanonicalJsonError` variant docs, flagged for whichever
future code path is first to construct `placeholder_values` (or any other hash-domain
JSON field) from a native float via `to_value`/`#[derive(Serialize)]`.

**Recommendation for the reviewer:** decide whether this residual risk needs a standing
guard now (e.g. a lint banning `serde_json::to_value` calls that could reach
`ProcessInstance`'s JSON fields) or whether "currently unreachable, grep-verified,
documented" is sufficient given the greenfield/no-production-instances context. Not
actioned unilaterally here — this is exactly the kind of fork the working contract says
to surface, not decide.

---

## h.3 — Finding #7 (round-trip property coverage gap) — CLOSED

**Original finding:** `proptest_round_trip` covered only `ConcurrencyTable` and its
substructures (the pre-existing V1.2-era surface). None of `Value`, `WaitState`,
`ProcessState`, `ErrorClass`, `Incident`, `Fiber`, `SessionScopeState`,
`SessionWorkspaceKind` had property-based round-trip coverage; `Fiber::canonical_decode`
in particular had **zero** test executions of any kind anywhere in the repository,
despite being the only nontrivial decode path in the module (a fixed-size `[Value; 8]`
array reconstruction with a `try_into` fallback branch asserted infallible only by
comment).

**Remediation:** new module `proptest_round_trip_v2_1h` in `canonical.rs`, with an
`arb_*` generator and a `canonicalize(decode(b)) == b` proptest for each of the eight
types named above.

**Verification a reviewer can re-run:**
```
cargo test -p bpmn-lite-types canonical::proptest_round_trip_v2_1h::
```
Expected: 8 tests, all passing, including `fiber_round_trip_is_a_fixed_point` — the
first test in the repository's history to invoke `Fiber::canonical_decode`.

---

## h.4 — Finding #8 (golden-bytes variant coverage gap) — CLOSED (cleanup)

**Original finding:** the one `ProcessInstance`/`Fiber` golden-bytes fixture used only
default/degenerate field values (`WaitState::Running`, `ProcessState::Running`, no
session scope) — a tag-byte or field-order regression in any other variant would not
have been caught by any committed fixture.

**Remediation:** six new golden-bytes tests in `canonical.rs`, each hand-verified against
the encoder's documented tag scheme and hardcoded as an exact byte vector (same style as
the pre-existing `golden_bytes_concurrency_table`/`golden_bytes_value_variants`):
`golden_bytes_wait_state_non_default_variants` (all 7 non-`Running` `WaitState` variants),
`golden_bytes_process_state_non_default_variants` (all 6 non-`Running` `ProcessState`
variants), `golden_bytes_error_class_variants` (all 3), `golden_bytes_populated_session_types`
(populated `SessionScopeState` + `SessionWorkspaceKind::Bpmn`), `golden_bytes_incident`
(a fully-populated `Incident`).

**Verification a reviewer can re-run:**
```
cargo test -p bpmn-lite-types canonical::tests::golden_bytes
```

---

## h.5 — Finding #9 (unchecked `usize as u32` length casts) — CLOSED (cleanup, scoped)

**Original finding:** `write_seq`/`write_bytes`/`encode_canonical_json`'s Object branch
cast collection/string lengths to `u32` unchecked; a length exceeding `u32::MAX` would
silently wrap instead of erroring — latent non-injectivity.

**Remediation, and its explicit scope limit:**

- Added `CanonicalJsonError::LengthOverflow { kind, len }` and a `checked_len_u32` helper.
  Applied at every length-prefix site in the JSON encoding path — `encode_canonical_json`'s
  `String`/`Array`/`Object` branches (including object *keys*) and `encode_session_stack`'s
  `workspace_stack` — all of which were already made `Result`-returning by h.2, so this
  closes with zero additional blast radius.
- `CanonicalWriter::write_bytes` and `write_seq` — the shared, **infallible** methods used
  by essentially every `CanonicalEncode` impl in the file (~30 call sites) — were
  deliberately **not** made fallible. Doing so would require making the
  `CanonicalEncode::canonical_encode` trait method itself fallible, a trait-wide
  signature change cascading through ~15 impls, for a condition that requires already
  holding a multi-gigabyte `Vec`/`String` in memory to trigger. `debug_assert!` was added
  to both instead, and the scope decision is documented in-code on each method rather than
  silently left as an unaddressed gap.

**Verification a reviewer can re-run:**
```
cargo test -p bpmn-lite-types canonical::tests::checked_len_u32_rejects_lengths_beyond_u32_max
```
(A test allocating an actual `u32::MAX + 1`-length `Vec`/`String` to exercise
`write_bytes`/`write_seq`'s `debug_assert!` directly was judged not worth the multi-GB
test-suite cost; the pure-function test above exercises the identical bounds-check logic
without the allocation.)

---

## h.6 — New finding from h.2 remediation (`serde_json::to_value` non-finite coercion) — CLOSED (BLOCKING, added by Adam after confirmation re-review)

**Origin:** h.2's remediation discovered, but did not close, that `serde_json::to_value`
silently coerces `NaN`/`Infinity`/`-Infinity` to `Value::Null` instead of returning
`Err` — structurally distinct from the (correctly-closed) `1e400`-parses-to-`Infinity`
premise, and uncatchable by any check inside `encode_canonical_json`, because the NaN
identity is already erased before a `Value` exists. That section's original text
reasoned this was acceptable to leave as a documented residual risk, since it was
"grep-verified... zero `serde_json::to_value` call sites in the workspace."

**Disposition:** on confirmation re-review, Adam overruled that judgment call —
"dead by grep, documented" is a process guarantee, and the entire reason V2.1 was
rescoped to full JSON supersession over the phased hybrid was to convert process
guarantees on the integrity layer into structural ones. Directed: land a standing guard
now, using the same self-test-fixture machinery as h.1, in `check-canonical-invariant.sh`
(already parsing the hash-domain types, already wired into CI).

**Correction to this report's own earlier claim:** building the guard immediately
falsified "zero `serde_json::to_value` call sites in the workspace" — that grep, as
originally run, was not broad enough. `bpmn-lite-server/src/rest.rs` had **six** live
occurrences of exactly the risky shape:
```rust
inst.placeholder_values = serde_json::to_value(&pv).ok();
```
in the REST demo's plan-execution fork/join/loop handling (`update_placeholder`-shaped
call sites around lines 620, 643, 649, 675, 681, 850 pre-fix) plus one at line 463
(`Some(serde_json::to_value(&initial_variables)?)`, in `create_instance`). On inspection
these particular six were not float-risky in practice either (their inputs are
`HashMap<String, serde_json::Value>` — already-`Value`-typed maps being re-serialized
through `to_value`, not raw `f64` — so `to_value`'s float-serialization path is never
actually invoked), but that "safe by nested-type reasoning" argument is exactly the kind
of per-call-site manual audit this remediation exists to replace with a mechanical
guarantee. All six were rewritten to construct `serde_json::Value::Object` directly from
the already-validated map (`initial_variables.into_iter().collect()`,
`updated_pv.into_iter().collect()`, `pv.into_iter().collect()`) — provably safe by
construction (no `Serialize`/float-formatting logic invoked at all), and simpler/cheaper
than the round-trip through the generic serializer they replaced.

**Remediation:** `scripts/check-canonical-invariant.sh`, new function
`check_to_value_near_hash_domain_fields`. Scans the same ISA-relevant crate directories
`check-glossary.sh` uses; for every `serde_json::to_value` occurrence, checks a ±3-line
window for an actual field-access/assignment/binding shape
(`\.domain_payload\b|\.placeholder_values\b|\bdomain_payload[:=,]|\bplaceholder_values[:=,]`)
— deliberately not a bare-word match, since a naive version of this check false-positived
against this very script's own target file's doc comments (which discuss
`domain_payload`/`placeholder_values` extensively in prose without constructing either).
Self-test fixture: `scripts/fixtures/canonical_invariant_to_value_violation.rs`
(a function assigning `serde_json::to_value(&pv)` into `inst.placeholder_values`, the
exact `rest.rs` shape).

**Explicitly scoped, not a claim of complete dataflow coverage:** this is a textual
proximity heuristic, the same class of guard as h.1's type-body extraction and the
pre-existing `check-glossary.sh` — it catches the concrete pattern found live and guards
against its reintroduction; it does not prove no bypass exists three function calls away
from a `domain_payload`/`placeholder_values` reference. That limitation is documented
in the script itself, not glossed over.

**Verification a reviewer can re-run:**
```
bash scripts/check-canonical-invariant.sh --self-test   # expect: both guards fire OK
bash scripts/check-canonical-invariant.sh                # expect: OK, clean against real source (post rest.rs fix)
cargo build -p bpmn-lite-server                           # expect: clean (rest.rs rewrite compiles)
```

---

## Full-suite verification (all six items applied together)

```
cargo build --workspace                                                   # clean
cargo test --workspace -- --test-threads=1                                # 103/103 test binaries, 0 failures
cargo clippy --workspace --all-targets --all-features -- -D warnings      # clean
bash scripts/check-canonical-invariant.sh --self-test && bash scripts/check-canonical-invariant.sh
bash scripts/check-glossary.sh --self-test && bash scripts/check-glossary.sh
bash scripts/check-layering.sh
```
All green as of this report. Diff is uncommitted on `main`'s working tree, on top of
`992b001`.

## Open items for the reviewer

None outstanding as judgment calls — h.6 closed the one item (h.2's NEW FINDING) this
report originally left as a flagged-but-unresolved risk. Everything in this report,
including h.6, is now a closed, tested, re-runnable claim.
