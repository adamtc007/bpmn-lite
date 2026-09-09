#!/usr/bin/env bash
# check-domain-nouns.sh — BL-T2 domain-noun gate (EOP-PLAN-CA-REUSE-001).
# Run from the repo root (bpmn-lite/).
#
# bpmn-lite-types is a generic leaf crate: no host-domain vocabulary may
# appear in its types, enums, strings, defaults or docs. The banned nouns
# are matched as whole words, case-insensitively, across the entire crate
# (src, tests, benches). There is NO allowlist — a legitimate-looking hit
# still fails and must be renamed, not exempted.

set -euo pipefail

CRATE_DIR="bpmn-lite-types"
NOUNS='kyc|cbu|onboarding|ca'

if [[ ! -d "$CRATE_DIR" ]]; then
    echo "check-domain-nouns: $CRATE_DIR not found (run from repo root)" >&2
    exit 2
fi

hits=$(grep -rniE "\b(${NOUNS})\b" "$CRATE_DIR" \
    --include='*.rs' --include='*.toml' --include='*.md' || true)

if [[ -n "$hits" ]]; then
    echo "check-domain-nouns: FAIL — banned domain nouns (${NOUNS}) in ${CRATE_DIR}:" >&2
    echo "$hits" >&2
    exit 1
fi

echo "check-domain-nouns: OK — no banned domain nouns in ${CRATE_DIR}"
