# Production operation

The production binary is `bpmn-lite-server`. The unauthenticated in-memory REST
walkthrough is a separate binary, `bpmn-lite-demo`, and refuses to run when
`BPMN_LITE_ENV=production`. Its default bind is `127.0.0.1:0`; a non-loopback
bind requires the explicit acknowledgement documented by `--help`/startup
errors.

Production startup is fail-closed. It requires the PostgreSQL feature and
store, `DATABASE_URL`, configured authentication, TLS (native or explicitly
terminated at a trusted proxy), positive request/scheduler limits, named
effect dispatchers, and retention settings. Database migrations and the
all-tenant recovery scan must complete before the health service is marked
ready. Set `BPMN_LITE_MAX_LATE_DELIVERY_MS` and
`BPMN_LITE_DEDUPE_RETENTION_MS`; startup rejects a dedupe window shorter than
the maximum accepted late delivery.

## Delivery guarantee

Transport delivery is **at least once**. Workflow effect application is
effectively once only when the receiver durably honors the supplied
idempotency key. The engine persists the deterministic effect identity before
dispatch and deduplicates responses before committing the resumed workflow
transition. It does not claim network-level exactly-once delivery.

Retain snapshots/checkpoints, journal records, payload versions, effect rows,
inbox/dedupe identities, incidents, and dead letters according to the maximum
replay and late-delivery windows. Never prune an inbox or dedupe identity while
a conforming sender may still redeliver it.
