#!/usr/bin/env bash
# check-canonical-invariant.sh — D3 Ring 2 hash-domain determinism guard
# (EOP-PLAN-BPMN-ISA-002 V2.1h.1/h.6; re-authored after the original script
# was written but never committed — see the V2 tranche record's "Superseded"
# note and the V2.1 blind review's finding #2).
# Run from the repo root (bpmn-lite/).
#
# The entire canonical binary encoder (`bpmn-lite-types/src/canonical.rs`)
# rests on one assumption, stated in that module's own doc comment: "caller
# sorts first for anything that must be canonical over an unordered
# collection — BTreeMap/BTreeSet iteration is already sorted." Nothing in
# the type system enforces that assumption. If any hash-domain structure's
# map/set field is ever retyped from BTreeMap/BTreeSet to HashMap/HashSet,
# the code keeps compiling — `.iter()` exists on both — and the encoder
# silently starts producing nondeterministic bytes for identical logical
# state, corrupting every downstream integrity guarantee (frame hashing,
# journal chain verification, replay divergence detection) without any
# build or test failure to announce it.
#
# This is an ENFORCED INVARIANT, not a one-time check: a future field
# addition to any of these types must not silently reintroduce a HashMap.
#
# Unlike a whole-file grep, this extracts just the *body* of each named
# type (brace-depth tracked) so it doesn't false-positive on unrelated
# types in the same file that legitimately use HashMap/HashSet outside the
# hash domain (e.g. `ArtifactMetadata::write_set: BTreeMap<String,
# HashSet<FlagKey>>` in types.rs — a write-set index, not part of
# `ProcessInstance`/`Fiber`/the concurrency table, and never encoded by
# `canonical.rs`).
#
# Usage:
#   check-canonical-invariant.sh              scan the real source tree
#   check-canonical-invariant.sh --self-test  scan the deliberate-violation
#                                              fixture and fail unless it fires
set -uo pipefail

# file:TypeName pairs for every struct/enum whose fields the canonical
# encoder (bpmn-lite-types/src/canonical.rs) walks to produce Ring 2
# hash-domain bytes, per its CanonicalEncode impls and
# ProcessInstance::try_canonical_hash_bytes/PersistedSnapshotState::try_canonical_hash_bytes.
HASH_DOMAIN_TYPES=(
  "bpmn-lite-types/src/types.rs:Value"
  "bpmn-lite-types/src/types.rs:WaitState"
  "bpmn-lite-types/src/types.rs:ProcessState"
  "bpmn-lite-types/src/types.rs:ErrorClass"
  "bpmn-lite-types/src/types.rs:Incident"
  "bpmn-lite-types/src/types.rs:Fiber"
  "bpmn-lite-types/src/types.rs:ProcessInstance"
  "bpmn-lite-types/src/session_stack.rs:SessionStackState"
  "bpmn-lite-types/src/session_stack.rs:SessionScopeState"
  "bpmn-lite-types/src/session_stack.rs:SessionWorkspaceKind"
  "bpmn-lite-types/src/concurrency.rs:ConcurrencyRecord"
  "bpmn-lite-types/src/concurrency.rs:ConcurrencyTable"
  "bpmn-lite-types/src/concurrency.rs:RecordKind"
  "bpmn-lite-types/src/concurrency.rs:RecordState"
  "bpmn-lite-types/src/concurrency.rs:RecordCounters"
  "bpmn-lite-types/src/persistence.rs:PersistedSnapshotState"
)

FORBIDDEN_PATTERN='\bHashMap\b|\bHashSet\b'

fail=0
note() { printf '  \033[31mCANONICAL INVARIANT VIOLATION\033[0m  %s\n' "$1"; fail=1; }

# Extracts the brace-delimited body of `pub struct TYPE { ... }` or
# `pub enum TYPE { ... }` (also matches without `pub`) from FILE, tracking
# brace depth so nested braces (e.g. a variant's own `{ field: T }`) don't
# terminate extraction early. Prints nothing if the type isn't found.
extract_type_body() {
  local file="$1" type_name="$2"
  awk -v type="$type_name" '
    BEGIN { depth = 0; capturing = 0 }
    !capturing && $0 ~ "^[[:space:]]*(pub[[:space:]]+)?(struct|enum)[[:space:]]+" type "([[:space:](<]|$)" {
      capturing = 1
    }
    capturing {
      print
      n = gsub(/\{/, "{")
      depth += n
      n = gsub(/\}/, "}")
      depth -= n
      if (depth <= 0 && $0 ~ /\}/) {
        capturing = 0
      }
    }
  ' "$file"
}

run_check() {
  # Takes the file:TypeName pairs as positional args (not a nameref array —
  # macOS ships bash 3.2, which lacks `local -n`).
  local any_hits=0
  for pair in "$@"; do
    local file="${pair%%:*}"
    local type_name="${pair##*:}"
    if [ ! -f "$file" ]; then
      note "expected file missing: $file (type $type_name) — canonical-invariant type inventory is stale, update this script"
      continue
    fi
    local body
    body="$(extract_type_body "$file" "$type_name")"
    if [ -z "$body" ]; then
      note "type '$type_name' not found in $file — canonical-invariant type inventory is stale, update this script"
      continue
    fi
    local hits
    hits="$(printf '%s\n' "$body" | grep -nE "$FORBIDDEN_PATTERN" || true)"
    if [ -n "$hits" ]; then
      any_hits=1
      note "$file: $type_name contains a HashMap/HashSet field — this type is walked by the canonical binary encoder (bpmn-lite-types/src/canonical.rs), which requires BTreeMap/BTreeSet-only for deterministic iteration order:
$hits"
    fi
  done
  return "$any_hits"
}

# V2.1h.6 (blind-review finding #6, follow-on): `serde_json::to_value` does
# not error on a non-finite `f64` — it silently coerces NaN/Infinity to
# `Value::Null` (see `canonical.rs`'s
# `serde_json_to_value_silently_coerces_non_finite_floats_to_null_not_an_error`
# test). Unlike h.1's field-type check, this is a call-site pattern, not a
# type-shape one: it flags any `serde_json::to_value` call textually near a
# `domain_payload`/`placeholder_values` reference — the two hash-domain
# fields capable of carrying externally-supplied JSON. This is a proximity
# heuristic (grep, not full dataflow analysis — the same honestly-scoped
# limitation `check-glossary.sh` and h.1's own type-body extraction accept),
# not a proof that every non-adjacent bypass is caught; it closes the
# concrete pattern found live in `bpmn-lite-server/src/rest.rs` during this
# remediation (`inst.placeholder_values = serde_json::to_value(&pv).ok();`,
# fixed to construct `serde_json::Value::Object` directly instead) and
# stands as a standing gate against its reintroduction.
TO_VALUE_SCAN_DIRS=(
  bpmn-lite-types/src
  bpmn-lite-kernel/src
  bpmn-lite-store/src
  bpmn-lite-store-postgres/src
  bpmn-lite-engine/src
  bpmn-lite-vm/src
  bpmn-lite-server-runner/src
  bpmn-lite-server-designer/src
  bpmn-lite-authoring/src
  bpmn-lite-bus-handler/src
)

# Flags `serde_json::to_value` occurrences within a small line-window of a
# `domain_payload`/`placeholder_values` identifier, in FILE. Prints hits
# (grep -n formatted) or nothing.
check_to_value_near_hash_domain_fields_in_file() {
  local file="$1"
  local hit_lines
  hit_lines="$(grep -nF 'serde_json::to_value' "$file" 2>/dev/null | cut -d: -f1 || true)"
  [ -z "$hit_lines" ] && return 0
  local line_no start end window
  for line_no in $hit_lines; do
    start=$((line_no - 3))
    [ "$start" -lt 1 ] && start=1
    end=$((line_no + 3))
    window="$(sed -n "${start},${end}p" "$file")"
    # Requires an actual field-access/assignment/binding shape
    # (`.placeholder_values`, `placeholder_values =`, `placeholder_values:`,
    # `placeholder_values,` for struct-literal shorthand) — a bare word
    # match would false-positive on this very file's own doc comments,
    # which discuss `domain_payload`/`placeholder_values` in prose
    # extensively without constructing either.
    if printf '%s\n' "$window" | grep -qE '\.(domain_payload|placeholder_values)\b|\b(domain_payload|placeholder_values)[[:space:]]*[:=,]'; then
      printf '%s:%s: serde_json::to_value within 3 lines of domain_payload/placeholder_values\n' "$file" "$line_no"
    fi
  done
}

check_to_value_near_hash_domain_fields() {
  local -a dirs
  if [ "$#" -gt 0 ]; then
    dirs=("$@")
  else
    dirs=("${TO_VALUE_SCAN_DIRS[@]}")
  fi
  local any_hits=0
  local file hits
  for dir in "${dirs[@]}"; do
    [ -d "$dir" ] || continue
    while IFS= read -r file; do
      hits="$(check_to_value_near_hash_domain_fields_in_file "$file")"
      if [ -n "$hits" ]; then
        any_hits=1
        note "serde_json::to_value reaching a hash-domain JSON field — encode_canonical_json's finiteness check cannot protect against a value that was already silently coerced to Null upstream (serde_json::to_value(f64::NAN) == Ok(Value::Null), not Err):
$hits"
      fi
    done < <(find "$dir" -name '*.rs')
  done
  return "$any_hits"
}

if [ "${1:-}" = "--self-test" ]; then
  echo "== bpmn-lite canonical hash-domain invariant guard self-test =="
  fixture="scripts/fixtures/canonical_invariant_violation.rs"
  if [ ! -f "$fixture" ]; then
    echo "  self-test fixture missing: $fixture"
    exit 1
  fi
  to_value_fixture="scripts/fixtures/canonical_invariant_to_value_violation.rs"
  if [ ! -f "$to_value_fixture" ]; then
    echo "  self-test fixture missing: $to_value_fixture"
    exit 1
  fi
  self_test_fail=0
  if run_check "$fixture:Fiber"; then
    echo "  FAIL — the HashMap-field lint did NOT fire against a fixture with a deliberate HashMap field (lint has gone toothless)."
    self_test_fail=1
  else
    echo "  OK — HashMap-field lint correctly fired against the deliberate HashMap-swap fixture."
  fi
  if check_to_value_near_hash_domain_fields "$(dirname "$to_value_fixture")"; then
    echo "  FAIL — the to_value guard did NOT fire against a fixture with a deliberate to_value-into-placeholder_values pattern (lint has gone toothless)."
    self_test_fail=1
  else
    echo "  OK — to_value guard correctly fired against the deliberate fixture."
  fi
  exit "$self_test_fail"
fi

echo "== bpmn-lite canonical hash-domain invariant guard (D3 Ring 2, V2.1h.1/h.6) =="

if ! run_check "${HASH_DOMAIN_TYPES[@]}"; then
  fail=1
fi

if ! check_to_value_near_hash_domain_fields; then
  fail=1
fi

# serde_json's `preserve_order` feature (if enabled anywhere in the
# workspace's resolved dependency graph, since Cargo features are unified)
# would make `serde_json::Map` iterate in insertion order instead of
# sorted-by-key — the same nondeterminism risk as a HashMap swap, for the
# domain_payload/placeholder_values/workspace_stack JSON canonicalization
# path in canonical.rs's encode_canonical_json. `preserve_order` pulls in
# `indexmap` specifically as *serde_json's own* dependency — checking for
# indexmap workspace-wide is too broad (petgraph, sqlx-core, rkyv, h2,
# serde_yaml, toml_edit, tower all depend on indexmap independently of
# serde_json in this workspace) so this greps serde_json's own `[[package]]`
# stanza in Cargo.lock specifically.
if [ -f Cargo.lock ]; then
  serde_json_stanza="$(awk '/^\[\[package\]\]$/{p=0} /^name = "serde_json"$/{p=1} p' Cargo.lock)"
  if printf '%s' "$serde_json_stanza" | grep -q 'indexmap'; then
    note "serde_json's own Cargo.lock dependency list includes indexmap — this means 'preserve_order' is enabled somewhere in the workspace, which breaks encode_canonical_json's 'serde_json::Map iteration is already key-sorted' assumption."
  fi
fi

if [ "$fail" -eq 0 ]; then
  echo "  OK — no HashMap/HashSet fields found in any hash-domain structure; no preserve_order-implying indexmap dependency; no serde_json::to_value near domain_payload/placeholder_values."
else
  echo ""
  echo "== Canonical hash-domain invariant guard FAILED =="
fi
exit "$fail"
