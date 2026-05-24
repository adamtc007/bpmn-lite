#!/usr/bin/env bash
# reset-demo.sh — clear all three domain stores for a clean demo run.
#
# Usage (from bpmn-lite/):
#   ./scripts/reset-demo.sh
#
# Assumes the docker-compose stack is running (containers healthy).
# If --local is passed, resets against the default local Postgres URLs
# instead of the Docker container names.

set -euo pipefail

LOCAL="${1:-}"

BPMN_LITE_PG="${BPMN_LITE_PG:-postgresql://postgres@localhost:5432/bpmn_lite}"
OB_POC_PG="${OB_POC_PG:-postgresql://postgres@localhost:5433/data_designer}"
DMN_LITE_PG="${DMN_LITE_PG:-postgresql://postgres@localhost:5434/dmn_lite}"

if [[ "$LOCAL" != "--local" ]]; then
    BPMN_LITE_PG="postgresql://postgres@bpmn-lite-postgres/bpmn_lite"
    OB_POC_PG="postgresql://postgres@ob-poc-postgres/data_designer"
    DMN_LITE_PG="postgresql://postgres@dmn-lite-postgres/dmn_lite"
fi

echo "=== Resetting bpmn-lite-postgres ==="
docker compose exec bpmn-lite-postgres psql -U postgres bpmn_lite -c "
    TRUNCATE bpmn_process_instance CASCADE;
    TRUNCATE bpmn_pending_invocation CASCADE;
    TRUNCATE outbox CASCADE;
    TRUNCATE inbox CASCADE;
" 2>/dev/null || \
psql "$BPMN_LITE_PG" -c "
    TRUNCATE bpmn_process_instance CASCADE;
    TRUNCATE bpmn_pending_invocation CASCADE;
    TRUNCATE outbox CASCADE;
    TRUNCATE inbox CASCADE;
"
echo "bpmn-lite reset ✓"

echo "=== Resetting ob-poc-postgres ==="
docker compose exec ob-poc-postgres psql -U postgres data_designer -c "
    TRUNCATE outbox CASCADE;
    TRUNCATE inbox CASCADE;
" 2>/dev/null || \
psql "$OB_POC_PG" -c "
    TRUNCATE outbox CASCADE;
    TRUNCATE inbox CASCADE;
"
echo "ob-poc reset ✓"

echo "=== Resetting dmn-lite-postgres ==="
docker compose exec dmn-lite-postgres psql -U postgres dmn_lite -c "
    TRUNCATE outbox CASCADE;
    TRUNCATE inbox CASCADE;
" 2>/dev/null || \
psql "$DMN_LITE_PG" -c "
    TRUNCATE outbox CASCADE;
    TRUNCATE inbox CASCADE;
"
echo "dmn-lite reset ✓"

echo ""
echo "Demo state reset complete. Run a fresh demo with:"
echo "  cargo run -p bpmn-lite-engine --example demo_run"
