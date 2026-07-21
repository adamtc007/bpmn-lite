#!/usr/bin/env bash
# check-glossary.sh — V&S §2 glossary conformance guard (EOP-PLAN-BPMN-ISA-002 V1.5).
# Run from the repo root (bpmn-lite/).
#
# EOP-VS-BPMN-ISA-002 §2 binds specific identifiers to specific meanings and
# forbids specific collisions. This script enforces the collisions that have
# a mechanical, false-positive-free grep signature:
#
#   - "Instruction (cell)... Never called a token." A bytecode/instruction
#     TYPE named Token is forbidden. Lexical "token" in a lexer/parser
#     (source-text tokenization) is a different, legitimate CS usage and is
#     excluded from this check by path.
#   - "Boundary event... Never used to name runtime state." A runtime-state
#     TYPE or table named BoundaryEventState (or similar) is forbidden; the
#     bound runtime term is Guard.
#   - "renamed [scope table] to Concurrency table" (V&S review R1 disposition,
#     §11): a type or table literally named ScopeTable/scope_table is
#     forbidden; the bound term is ConcurrencyTable.
#
# This is a collision lint, not a style lint: it does not forbid the word
# "Scope" itself (the glossary's general dynamic-extent category term) or
# "Token" in lexer/parser code — only the specific renamed/forbidden
# identifier collisions above.
#
# Usage:
#   check-glossary.sh              scan the real source tree; fail on any hit
#   check-glossary.sh --self-test  scan scripts/fixtures/glossary_violations.rs
#                                  and fail unless EVERY rule's pattern fires —
#                                  proves the lint isn't silently toothless
set -uo pipefail

TOKEN_PATTERN='\b(struct|enum|type)[[:space:]]+Token\b'
BOUNDARY_STATE_PATTERN='\b(struct|enum|type)[[:space:]]+BoundaryEvent(State|Record)?\b'
SCOPE_TABLE_PATTERN='\bScopeTable\b|\bscope_table\b|\bscopes_table\b'

fail=0
note() { printf '  \033[31mGLOSSARY VIOLATION\033[0m  %s\n' "$1"; fail=1; }

if [ "${1:-}" = "--self-test" ]; then
  echo "== bpmn-lite glossary guard self-test =="
  fixture="scripts/fixtures/glossary_violations.rs"
  if [ ! -f "$fixture" ]; then
    echo "  self-test fixture missing: $fixture"
    exit 1
  fi
  ok=0
  for label_pattern in \
    "Token-as-bytecode-type:$TOKEN_PATTERN" \
    "BoundaryEvent-naming-runtime-state:$BOUNDARY_STATE_PATTERN" \
    "Scope-naming-the-table:$SCOPE_TABLE_PATTERN"
  do
    label="${label_pattern%%:*}"
    pattern="${label_pattern#*:}"
    if grep -nE "$pattern" "$fixture" >/dev/null 2>&1; then
      echo "  OK — rule '$label' fires against the fixture."
    else
      echo "  FAIL — rule '$label' did NOT fire against the fixture (lint has gone toothless)."
      ok=1
    fi
  done
  exit "$ok"
fi

echo "== bpmn-lite glossary guard (V&S §2) =="

# Directories holding ISA/bytecode/runtime-state identifiers. Excludes DSL
# and DMN lexers/parsers, where "Token" is legitimate source-text lexing
# vocabulary, not a bytecode/instruction identifier.
isa_dirs=(
  bpmn-lite-types/src
  bpmn-lite-kernel/src
  bpmn-lite-store/src
  bpmn-lite-store-postgres/src
  bpmn-lite-engine/src
  bpmn-lite-vm/src
  bpmn-lite-server/src
  bpmn-lite-authoring/src
)
isa_files_excluding_lexers() {
  for dir in "${isa_dirs[@]}"; do
    [ -d "$dir" ] || continue
    find "$dir" -name '*.rs' \
      -not -path '*/dsl/lexer.rs' -not -path '*/dsl/parser.rs' \
      -not -name 'lexer.rs' -not -name 'parser.rs'
  done
}

rs_files="$(isa_files_excluding_lexers)"

if [ -n "$rs_files" ]; then
  token_hits="$(printf '%s\n' "$rs_files" | xargs grep -nE "$TOKEN_PATTERN" 2>/dev/null || true)"
  [ -n "$token_hits" ] && note "bytecode/instruction type named Token (bound term: Instr / cell):
$token_hits"

  boundary_state_hits="$(printf '%s\n' "$rs_files" | xargs grep -nE "$BOUNDARY_STATE_PATTERN" 2>/dev/null || true)"
  [ -n "$boundary_state_hits" ] && note "runtime state named after Boundary event (bound term: Guard — boundary event is an authoring construct only):
$boundary_state_hits"

  scope_table_hits="$(printf '%s\n' "$rs_files" | xargs grep -nE "$SCOPE_TABLE_PATTERN" 2>/dev/null || true)"
  [ -n "$scope_table_hits" ] && note "concurrency table named after Scope (bound term: ConcurrencyTable — renamed under review R1, V&S §11):
$scope_table_hits"
fi

# Same collisions in SQL migrations (table/column names).
if [ -d bpmn-lite-store-postgres/migrations ]; then
  sql_scope_hits="$(grep -rniE 'scope_table|scopes_table' \
    bpmn-lite-store-postgres/migrations 2>/dev/null || true)"
  [ -n "$sql_scope_hits" ] && note "migration names a table after Scope (bound term: concurrency_table):
$sql_scope_hits"
fi

if [ "$fail" -eq 0 ]; then
  echo "  OK — no forbidden glossary collisions found."
else
  echo ""
  echo "== Glossary guard FAILED =="
fi
exit "$fail"
