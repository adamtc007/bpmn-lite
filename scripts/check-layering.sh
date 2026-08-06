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
           bpmn-lite-store-postgres/src bpmn-lite-server-runner/src bpmn-lite-server-designer/src \
           bpmn-lite-vm/src \
           bpmn-lite-compiler/src bpmn-lite-authoring/src bpmn-lite-kernel/src; do
  [ -d "$src" ] || continue
  hits="$(grep -rnE '\bob_poc_types\b' "$src" 2>/dev/null \
    | grep -vE ':[0-9]+:[[:space:]]*//' || true)"
  [ -n "$hits" ] && note "$(dirname $src) references ob_poc_types:
$hits"
done

# E4 — the deterministic kernel has one permitted dependency edge and no
# ambient I/O, clock, or random sources.
kernel_manifest="bpmn-lite-kernel/Cargo.toml"
kernel_source="bpmn-lite-kernel/src"
if [ -f "$kernel_manifest" ]; then
  dependency_lines="$(awk '
    /^\[dependencies\]$/ { in_dependencies=1; next }
    /^\[/ { in_dependencies=0 }
    in_dependencies && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ { print }
  ' "$kernel_manifest")"
  unexpected_dependencies="$(printf '%s\n' "$dependency_lines" \
    | grep -vE '^[[:space:]]*bpmn-lite-types[[:space:]]*=' || true)"
  [ -n "$unexpected_dependencies" ] && note "bpmn-lite-kernel has dependencies other than bpmn-lite-types:
$unexpected_dependencies"

  forbidden_kernel_source="$(grep -rnE \
    'SystemTime::now|Uuid::now_v7|thread_rng|std::net|std::fs|TcpStream|UdpSocket|tokio::|sqlx::' \
    "$kernel_source" 2>/dev/null || true)"
  [ -n "$forbidden_kernel_source" ] && note "bpmn-lite-kernel reaches an ambient capability:
$forbidden_kernel_source"
fi

# Shared-contract ownership: BPMN may import these names from SemOS after the
# pinned release, but must never define a local shadow contract/crate.
shadow_contracts="$(rg -n '^[[:space:]]*(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?(struct|enum)[[:space:]]+(CandidateSemanticSlice|SemanticDecisionBoard|InferenceEvidence|InferenceDisposition|ProposalWorkbook|WorkbookSlot|FiniteScore)[[:space:]{]' \
  --glob '*.rs' . 2>/dev/null \
  | rg -v '^utterance-engine/src/contract.rs:.*struct FiniteScore' || true)"
[ -n "$shadow_contracts" ] && note "BPMN defines a shared SemOS contract locally:
$shadow_contracts"

if [ -d "utterance-mapper-core" ] || rg -n 'name[[:space:]]*=[[:space:]]*"utterance-mapper-core"' --glob Cargo.toml . >/dev/null 2>&1; then
  note "forbidden parallel crate utterance-mapper-core exists"
fi

# T10 — engine code that constructs DeterministicContext must obtain ambient
# values through runtime_context.rs, which has deterministic test adapters.
engine_apply_source="bpmn-lite-engine/src/engine.rs"
if [ -f "$engine_apply_source" ]; then
  forbidden_engine_apply="$(grep -nE \
    'SystemTime::now|Uuid::now_v7|thread_rng|rand::' \
    "$engine_apply_source" 2>/dev/null || true)"
  [ -n "$forbidden_engine_apply" ] && note "engine apply path bypasses RuntimeContext:
$forbidden_engine_apply"
fi

if [ "$fail" -eq 0 ]; then
  echo "  OK — layering and deterministic-kernel capability rules hold."
else
  echo ""
  echo "== Layering guard FAILED =="
fi
exit "$fail"
