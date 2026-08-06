#!/usr/bin/env bash
# run-designer-q9-capture.sh — THE designated Q9 capture deployment.
#
# This is the ONLY sanctioned build surface that enables the q9-capture
# feature (EOP-GOV-Q9-CHARTER-001 §10.2-10.3, ratified by Adam
# 2026-08-06). It is allowlisted by name in check-q9-capture-gate.sh;
# every other build surface mentioning the feature remains a gate
# violation.
#
# Requirements enforced here and by the code (fail closed, both ends):
#   - Q9_CAPTURE_DIR must be set to a repo-private local directory
#     (charter §4/§5: durable, local-machine, no cloud persistence).
#   - The charter reference is pinned HERE and validated by
#     CapturePipeline::under_ratified_charter against the compiled-in
#     ratified constant — a mismatch refuses startup.
set -euo pipefail

: "${Q9_CAPTURE_DIR:?set Q9_CAPTURE_DIR to a repo-private local capture directory (charter §4/§5)}"
export Q9_CHARTER_REF="Q9-CHARTER-001@v1.0"

exec cargo run -p bpmn-lite-server-designer --features q9-capture "$@"
