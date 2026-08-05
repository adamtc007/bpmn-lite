#!/usr/bin/env bash
# check-shared-pin.sh — proves the committed dependency-resolution artifacts
# actually pin the shared /dev/dsl (sem_os_*/dsl-core/dsl_types) revision the
# BPMN mapper was built and reviewed against, per plan invariant 19:
# "BPMN depends on a pinned shared release/revision in committed manifests.
# Local path patches are development-only; CI must prove the shared
# dependencies are active and not listed as unused patches."
#
# Why this exists: the committed Cargo.toml/Cargo.lock pin was verified
# correct by hand during the utterance-mapper review (2026-08-05), but
# nothing ran that check as a gate. Meanwhile this developer's global
# ~/.cargo/config.toml carries local-dev [patch] redirects for the dsl repo
# (deliberate, documented cross-repo infra -- see that file's own header),
# which silently rewrites Cargo.lock to drop the git pin the moment any
# ordinary `cargo build`/`cargo test` runs locally. One `git add -A` from
# that state would ship an unpinned lock with nothing to catch it. This
# script is a pure static check of the COMMITTED Cargo.lock text -- it does
# not invoke cargo and is therefore immune to the same local-patch
# pollution it exists to guard against.
#
# Two checks:
#   1. Every locked package sourced from the shared dsl repo (identified by
#      `source = "git+https://github.com/adamtc007/dsl?rev=..."`) resolves
#      to the EXACT SAME revision, and that revision matches the rev pinned
#      in Cargo.toml for sem_os_ontology/sem_os_policy (the only two direct
#      deps; the rest -- dsl-core, dsl_types, sem_os_core, sem_os_types --
#      are transitive and must follow the same git checkout).
#   2. Cargo.lock contains no `[[patch.unused]]` block naming any of the six
#      known shared-crate package names. An unused-patch entry for one of
#      these means Cargo resolved that dependency from somewhere other than
#      the declared git pin (typically a local path patch) -- exactly the
#      failure mode this gate exists to catch.
#
# Usage:
#   check-shared-pin.sh              scan the real repo (Cargo.toml/Cargo.lock)
#   check-shared-pin.sh --self-test  scan deliberate-violation fixtures (via
#                                     the SAME functions used for the real
#                                     scan) and fail unless every one fires
set -uo pipefail

SHARED_REPO_URL="https://github.com/adamtc007/dsl"
SHARED_PACKAGES=(sem_os_ontology sem_os_policy dsl-core dsl_types sem_os_core sem_os_types)

fail=0
note() { printf '  \033[31mSHARED-PIN VIOLATION\033[0m  %s\n' "$1"; fail=1; }

# Extracts the rev pinned in Cargo.toml for sem_os_ontology (the reference
# revision every other shared-repo package must match). Scans relative to
# the CURRENT DIRECTORY so --self-test can operate on a throwaway fixture.
expected_rev_from_manifest() {
  local manifest="Cargo.toml"
  [ -f "$manifest" ] || { echo ""; return; }
  grep -E '^\s*sem_os_ontology\s*=.*git\s*=\s*"'"$SHARED_REPO_URL"'"' "$manifest" \
    | grep -oE 'rev\s*=\s*"[0-9a-f]{7,40}"' \
    | grep -oE '[0-9a-f]{7,40}' \
    | head -n1
}

# Check 1: every dsl-repo-sourced package in Cargo.lock resolves to the same
# revision, and that revision matches Cargo.toml's declared pin. A package
# whose `source` line names the shared repo URL but a DIFFERENT rev than
# its siblings (or than Cargo.toml) means resolution drifted -- e.g. a
# local worktree at a different commit was picked up, or two revisions of
# the same repo are simultaneously live in the graph.
check_pinned_revision() {
  local lockfile="Cargo.lock"
  [ -f "$lockfile" ] || { note "no Cargo.lock present to check"; return 1; }

  local expected
  expected="$(expected_rev_from_manifest)"
  if [ -z "$expected" ]; then
    note "Cargo.toml does not pin sem_os_ontology to an exact git rev (git = \"$SHARED_REPO_URL\", rev = \"<sha>\") -- cannot verify the lock against it"
    return 1
  fi

  local any=0
  local pkg name source rev
  for pkg in "${SHARED_PACKAGES[@]}"; do
    source="$(awk -v pkg="$pkg" '
      $0 == "[[package]]" { in_pkg=0; name=""; src="" }
      /^name = / { name=$0; sub(/^name = "/, "", name); sub(/"$/, "", name) }
      /^source = / && name == pkg { print; exit }
    ' "$lockfile")"
    if [ -z "$source" ]; then
      note "$pkg is not present in Cargo.lock at all -- expected it resolved from $SHARED_REPO_URL"
      any=1
      continue
    fi
    if ! printf '%s' "$source" | grep -q "git+${SHARED_REPO_URL}?rev="; then
      note "$pkg does not resolve from a pinned git rev of $SHARED_REPO_URL: $source"
      any=1
      continue
    fi
    rev="$(printf '%s' "$source" | grep -oE 'rev=[0-9a-f]{7,40}' | head -n1 | cut -d= -f2)"
    if [ "$rev" != "$expected" ]; then
      note "$pkg resolves to rev $rev, expected $expected (Cargo.toml's pinned sem_os_ontology rev): $source"
      any=1
    fi
  done
  return "$any"
}

# Check 2: no `[[patch.unused]]` entry names one of the six shared package
# names. Cargo emits this section in Cargo.lock when a [patch] table
# (typically a local path override) was configured but its target was
# never selected by resolution -- if that happens for one of OUR packages,
# something else (a different [patch], a stale lock, a floating dependency)
# resolved it instead of the declared git pin, and simply not seeing the
# patch fire is not proof the pin held.
check_no_unused_patch() {
  local lockfile="Cargo.lock"
  [ -f "$lockfile" ] || return 0

  local any=0
  local pkg hit
  for pkg in "${SHARED_PACKAGES[@]}"; do
    hit="$(awk -v pkg="$pkg" '
      $0 == "[[patch.unused]]" { in_block=1; next }
      /^\[\[/ { in_block=0 }
      in_block && /^name = / {
        n=$0; sub(/^name = "/, "", n); sub(/"$/, "", n)
        if (n == pkg) print NR ": [[patch.unused]] name = \"" pkg "\""
      }
    ' "$lockfile")"
    if [ -n "$hit" ]; then
      note "Cargo.lock lists $pkg under [[patch.unused]] -- it was NOT resolved from the declared git pin:
$hit"
      any=1
    fi
  done
  return "$any"
}

if [ "${1:-}" = "--self-test" ]; then
  echo "== bpmn-lite shared-pin guard self-test =="
  ok=1
  orig_dir="$(pwd)"

  # --- Fixture 1: a package resolves to a different rev than the pin ---
  tmp1="$(mktemp -d)"
  cat > "$tmp1/Cargo.toml" <<'EOF'
[dependencies]
sem_os_ontology = { git = "https://github.com/adamtc007/dsl", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
sem_os_policy = { git = "https://github.com/adamtc007/dsl", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
EOF
  cat > "$tmp1/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "sem_os_ontology"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[package]]
name = "sem_os_policy"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb#bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
EOF
  if (cd "$tmp1" && ! check_pinned_revision > /dev/null 2>&1); then
    echo "  OK — mismatched-revision fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a package resolved to a different rev than the pin (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp1"

  # --- Fixture 2: a shared package is missing from the lock entirely ---
  tmp2="$(mktemp -d)"
  cat > "$tmp2/Cargo.toml" <<'EOF'
[dependencies]
sem_os_ontology = { git = "https://github.com/adamtc007/dsl", rev = "cccccccccccccccccccccccccccccccccccccccc" }
EOF
  cat > "$tmp2/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "sem_os_ontology"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=cccccccccccccccccccccccccccccccccccccccc#cccccccccccccccccccccccccccccccccccccccc"
EOF
  if (cd "$tmp2" && ! check_pinned_revision > /dev/null 2>&1); then
    echo "  OK — missing-package fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a shared package absent from Cargo.lock (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp2"

  # --- Fixture 3: a shared package resolves from a local path, not the
  # pinned git rev at all (source line missing / non-git) ---
  tmp3="$(mktemp -d)"
  cat > "$tmp3/Cargo.toml" <<'EOF'
[dependencies]
sem_os_ontology = { git = "https://github.com/adamtc007/dsl", rev = "dddddddddddddddddddddddddddddddddddddddd" }
EOF
  cat > "$tmp3/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "sem_os_ontology"
version = "0.1.0"
EOF
  if (cd "$tmp3" && ! check_pinned_revision > /dev/null 2>&1); then
    echo "  OK — local/unpinned-source fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against a shared package with no pinned git source (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp3"

  # --- Fixture 4: [[patch.unused]] names one of the shared packages ---
  tmp4="$(mktemp -d)"
  cat > "$tmp4/Cargo.lock" <<'EOF'
version = 4

[[patch.unused]]
name = "sem_os_policy"
version = "0.1.0"
EOF
  if (cd "$tmp4" && ! check_no_unused_patch > /dev/null 2>&1); then
    echo "  OK — [[patch.unused]] fixture correctly detected."
  else
    echo "  FAIL — the lint did NOT fire against sem_os_policy listed in [[patch.unused]] (lint has gone toothless)."
    ok=0
  fi
  rm -rf "$tmp4"

  # --- Fixture 5: clean, consistently-pinned lock must NOT fire ---
  tmp5="$(mktemp -d)"
  cat > "$tmp5/Cargo.toml" <<'EOF'
[dependencies]
sem_os_ontology = { git = "https://github.com/adamtc007/dsl", rev = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" }
sem_os_policy = { git = "https://github.com/adamtc007/dsl", rev = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" }
EOF
  cat > "$tmp5/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "sem_os_ontology"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee#eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

[[package]]
name = "sem_os_policy"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee#eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

[[package]]
name = "dsl-core"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee#eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

[[package]]
name = "dsl_types"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee#eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

[[package]]
name = "sem_os_core"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee#eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

[[package]]
name = "sem_os_types"
version = "0.1.0"
source = "git+https://github.com/adamtc007/dsl?rev=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee#eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
EOF
  cd "$tmp5"
  clean_ok=1
  check_pinned_revision > /dev/null 2>&1 || clean_ok=0
  check_no_unused_patch > /dev/null 2>&1 || clean_ok=0
  cd "$orig_dir"
  if [ "$clean_ok" -eq 1 ]; then
    echo "  OK — clean, consistently-pinned fixture correctly passes."
  else
    echo "  FAIL — the lint fired against a CLEAN, correctly-pinned fixture (false positive)."
    ok=0
  fi
  rm -rf "$tmp5"

  if [ "$ok" -eq 1 ]; then
    exit 0
  else
    exit 1
  fi
fi

echo "== bpmn-lite shared-pin guard =="
fail=0
if ! check_pinned_revision; then
  fail=1
fi
if ! check_no_unused_patch; then
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "  OK — sem_os_ontology, sem_os_policy, dsl-core, dsl_types, sem_os_core and sem_os_types all resolve from the single pinned $SHARED_REPO_URL revision, with no unused-patch fallback."
else
  echo ""
  echo "== Shared-pin guard FAILED =="
fi
exit "$fail"
