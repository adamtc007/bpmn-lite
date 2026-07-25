#!/usr/bin/env bash
# check-transition-read-safety.sh — stale-read-within-transition guard.
# Run from the repo root (bpmn-lite/).
#
# `apply_tick` runs one fibre across potentially many bytecode instructions
# within a single transition without re-snapshotting between them. A
# concurrency-record read that goes straight to `snapshot.concurrency_
# table()` mid-transition is blind to mutations this SAME transition has
# already staged into `changes` (see `fetch_record_in_transition`'s own
# doc comment in bpmn-lite-kernel/src/lib.rs). Per-site discipline alone
# does not hold against this: any new `.concurrency_table()` call inside
# bpmn-lite-kernel/src/lib.rs must be added to this script's own allowlist
# by name, or it fails CI.
#
# `fetch_record_in_transition` (single record) and `armed_record_ids_in_
# transition` (bulk) are the two sanctioned raw-read sites — everything
# else reachable from `apply_tick`'s instruction loop must route through
# one of them. A handful of functions are allowlisted as verified SAFE
# raw readers instead: each is a single-shot Command handler (or the
# snapshot-commit reducer) provably running with `changes` still empty, or
# freshly constructed, at the point of its own read — see this script's
# SAFE_FUNCTIONS comments below for the per-function reasoning, and re-
# verify that reasoning (not just re-add the name) before allowlisting any
# new function.
#
# Usage:
#   check-transition-read-safety.sh              scan the real source tree
#   check-transition-read-safety.sh --self-test  scan the deliberate-
#                                                 violation fixture and fail
#                                                 unless it fires
set -uo pipefail

KERNEL_FILE="bpmn-lite-kernel/src/lib.rs"

# The two sanctioned raw-read sites — the only functions permitted to call
# `.concurrency_table()` without going through one of these two.
HELPER_FUNCTIONS=(
  "fetch_record_in_transition"
  "armed_record_ids_in_transition"
)

# Verified-safe raw readers. Do not extend this list without tracing the
# call graph back to a point where `changes` is provably empty or freshly
# constructed at the read — see each comment below for that proof.
SAFE_FUNCTIONS=(
  # Snapshot-commit reducers: build the NEXT snapshot from a prior one plus
  # an already-finished `Transition` — not a mid-transition read at all.
  "materialize_snapshot"
  "derive_post_transition_frame"
  # Command::EffectFailed handler. Every branch that reaches a
  # `.concurrency_table()` read returns before any earlier branch in the
  # SAME call could have staged a concurrency_mutation — `changes` is
  # still `Changes::default()` at each read.
  "apply_job_failure"
  # Command::V2TriggerGuard handler and its two shared helpers — each
  # constructs its own `Changes` fresh at the top of the call, so there is
  # no "earlier instruction this transition" for a stale read to see.
  "apply_v2_trigger_guard"
  "v2_trigger_guard_changes"
  "v2_trigger_guard_changes_with_target"
  # Command::TimerFired handler. The `V2GuardTimer` arm's read happens
  # before `changes`/`instance` are constructed for this call.
  "apply_timer"
)

fail=0
note() { printf '  \033[31mTRANSITION READ-SAFETY VIOLATION\033[0m  %s\n' "$1"; fail=1; }

# Production-code prefix of FILE, `//`-comment-stripped, so:
#   (a) a prose doc comment containing a stray `{`/`}` (this codebase's
#       functions carry dense multi-paragraph doc comments — e.g. this
#       very script's own target functions) can't desync the brace-depth
#       counter below, and a comment that merely MENTIONS
#       `.concurrency_table()` in prose (as several already do,
#       describing the hazard this lint guards against) doesn't
#       false-positive;
#   (b) `#[cfg(test)] mod tests { ... }` — which reads `.concurrency_
#       table()` extensively but only ever on an already-finished
#       `Snapshot` for test assertions, never mid-transition, and is not
#       part of `apply_tick`'s call graph at all — is excluded, rather
#       than needing every test function individually allowlisted.
# Line numbers are preserved throughout (`s#//.*$##`/blanking, not
# deletion) so hit line numbers in the report still point at the real
# file.
production_code_prefix() {
  awk '/^#\[cfg\(test\)\]$/ { in_test = 1 } in_test { $0 = "" } { print }' "$1"
}

strip_line_comments() {
  production_code_prefix "$1" | sed -E 's#//.*$##'
}

# Extracts the body of `fn NAME(...) -> ... { ... }` (also matches `pub
# fn`/`async fn`) from comment-stripped TEXT (see strip_line_comments),
# brace-depth tracked from the function's own opening brace (which may be
# several lines after the `fn` line, past a multi-line parameter list)
# through to its matching close. Mirrors check-canonical-invariant.sh's
# extract_type_body, adapted for `fn` instead of `struct`/`enum`.
extract_fn_body() {
  local stripped_text="$1" fn_name="$2"
  printf '%s\n' "$stripped_text" | awk -v name="$fn_name" '
    BEGIN { depth = 0; capturing = 0; started_body = 0 }
    !capturing && $0 ~ "^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+" name "[[:space:](<]" {
      capturing = 1
    }
    capturing {
      print NR ":" $0
      n = gsub(/\{/, "{")
      if (n > 0) started_body = 1
      depth += n
      n = gsub(/\}/, "}")
      depth -= n
      if (started_body && depth <= 0) {
        capturing = 0
      }
    }
  '
}

# All top-level `fn`/`pub fn`/`async fn` names declared in comment-stripped
# TEXT, in declaration order.
list_fn_names() {
  local stripped_text="$1"
  printf '%s\n' "$stripped_text" \
    | grep -oE '^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z0-9_]+' \
    | grep -oE '[A-Za-z0-9_]+$'
}

# Scans FILE for any function whose body contains `.concurrency_table()`
# and is not in HELPER_FUNCTIONS/SAFE_FUNCTIONS. Prints violations (empty
# if none) and returns 1 if any were found, 0 otherwise.
run_check() {
  local file="$1"
  local stripped_text
  stripped_text="$(strip_line_comments "$file")"
  local any_hits=0
  local fn_name allowed body hits entry line_offset
  while IFS= read -r fn_name; do
    [ -z "$fn_name" ] && continue
    allowed=0
    for entry in "${HELPER_FUNCTIONS[@]}" "${SAFE_FUNCTIONS[@]}"; do
      if [ "$entry" = "$fn_name" ]; then
        allowed=1
        break
      fi
    done
    [ "$allowed" -eq 1 ] && continue
    # `body`'s lines are already `<real file line number>:<content>`
    # (stamped by extract_fn_body's `print NR ":" $0`, against the
    # comment-stripped-but-line-preserved text) — grep here just filters,
    # it doesn't need its own `-n` to produce a correct line number.
    body="$(extract_fn_body "$stripped_text" "$fn_name")"
    [ -z "$body" ] && continue
    hits="$(printf '%s\n' "$body" | grep -F '.concurrency_table()' || true)"
    if [ -n "$hits" ]; then
      any_hits=1
      note "$file: fn $fn_name() calls .concurrency_table() directly but is not in check-transition-read-safety.sh's allowlist — route the read through fetch_record_in_transition/armed_record_ids_in_transition instead, or add this function to SAFE_FUNCTIONS with a proven (not assumed) reason it cannot observe a stale mid-transition read:
$hits"
    fi
  done < <(list_fn_names "$stripped_text")
  return "$any_hits"
}

if [ "${1:-}" = "--self-test" ]; then
  echo "== bpmn-lite transition read-safety guard self-test =="
  fixture="scripts/fixtures/transition_read_safety_violation.rs"
  if [ ! -f "$fixture" ]; then
    echo "  self-test fixture missing: $fixture"
    exit 1
  fi
  if run_check "$fixture"; then
    echo "  FAIL — the lint did NOT fire against a fixture with a deliberate unallowlisted .concurrency_table() call (lint has gone toothless)."
    exit 1
  else
    echo "  OK — lint correctly fired against the deliberate-violation fixture."
    exit 0
  fi
fi

echo "== bpmn-lite transition read-safety guard =="

# Scan EVERY kernel source file, not just lib.rs — a raw read moved into a
# new module would otherwise silently escape the lint (baseline-review F5).
# lib.rs must still exist (the anchor); additional files are picked up
# automatically.
if [ ! -f "$KERNEL_FILE" ]; then
  note "expected file missing: $KERNEL_FILE — this lint's target has moved, update this script"
fi
scanned=0
while IFS= read -r kernel_src; do
  scanned=$((scanned + 1))
  if ! run_check "$kernel_src"; then
    fail=1
  fi
done < <(find bpmn-lite-kernel/src -name '*.rs' | sort)
if [ "$scanned" -eq 0 ]; then
  note "no kernel source files found under bpmn-lite-kernel/src — the lint scanned nothing"
fi

if [ "$fail" -eq 0 ]; then
  echo "  OK — every .concurrency_table() call across $scanned kernel source file(s) is inside a helper or an allowlisted verified-safe function."
else
  echo ""
  echo "== Transition read-safety guard FAILED =="
fi
exit "$fail"
