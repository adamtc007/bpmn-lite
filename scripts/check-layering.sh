#!/usr/bin/env bash
# check-layering.sh — layering guard for bpmn-lite.
# Run from the repo root (bpmn-lite/).
#
# Rule: bpmn-lite must NOT reach sideways into ob_poc_types.
# Phase 4 (2026-05-22) replaced the ob-poc-types git dep with locally
# defined types in bpmn-lite-types::session_stack. This guard ensures
# it stays replaced.
set -uo pipefail

fail=0
note() { printf '  \033[31mFORBIDDEN EDGE\033[0m  %s\n' "$1"; fail=1; }

echo "== bpmn-lite layering guard =="

for src in bpmn-lite-types/src bpmn-lite-engine/src bpmn-lite-store/src \
           bpmn-lite-store-postgres/src bpmn-lite-server/src bpmn-lite-vm/src \
           bpmn-lite-compiler/src bpmn-lite-authoring/src; do
  [ -d "$src" ] || continue
  hits="$(grep -rnE '\bob_poc_types\b' "$src" 2>/dev/null \
    | grep -vE ':[0-9]+:[[:space:]]*//' || true)"
  [ -n "$hits" ] && note "$(dirname $src) references ob_poc_types:
$hits"
done

if [ "$fail" -eq 0 ]; then
  echo "  OK — bpmn-lite has no ob_poc_types dependency."
else
  echo ""
  echo "== Layering guard FAILED =="
fi
exit "$fail"
