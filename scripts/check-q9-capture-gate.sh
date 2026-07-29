#!/usr/bin/env bash
# check-q9-capture-gate.sh — Q9-gated user-capture feature must stay OUT
# of every default build and every documented build/release/CI command.
# Run from the repo root (bpmn-lite/).
#
# DIR-004 Phase 1 ruling (Option B, structural separation, 2026-07-29):
# `utterance-engine`'s Q9-gated user-capture module (`capture.rs`) is
# feature-gated behind `q9-capture`, off by default, so a pre-charter
# build cannot even compile a live user-capture path. That guarantee is
# only real if nothing flips the feature on behind the scenes -- a
# `default = [..., "q9-capture"]` in some Cargo.toml, or a CI/Docker
# command that passes `--all-features` (which cannot exclude any
# feature by construction) or an explicit `--features ...q9-capture...`
# in ANY form (quoted, unquoted, line-continued). This script is the
# mechanical check; per-site discipline alone is exactly what Option B
# was ruled to replace.
#
# Two checks:
#   1. No Cargo.toml anywhere in the repo lists q9-capture inside its
#      `default = [...]` feature array.
#   2. No build-invocation surface (.github/workflows/*.yml,
#      scripts/*.sh, Dockerfile*, Makefile, justfile) contains
#      `--all-features`, OR mentions the literal string `q9-capture` at
#      all outside a comment -- deliberately a blunt substring check,
#      not a flag-syntax parser: a workflow/script/Dockerfile has no
#      legitimate reason to write that feature name at all except to
#      enable it, so trying to distinguish "quoted", "line-continued",
#      or other --features spellings is a losing game the substring
#      check sidesteps entirely. UNLESS the file is in this script's
#      ALLOWLIST_FILES array below.
#
# ALLOWLIST_FILES is empty today -- there is no ratified Q9 charter yet,
# so nothing is allowlisted. When a charter lands and a real,
# purpose-built release path for it is added, add that ONE file's path
# here with a comment naming the charter reference, never a wildcard.
#
# Scan surface is named explicitly (workflows, scripts, Dockerfiles,
# Makefile/justfile) -- if a new build-invocation surface is introduced
# (a different CI provider's config, a fresh task-runner file), add its
# glob here; this script does not discover build surfaces on its own.
#
# Usage:
#   check-q9-capture-gate.sh              scan the real repo
#   check-q9-capture-gate.sh --self-test  scan deliberate-violation
#                                          fixtures (in a throwaway temp
#                                          repo, via the SAME functions
#                                          used for the real scan) and
#                                          fail unless every one fires
set -uo pipefail

ALLOWLIST_FILES=(
  # (empty -- see header. Add exactly one file per ratified exception.)
)

fail=0
note() { printf '  \033[31mQ9-CAPTURE GATE VIOLATION\033[0m  %s\n' "$1"; fail=1; }

is_allowlisted() {
  local f="$1" entry
  for entry in "${ALLOWLIST_FILES[@]:-}"; do
    [ -n "$entry" ] && [ "$entry" = "$f" ] && return 0
  done
  return 1
}

# Check 1: q9-capture must never appear inside a `default = [...]` array
# in any Cargo.toml. Scans the array literal itself (possibly spanning
# multiple lines up to the closing `]`), not just any mention of the
# feature name elsewhere in the file (comments/other feature defs are
# expected to say "q9-capture" -- that is not a violation by itself).
# Scans relative to the CURRENT DIRECTORY, so --self-test can cd into a
# throwaway fixture repo and call this exact function unmodified.
check_no_default_feature() {
  local any=0
  local file
  while IFS= read -r file; do
    local in_default=0 array_text=""
    while IFS= read -r line; do
      if [ "$in_default" -eq 0 ]; then
        if printf '%s' "$line" | grep -qE '^\s*default\s*='; then
          in_default=1
          array_text="$line"
          if printf '%s' "$line" | grep -q ']'; then
            in_default=0
            if printf '%s' "$array_text" | grep -q '"q9-capture"'; then
              note "$file: q9-capture is listed in a [features] default = [...] array -- it must never be a default feature"
              any=1
            fi
            array_text=""
          fi
        fi
      else
        array_text="$array_text"$'\n'"$line"
        if printf '%s' "$line" | grep -q ']'; then
          in_default=0
          if printf '%s' "$array_text" | grep -q '"q9-capture"'; then
            note "$file: q9-capture is listed in a [features] default = [...] array -- it must never be a default feature"
            any=1
          fi
          array_text=""
        fi
      fi
    done < "$file"
  done < <(find . -name 'Cargo.toml' -not -path '*/target/*' | sort)
  return "$any"
}

# Strips full-line `#` comments (YAML and bash share the syntax), so
# prose mentioning "--all-features" or "q9-capture" while EXPLAINING
# this very check (as this script's own header, or a workflow's
# explanatory comment, both do) is not mistaken for a real invocation.
# Line numbers are preserved (blanked, not deleted) so reported hits
# still point at the real file. Deliberately line-based, not aware of
# YAML block-scalar (`run: |`) boundaries -- a `#` at the start of a
# line inside a `run: |` block is extremely unusual shell (it would be
# a shell comment there too) and stripping it costs nothing; the
# alternative (a YAML-aware parser) is not worth the complexity this
# check needs to stay auditable by reading it.
strip_comment_lines() {
  sed -E 's/^([[:space:]]*)#.*$/\1/' "$1"
}

# Check 2: no build-invocation surface may pass --all-features (cannot
# exclude q9-capture by construction), or mention `q9-capture` at all
# outside a comment (a blunt substring check, deliberately NOT a
# --features flag parser -- a prior version tried to pattern-match the
# flag syntax and was defeated by quoting the feature list, e.g.
# `--features "postgres,q9-capture"`; a substring check has no syntax
# to evade). This checker script itself is excluded from the scan -- it
# is the detector, not a build surface, and necessarily contains these
# strings in its own matching logic and this very comment. Scans
# relative to the CURRENT DIRECTORY (see check_no_default_feature).
check_no_build_invocation() {
  local any=0
  local file stripped
  while IFS= read -r file; do
    is_allowlisted "$file" && continue
    case "$file" in
      */check-q9-capture-gate.sh|check-q9-capture-gate.sh) continue ;;
    esac
    stripped="$(strip_comment_lines "$file")"
    local hits
    hits="$(printf '%s\n' "$stripped" | grep -nE -- '--all-features' || true)"
    if [ -n "$hits" ]; then
      note "$file: uses --all-features, which cannot exclude q9-capture -- name features explicitly instead:
$hits"
      any=1
    fi
    # Invoking THIS checker script by name ("check-q9-capture-gate.sh")
    # is expected everywhere (every workflow runs it) and is not itself
    # a feature-enabling mention -- strip that one token before the
    # substring test so the gate doesn't flag its own invocation, while
    # any OTHER occurrence of "q9-capture" on the same or another line
    # still fires.
    hits="$(printf '%s\n' "$stripped" | sed -E 's/check-q9-capture-gate\.sh//g' | grep -nF -- 'q9-capture' || true)"
    if [ -n "$hits" ]; then
      note "$file: mentions q9-capture outside a comment -- a build/CI/Docker/task-runner file has no legitimate reason to name this feature at all except to enable it:
$hits"
      any=1
    fi
  done < <( {
      find .github/workflows -name '*.yml' -o -name '*.yaml'
      find scripts -name '*.sh'
      find . -maxdepth 1 -iname 'Dockerfile*'
      find . -maxdepth 1 -iname 'Makefile' -o -maxdepth 1 -iname 'justfile'
    } 2>/dev/null | sort -u )
  return "$any"
}

if [ "${1:-}" = "--self-test" ]; then
  echo "== bpmn-lite Q9 capture-gate guard self-test =="
  ok=1
  orig_dir="$(pwd)"

  # --- Fixture 1: q9-capture in a Cargo.toml default array ---
  tmp1="$(mktemp -d)"
  cat > "$tmp1/Cargo.toml" <<'EOF'
[package]
name = "fixture"

[features]
default = ["postgres", "q9-capture"]
postgres = []
q9-capture = []
EOF
  if (cd "$tmp1" && ! check_no_default_feature > /dev/null 2>&1); then
    echo "  OK — default-feature fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a Cargo.toml with q9-capture in default = [...] (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp1"

  # --- Fixture 2: bare --all-features in a workflow ---
  tmp2="$(mktemp -d)"
  mkdir -p "$tmp2/.github/workflows" "$tmp2/scripts"
  cat > "$tmp2/.github/workflows/fixture.yml" <<'EOF'
name: fixture
jobs:
  x:
    steps:
      - run: cargo test --workspace --all-features
EOF
  if (cd "$tmp2" && ! check_no_build_invocation > /dev/null 2>&1); then
    echo "  OK — --all-features fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a workflow using --all-features (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp2"

  # --- Fixture 3: quoted --features "...,q9-capture" (the bypass a
  # flag-syntax regex missed; this is why check_no_build_invocation is a
  # blunt substring check now, not a flag parser) ---
  tmp3="$(mktemp -d)"
  mkdir -p "$tmp3/.github/workflows" "$tmp3/scripts"
  cat > "$tmp3/.github/workflows/fixture.yml" <<'EOF'
name: fixture
jobs:
  x:
    steps:
      - run: cargo build --workspace --features "postgres,q9-capture"
EOF
  if (cd "$tmp3" && ! check_no_build_invocation > /dev/null 2>&1); then
    echo "  OK — quoted --features \"...,q9-capture\" fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a QUOTED --features list naming q9-capture (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp3"

  # --- Fixture 4: line-continued --features \ ... q9-capture ---
  tmp4="$(mktemp -d)"
  mkdir -p "$tmp4/.github/workflows" "$tmp4/scripts"
  cat > "$tmp4/scripts/build.sh" <<'EOF'
#!/usr/bin/env bash
cargo build --workspace --features \
  postgres,q9-capture
EOF
  if (cd "$tmp4" && ! check_no_build_invocation > /dev/null 2>&1); then
    echo "  OK — line-continued --features ... q9-capture fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a line-continued --features naming q9-capture (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp4"

  # --- Fixture 5: clean repo (comments mentioning both terms only, plus
  # the ordinary act of INVOKING this checker script by name -- every
  # real workflow does exactly this) must NOT fire. Regression guard for
  # the self-referential false positive this script's own invocation
  # line used to trigger against the real workflows. ---
  tmp5="$(mktemp -d)"
  mkdir -p "$tmp5/.github/workflows" "$tmp5/scripts"
  cat > "$tmp5/.github/workflows/fixture.yml" <<'EOF'
name: fixture
jobs:
  x:
    steps:
      # explanatory comment: q9-capture and --all-features are both
      # mentioned here in prose, never invoked
      - run: cargo build --workspace --features postgres,database,embed,candle-probe
      - name: Q9 capture-gate guard
        run: bash scripts/check-q9-capture-gate.sh
EOF
  cd "$tmp5"
  clean_ok=1
  check_no_build_invocation > /dev/null 2>&1 || clean_ok=0
  check_no_default_feature > /dev/null 2>&1 || clean_ok=0
  cd "$orig_dir"
  if [ "$clean_ok" -eq 1 ]; then
    echo "  OK — clean fixture (comment-only mentions + invoking this checker by name) correctly passes."
  else
    echo "  FAIL — the lint fired against a CLEAN fixture (false positive — check the self-invocation exclusion)."
    ok=0
  fi
  rm -rf "$tmp5"

  if [ "$ok" -eq 1 ]; then
    exit 0
  else
    exit 1
  fi
fi

echo "== bpmn-lite Q9 capture-gate guard =="
fail=0
if ! check_no_default_feature; then
  fail=1
fi
if ! check_no_build_invocation; then
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "  OK — q9-capture is not a default feature anywhere, and no build/CI command enables it."
else
  echo ""
  echo "== Q9 capture-gate guard FAILED =="
fi
exit "$fail"
