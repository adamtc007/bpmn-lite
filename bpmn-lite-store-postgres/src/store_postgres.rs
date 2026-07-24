use async_trait::async_trait;
#[cfg(test)]
use bpmn_lite_store::store::{transition_from_tick_ops, TickOperation};
use bpmn_lite_store::store::{
    AdminProjectionStore, ArtifactRepository, JournalReader, RuntimeStore,
};
#[cfg(test)]
use bpmn_lite_store::TemplateSummary;
use bpmn_lite_store::{
    ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreError, StoreResult,
};
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::integrity::compute_instance_integrity_hash;
use bpmn_lite_types::*;
use std::sync::Arc;
use uuid::Uuid;

type Result<T> = StoreResult<T>;

trait PersistenceFailure: Sized {
    fn unavailable(error: impl std::fmt::Display) -> Self;
}

impl PersistenceFailure for StoreError {
    fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

impl PersistenceFailure for ClaimError {
    fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

impl PersistenceFailure for CommitError {
    fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

impl PersistenceFailure for ArtifactStoreError {
    fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

trait IntoPersistenceResult<T> {
    fn persistence<E: PersistenceFailure>(self) -> std::result::Result<T, E>;
}

impl<T, Source> IntoPersistenceResult<T> for std::result::Result<T, Source>
where
    Source: std::fmt::Display,
{
    fn persistence<E: PersistenceFailure>(self) -> std::result::Result<T, E> {
        self.map_err(E::unavailable)
    }
}

const EVENT_NOTIFY_CHANNEL: &str = "bpmn_lite_events";

/// R1 mitigation (c) (EOP-VS-BPMN-ISA-002 §"Named risk R1"): default rate
/// for the sampled canonical round-trip assertion on commit — every 128th
/// revision pays the `decode` + `canonical_bytes` cost to catch canonical-
/// form drift (nondeterministic serialization) live, not just in CI.
/// `BPMN_LITE_CANONICAL_SAMPLE_RATE` overrides it; `0` disables sampling
/// explicitly (an operator opt-out, not a silent default).
const DEFAULT_CANONICAL_SAMPLE_RATE: u64 = 128;

/// `revision % rate == 0` is deterministic across replicas (no RNG, no
/// wall-clock) so the same commit always samples the same way, keeping the
/// check reproducible under `BPMN_LITE_CANONICAL_SAMPLE_RATE=1` in tests.
fn should_sample_canonical_round_trip(revision: u64) -> bool {
    let rate = std::env::var("BPMN_LITE_CANONICAL_SAMPLE_RATE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CANONICAL_SAMPLE_RATE);
    rate != 0 && revision.is_multiple_of(rate)
}

/// Serialize a `Value` into a deterministic string key for dead-letter lookup.
/// Must match MemoryStore's `value_key()` exactly.
fn value_key(v: &Value) -> String {
    match v {
        Value::Bool(b) => format!("b:{b}"),
        Value::I64(n) => format!("i:{n}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Ref(r) => format!("r:{r}"),
        // §18 ruling K Part 2: `Value::Array` is new. Same "a:" + hex of
        // canonical bytes convention as the other `value_key` copies in
        // this workspace.
        Value::Array(_) => format!(
            "a:{}",
            v.to_canonical_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
    }
}

/// Deserialize a JSONB `Vec<Value>` into `[Value; 8]`, padding with `Value::Bool(false)` if short.
/// Convert a `[u8; 32]` BYTEA column loaded as `Vec<u8>` back to `[u8; 32]`.
fn bytes_to_hash(bytes: Vec<u8>) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| StoreError::Integrity(format!("expected 32 bytes, got {}", v.len())))
}

/// Convert an epoch-ms i64 to a `chrono::DateTime<chrono::Utc>` for TIMESTAMPTZ binding.
fn epoch_ms_to_datetime(epoch_ms: i64) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    let secs = epoch_ms / 1000;
    let nanos = ((epoch_ms % 1000) * 1_000_000) as u32;
    // Substituting `Utc::now()` here would silently write the wrong
    // timestamp into a claim/lease/expiry column with no error signal —
    // an out-of-range value means the caller computed a corrupt epoch_ms
    // and that must fail the commit, not be papered over.
    chrono::Utc
        .timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(|| panic!("epoch_ms_to_datetime: {epoch_ms} is out of representable range"))
}

fn datetime_to_epoch_ms(dt: chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp_millis()
}

async fn persist_payload_ref(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    payload: &[u8],
) -> std::result::Result<[u8; 32], CommitError> {
    let payload_hash: [u8; 32] = blake3::hash(payload).into();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_payloads (tenant_id, payload_hash, schema_version, payload)
        VALUES ($1,$2,1,$3)
        ON CONFLICT (tenant_id, payload_hash) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .bind(&payload_hash[..])
    .bind(payload)
    .execute(&mut **tx)
    .await
    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
    if result.rows_affected() == 0 {
        let matches_content: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_payloads WHERE tenant_id = $1 AND payload_hash = $2 AND payload = $3)",
        )
        .bind(tenant_id)
        .bind(&payload_hash[..])
        .bind(payload)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        if !matches_content {
            return Err(CommitError::Integrity(
                "content-addressed payload collision".to_string(),
            ));
        }
    }
    Ok(payload_hash)
}

pub struct TenantTx<'c> {
    pub tx: sqlx::Transaction<'c, sqlx::Postgres>,
    pub tenant_id: String,
    pub lease_owner: String,
}

impl<'c> TenantTx<'c> {
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn lease_owner(&self) -> &str {
        &self.lease_owner
    }

    pub fn assert_rows_affected(
        &self,
        result: &sqlx::postgres::PgQueryResult,
        expected: u64,
        msg: &str,
    ) -> Result<()> {
        let rows = result.rows_affected();
        if rows != expected {
            return Err(StoreError::Integrity(format!(
                "{} (affected {} rows, expected {})",
                msg, rows, expected
            )));
        }
        Ok(())
    }
}

pub async fn execute_tenant_scoped_on_pool<F, T>(
    pool: &sqlx::PgPool,
    tenant_id: &str,
    lease_owner: &str,
    f: F,
) -> Result<T>
where
    F: for<'b, 'c> FnOnce(
            &'b mut TenantTx<'c>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T>> + Send + 'b>,
        > + Send,
    T: Send,
{
    let mut tx = pool.begin().await.map_err(|error| {
        StoreError::Unavailable(format!("execute_tenant_scoped: begin transaction: {error}"))
    })?;

    PostgresWorkflowStore::set_tenant_context(&mut tx, tenant_id)
        .await
        .persistence()?;

    let mut tenant_tx = TenantTx {
        tx,
        tenant_id: tenant_id.to_string(),
        lease_owner: lease_owner.to_string(),
    };

    let result = f(&mut tenant_tx).await;

    if result.is_ok() {
        tenant_tx.tx.commit().await.map_err(|error| {
            StoreError::Unavailable(format!(
                "execute_tenant_scoped: commit transaction: {error}"
            ))
        })?;
    }

    result
}

/// PostgreSQL-backed implementation of `WorkflowStore`.
pub struct PostgresWorkflowStore {
    pool: sqlx::PgPool,
}

impl PostgresWorkflowStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    pub async fn execute_tenant_scoped<F, T>(
        &self,
        tenant_id: &str,
        lease_owner: &str,
        f: F,
    ) -> Result<T>
    where
        F: for<'b, 'c> FnOnce(
                &'b mut TenantTx<'c>,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<T>> + Send + 'b>,
            > + Send,
        T: Send,
    {
        execute_tenant_scoped_on_pool(&self.pool, tenant_id, lease_owner, f).await
    }

    /// A18 — Execute `f` inside a transaction with `app.current_tenant` set via
    /// SET LOCAL. Every gRPC handler that mutates tenant-scoped data must
    /// use this wrapper so that RLS policies (migration 025) see the correct
    /// tenant on every query within the transaction.
    ///
    /// SET LOCAL scopes the setting to the transaction only — it is reset
    /// automatically on commit or rollback, so connection-pool reuse is safe.
    pub async fn with_tenant<F, T>(&self, tenant_id: &str, f: F) -> Result<T>
    where
        F: for<'c> FnOnce(
                &'c mut sqlx::Transaction<'_, sqlx::Postgres>,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<T>> + Send + 'c>,
            > + Send,
        T: Send,
    {
        let mut tx = self.pool.begin().await.map_err(|error| {
            StoreError::Unavailable(format!("with_tenant: begin transaction: {error}"))
        })?;
        Self::set_tenant_context(&mut tx, tenant_id)
            .await
            .persistence()?;
        let result = f(&mut tx).await.persistence()?;
        tx.commit().await.map_err(|error| {
            StoreError::Unavailable(format!("with_tenant: commit transaction: {error}"))
        })?;
        Ok(result)
    }

    /// Expose the inner pool for callers that need ad-hoc executor access
    /// outside of `with_tenant` (e.g. read-only queries, health checks).
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// Run embedded migrations.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("failed to run bpmn-lite migrations: {error}"))
            })?;
        Ok(())
    }

    /// A16 — Set the tenant context for the current transaction.
    ///
    /// Call `SET LOCAL app.current_tenant = <tenant>` at the start of each
    /// transaction so that Row-Level Security policies can filter rows.
    /// `SET LOCAL` scopes the setting to the current transaction only;
    /// it is reset automatically when the transaction commits or rolls back.
    ///
    /// Usage: call this immediately after beginning a transaction, before
    /// any data query. Without this, RLS policies using
    /// `current_setting('app.current_tenant', true)` will return NULL and
    /// no rows will be visible.
    pub async fn set_tenant_context(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant_id: &str,
    ) -> Result<()> {
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(tenant_id)
            .execute(tx.as_mut())
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("failed to set tenant context for RLS: {error}"))
            })?;
        Ok(())
    }

    /// V&S §15 (v0.7) ruling F: the store-side repeated-failure budget.
    /// Called from `commit_transition` with `concurrency_table` already
    /// carrying this transition's own mutations applied (so a
    /// just-cancelled guard's `opened_at` is still resolvable even though
    /// its `RecordState` is now `Retired`).
    ///
    /// - `V2ScopeCancelled` whose `fiber_id` is in `transition.fibers_delete()`
    ///   is ruling C's automatic rollback (the triggering fibre is killed,
    ///   `RollbackCaller::Dies`) — increments the budget, quarantining the
    ///   instance via the existing `quarantine_state` mechanism if
    ///   exhausted (T10.3's claim gate already refuses claims for any
    ///   quarantined instance — no new claim-path code needed). The same
    ///   event with a *surviving* `fiber_id` is an in-line, explicit
    ///   `V2CancelScope` (`RollbackCaller::Continues`) — intentional
    ///   control flow, not a failure, and does not touch the budget.
    /// - `V2GuardRetired` (a guard closing normally via `V2GuardEnd`)
    ///   resets the budget for that guard.
    async fn apply_guard_failure_budget(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant_id: &str,
        instance_id: Uuid,
        transition: &Transition,
        concurrency_table: &ConcurrencyTable,
    ) -> Result<()> {
        // §31: the per-guard escalation ceiling is artifact-resident (the
        // counter stays store-side, below). Loaded lazily — only a guard
        // cancellation reads it — from the instance's pinned artifact, keyed
        // by the guard's `opened_at` address, falling back to the artifact's
        // workflow-level default.
        let has_cancel = transition
            .events()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::V2ScopeCancelled { .. }));
        let (guard_budgets, default_budget) = if has_cancel {
            // Source the per-guard ceiling with a tx-scoped, pure read of the
            // instance's pinned artifact — same connection as the commit, no
            // second pool checkout, no self-healing writes escaping `tx`. A
            // missing or non-canonical pinned artifact on a guard cancellation
            // is an integrity violation, NOT a licence to apply a lenient
            // default that could weaken a stricter declared budget: fail closed.
            let hash = transition.next_snapshot().bytecode_version;
            let bytes: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT canonical_bytes FROM compiled_programs WHERE bytecode_version = $1",
            )
            .bind(&hash[..])
            .fetch_optional(&mut **tx)
            .await
            .persistence()?
            .flatten();
            let Some(bytes) = bytes else {
                return Err(StoreError::Integrity(format!(
                    "guard cancellation on instance {instance_id}: pinned artifact {:?} is \
                     absent or pre-canonical; refusing to apply a guessed failure budget",
                    ArtifactHash::from_bytes(hash)
                )));
            };
            // Full verify (not a bare decode) is defensible here: guard
            // cancellations are rare, and the whole-corpus verify gate proves
            // every stored artifact admissible at cutover — this is the
            // belt-and-suspenders read, not the primary admission point.
            let workflow = ExecutableWorkflow::verify(&bytes).map_err(|error| {
                StoreError::Integrity(format!(
                    "pinned artifact failed verification on guard cancellation: {error}"
                ))
            })?;
            let metadata = workflow.envelope().metadata();
            (
                metadata.v2_guard_budgets().clone(),
                metadata.default_guard_budget(),
            )
        } else {
            (
                std::collections::BTreeMap::new(),
                ScopeFailureBudget::conservative_default(),
            )
        };
        for event in transition.events() {
            match event {
                RuntimeEvent::V2ScopeCancelled { record_id, fiber_id, .. } => {
                    if !transition.fibers_delete().contains(fiber_id) {
                        continue;
                    }
                    let Some(guard_addr) =
                        concurrency_table.get(*record_id).and_then(|record| record.opened_at)
                    else {
                        continue;
                    };
                    let guard_addr_i32 = i32::try_from(guard_addr.get()).map_err(|_| {
                        StoreError::Integrity("guard address exceeds PostgreSQL INTEGER".to_string())
                    })?;
                    let new_count: i32 = sqlx::query_scalar(
                        r#"
                        INSERT INTO guard_failure_budget (tenant_id, instance_id, guard_addr, failure_count, updated_at)
                        VALUES ($1, $2, $3, 1, now())
                        ON CONFLICT (tenant_id, instance_id, guard_addr)
                        DO UPDATE SET failure_count = guard_failure_budget.failure_count + 1, updated_at = now()
                        RETURNING failure_count
                        "#,
                    )
                    .bind(tenant_id)
                    .bind(instance_id)
                    .bind(guard_addr_i32)
                    .fetch_one(&mut **tx)
                    .await
                    .persistence()?;
                    let prior = u32::try_from(new_count).unwrap_or(u32::MAX).saturating_sub(1);
                    let budget = guard_budgets
                        .get(&guard_addr)
                        .copied()
                        .unwrap_or(default_budget);
                    if matches!(budget.decision(prior), ScopeFailureDecision::Exhausted(_)) {
                        sqlx::query(
                            r#"
                            UPDATE workflow_instances
                            SET quarantine_state = 'guard_failure_budget_exhausted',
                                lease_owner = NULL, lease_until = NULL
                            WHERE tenant_id = $1 AND instance_id = $2
                            "#,
                        )
                        .bind(tenant_id)
                        .bind(instance_id)
                        .execute(&mut **tx)
                        .await
                        .persistence()?;
                        tracing::warn!(
                            %instance_id,
                            guard_addr = guard_addr.get(),
                            failure_count = new_count,
                            "guard repeated-failure budget exhausted; instance quarantined"
                        );
                    }
                }
                RuntimeEvent::V2GuardRetired { record_id, .. } => {
                    if let Some(guard_addr) =
                        concurrency_table.get(*record_id).and_then(|record| record.opened_at)
                    {
                        let guard_addr_i32 = i32::try_from(guard_addr.get()).map_err(|_| {
                            StoreError::Integrity("guard address exceeds PostgreSQL INTEGER".to_string())
                        })?;
                        sqlx::query(
                            "DELETE FROM guard_failure_budget WHERE tenant_id = $1 AND instance_id = $2 AND guard_addr = $3",
                        )
                        .bind(tenant_id)
                        .bind(instance_id)
                        .bind(guard_addr_i32)
                        .execute(&mut **tx)
                        .await
                        .persistence()?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn list_tenants(&self) -> Result<Vec<String>> {
        use sqlx::Row;
        let rows = sqlx::query("SELECT tenant_id FROM tenants ORDER BY first_seen_at")
            .fetch_all(&self.pool)
            .await;
        match rows {
            Ok(rows) => {
                let tenants: Vec<String> = rows
                    .iter()
                    .map(|r| r.get::<String, _>("tenant_id"))
                    .collect();
                if tenants.is_empty() {
                    Ok(vec!["default".to_string()])
                } else {
                    Ok(tenants)
                }
            }
            Err(_) => Ok(vec!["default".to_string()]),
        }
    }
}

async fn notify_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    instance_id: Uuid,
) -> Result<()> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(EVENT_NOTIFY_CHANNEL)
        .bind(instance_id.to_string())
        .execute(&mut **tx)
        .await
        .persistence()?;
    Ok(())
}

pub struct StaleReclaimInfo {
    pub job_key: String,
    pub process_instance_id: Uuid,
    pub previous_worker_id: Option<String>,
}

#[async_trait]
impl RuntimeStore for PostgresWorkflowStore {
    // ── Instance ──

    async fn load_instance(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<ProcessInstance>> {
        let tenant_id = tenant_id.to_string();
        let tenant_id_query = tenant_id.clone();
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                SELECT instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                       domain_payload_hash, session_stack, flags, counters, join_expected, state,
                       correlation_id, entry_id, runbook_id,
                       (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms,
                       integrity_hash,
                       quarantine_state,
                       plan_hash,
                       current_node_id,
                       placeholder_values
                FROM workflow_instances
                WHERE tenant_id = $1 AND instance_id = $2
                "#,
                )
                .bind(&tenant_id_query)
                .bind(id)
                .fetch_optional(&mut *tx.tx)
                .await
                .persistence()?;

                match row {
                    None => Ok(None),
                    Some(row) => {
                        use sqlx::Row;
                        let bytecode_version: Vec<u8> = row.get("bytecode_version");
                        let domain_payload_hash: Vec<u8> = row.get("domain_payload_hash");
                        let session_stack_json: serde_json::Value = row.get("session_stack");
                        let flags_json: serde_json::Value = row.get("flags");
                        let counters_json: serde_json::Value = row.get("counters");
                        let join_expected_json: serde_json::Value = row.get("join_expected");
                        let state_json: serde_json::Value = row.get("state");
                        let created_at_ms: i64 = row.get("created_at_ms");
                        let integrity_hash_raw: Option<Vec<u8>> = row.get("integrity_hash");
                        let integrity_hash = integrity_hash_raw.map(bytes_to_hash).transpose()?;
                        let plan_hash_raw: Option<Vec<u8>> = row.get("plan_hash");
                        let plan_hash = plan_hash_raw.map(bytes_to_hash).transpose()?;

                        Ok(Some(ProcessInstance {
                            instance_id: row.get("instance_id"),
                            tenant_id: row.get("tenant_id"),
                            process_key: row.get("process_key"),
                            bytecode_version: bytes_to_hash(bytecode_version)?,
                            domain_payload: Arc::<str>::from(
                                row.get::<String, _>("domain_payload"),
                            ),
                            domain_payload_hash: bytes_to_hash(domain_payload_hash)?,
                            session_stack: serde_json::from_value(session_stack_json)
                                .persistence()?,
                            flags: serde_json::from_value(flags_json).persistence()?,
                            counters: serde_json::from_value(counters_json).persistence()?,
                            join_expected: serde_json::from_value(join_expected_json)
                                .persistence()?,
                            state: serde_json::from_value(state_json).persistence()?,
                            correlation_id: row.get("correlation_id"),
                            entry_id: row.get("entry_id"),
                            runbook_id: row.get("runbook_id"),
                            created_at: created_at_ms,
                            integrity_hash,
                            quarantine_state: row.get("quarantine_state"),
                            plan_hash,
                            current_node_id: row.get("current_node_id"),
                            placeholder_values: row.get("placeholder_values"),
                        }))
                    }
                }
            })
        })
        .await
    }

    // ── Fibers ──

    async fn load_fiber(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        fiber_id: Uuid,
    ) -> StoreResult<Option<Fiber>> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;

        let row = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE tenant_id = $1 AND instance_id = $2 AND fiber_id = $3",
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .bind(fiber_id)
        .fetch_optional(&mut *tx)
        .await.persistence()?;

        tx.commit().await.persistence()?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let pc: i32 = row.get("pc");
                let stack_json: serde_json::Value = row.get("stack");
                let wait_json: serde_json::Value = row.get("wait_state");
                let loop_epoch: i32 = row.get("loop_epoch");

                Ok(Some(Fiber {
                    fiber_id: row.get("fiber_id"),
                    pc: Addr::new(pc as u32),
                    stack: serde_json::from_value(stack_json).persistence()?,
                    wait: serde_json::from_value(wait_json).persistence()?,
                    loop_epoch: loop_epoch as u32,
                    // No control_stack column yet — V2 adds it under the
                    // SnapshotSchema tripwire (V&S §8). No v2 word exists
                    // to have populated one before V4, so empty is exact.
                    control_stack: Vec::new(),
                }))
            }
        }
    }

    async fn load_fibers(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Fiber>> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;

        let rows = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .fetch_all(&mut *tx)
        .await.persistence()?;

        tx.commit().await.persistence()?;

        let mut fibers = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row;
            let pc: i32 = row.get("pc");
            let stack_json: serde_json::Value = row.get("stack");
            let wait_json: serde_json::Value = row.get("wait_state");
            let loop_epoch: i32 = row.get("loop_epoch");

            fibers.push(Fiber {
                fiber_id: row.get("fiber_id"),
                pc: Addr::new(pc as u32),
                stack: serde_json::from_value(stack_json).persistence()?,
                wait: serde_json::from_value(wait_json).persistence()?,
                loop_epoch: loop_epoch as u32,
                control_stack: Vec::new(),
            });
        }
        Ok(fibers)
    }

    // ── Join barriers ──

    // ── Dedupe cache ──

    async fn dedupe_get(
        &self,
        tenant_id: &TenantId,
        key: &str,
    ) -> StoreResult<Option<JobCompletion>> {
        let key = key.to_string();
        self.with_tenant(tenant_id.as_str(), |tx| {
            Box::pin(async move {
                let row = sqlx::query("SELECT completion FROM dedupe_cache WHERE job_key = $1")
                    .bind(&key)
                    .fetch_optional(&mut **tx)
                    .await
                    .persistence()?;

                match row {
                    None => Ok(None),
                    Some(row) => {
                        use sqlx::Row;
                        let json: serde_json::Value = row.get("completion");
                        Ok(Some(serde_json::from_value(json).persistence()?))
                    }
                }
            })
        })
        .await
    }

    // ── Job queue ──

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &TenantId,
        worker_id: &str,
        lease_ms: u64,
    ) -> StoreResult<Vec<JobActivation>> {
        let task_types = task_types.to_vec();
        let tenant_id_owned = tenant_id.to_string();
        let worker_id_owned = worker_id.to_string();
        self.execute_tenant_scoped(&tenant_id_owned, &worker_id_owned, |tx| {
            Box::pin(async move { Self::dequeue_jobs_inner(tx, &task_types, max, lease_ms).await })
        })
        .await
    }

    async fn validate_job_claim(
        &self,
        tenant_id: &TenantId,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> StoreResult<bool> {
        let lease_owner = "unused";
        let tenant_id = tenant_id.to_string();
        let job_key = job_key.to_string();
        let worker_id = worker_id.to_string();
        let claim_token = claim_token.to_string();
        self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                SELECT 1
                FROM job_queue
                WHERE job_key = $1
                  AND status = 'claimed'
                  AND worker_id = $2
                  AND claim_token = $3
                  AND claim_expires_at > now()
                  AND retries_remaining > 1
                "#,
                )
                .bind(&job_key)
                .bind(&worker_id)
                .bind(&claim_token)
                .fetch_optional(&mut *tx.tx)
                .await
                .persistence()?;
                Ok(row.is_some())
            })
        })
        .await
    }

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> StoreResult<()> {
        let key = value_key(corr_key);
        let expires_at = chrono::Utc::now() + chrono::Duration::milliseconds(ttl_ms as i64);

        sqlx::query(
            r#"
            INSERT INTO dead_letter_queue (name, corr_key, payload, expires_at)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (name, corr_key) DO UPDATE SET
                payload = EXCLUDED.payload,
                expires_at = EXCLUDED.expires_at
            "#,
        )
        .bind(name as i32)
        .bind(&key)
        .bind(payload)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .persistence()?;
        Ok(())
    }

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> StoreResult<Option<Vec<u8>>> {
        let key = value_key(corr_key);

        let row = sqlx::query(
            "DELETE FROM dead_letter_queue WHERE name = $1 AND corr_key = $2 AND expires_at > now() RETURNING payload",
        )
        .bind(name as i32)
        .bind(&key)
        .fetch_optional(&self.pool)
        .await.persistence()?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                Ok(Some(row.get("payload")))
            }
        }
    }

    async fn claim_buffered_message(
        &self,
        tenant_id: &TenantId,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> StoreResult<Option<ClaimedBufferedMessage>> {
        let claim_until_ms = (chrono::Utc::now() + chrono::Duration::milliseconds(claim_ms as i64))
            .timestamp_millis();
        let claim_until = epoch_ms_to_datetime(claim_until_ms);
        let claim_token = Uuid::now_v7().to_string();
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;

        let row = sqlx::query(
            r#"
            WITH picked AS (
                SELECT tenant_id, message_name, correlation_key, msg_id
                FROM message_buffer
                WHERE tenant_id = $1
                  AND message_name = $2
                  AND correlation_key = $3
                  AND consumed_at IS NULL
                  AND expires_at > now()
                  AND (claim_token IS NULL OR claim_until <= now())
                ORDER BY received_at
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE message_buffer
            SET claim_token = $4,
                claimed_at = now(),
                claim_until = $5,
                status = 'claimed'
            FROM picked
            WHERE message_buffer.tenant_id = picked.tenant_id
              AND message_buffer.message_name = picked.message_name
              AND message_buffer.correlation_key = picked.correlation_key
              AND message_buffer.msg_id = picked.msg_id
            RETURNING message_buffer.tenant_id,
                      message_buffer.message_name,
                      message_buffer.correlation_key,
                      message_buffer.msg_id,
                      message_buffer.payload,
                      message_buffer.payload_hash,
                      message_buffer.process_instance_id,
                      message_buffer.received_at,
                      message_buffer.expires_at
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(message_name)
        .bind(correlation_key)
        .bind(&claim_token)
        .bind(claim_until)
        .fetch_optional(&mut *tx)
        .await
        .persistence()?;

        tx.commit().await.persistence()?;

        let Some(row) = row else {
            return Ok(None);
        };
        use sqlx::Row;
        let payload_hash: Option<Vec<u8>> = row.get("payload_hash");
        let received_at: chrono::DateTime<chrono::Utc> = row.get("received_at");
        let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
        Ok(Some(ClaimedBufferedMessage {
            message: BufferedMessage {
                tenant_id: row.get("tenant_id"),
                message_name: row.get("message_name"),
                correlation_key: row.get("correlation_key"),
                msg_id: row.get("msg_id"),
                payload: row.get("payload"),
                payload_hash: payload_hash.map(bytes_to_hash).transpose()?,
                process_instance_id: row.get("process_instance_id"),
                received_at: datetime_to_epoch_ms(received_at),
                expires_at: datetime_to_epoch_ms(expires_at),
            },
            claim_token,
            claim_until: datetime_to_epoch_ms(claim_until),
        }))
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32> {
        let tenants = self.list_tenants().await.persistence()?;
        let mut total_affected = 0;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await.persistence()?;
            if Self::set_tenant_context(&mut tx, &tenant_id).await.is_err() {
                continue;
            }
            let result = sqlx::query(
                r#"
                UPDATE message_buffer
                SET claim_token = NULL,
                    claimed_at = NULL,
                    claim_until = NULL,
                    status = 'buffered'
                WHERE consumed_at IS NULL
                  AND claim_token IS NOT NULL
                  AND claim_until <= now()
                "#,
            )
            .execute(&mut *tx)
            .await
            .persistence()?;
            tx.commit().await.persistence()?;
            total_affected += result.rows_affected() as u32;
        }
        Ok(total_affected)
    }

    async fn prune_expired_messages(&self) -> StoreResult<u32> {
        let tenants = self.list_tenants().await.persistence()?;
        let mut total_pruned = 0;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await.persistence()?;
            if Self::set_tenant_context(&mut tx, &tenant_id).await.is_err() {
                continue;
            }
            let rows = sqlx::query(
                r#"
                DELETE FROM message_buffer
                WHERE consumed_at IS NULL
                  AND expires_at <= now()
                RETURNING process_instance_id, message_name, correlation_key, msg_id
                "#,
            )
            .fetch_all(&mut *tx)
            .await
            .persistence()?;

            use sqlx::Row;
            for row in &rows {
                let instance_id: Option<Uuid> = row.get("process_instance_id");
                if let Some(instance_id) = instance_id {
                    let event = RuntimeEvent::BufferedMessageExpired {
                        message_name: row.get("message_name"),
                        correlation_key: row.get("correlation_key"),
                        msg_id: row.get("msg_id"),
                    };
                    let event_json = serde_json::to_value(&event).persistence()?;

                    sqlx::query(
                        r#"
                        WITH seq AS (
                            INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                            VALUES ($1, 1, $3)
                            ON CONFLICT (instance_id) DO UPDATE
                                SET next_seq = event_sequences.next_seq + 1
                            RETURNING next_seq
                        )
                        INSERT INTO event_log (instance_id, seq, event, tenant_id)
                        SELECT $1, seq.next_seq, $2, $3
                        FROM seq
                        "#,
                    )
                    .bind(instance_id)
                    .bind(&event_json)
                    .bind(&tenant_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| StoreError::Unavailable(format!("prune_expired_messages: failed to append BufferedMessageExpired event: {error}")))?;

                    notify_event_tx(&mut tx, instance_id).await.persistence()?;
                }
            }

            tx.commit().await.persistence()?;
            total_pruned += rows.len() as u32;
        }
        Ok(total_pruned)
    }

    // ── Incidents ──

    async fn load_incidents(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Incident>> {
        let tenant_id = tenant_id.to_string();
        let tenant_id_query = tenant_id.clone();
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                SELECT incident_id, process_instance_id, fiber_id, service_task_id,
                       bytecode_addr, error_class, message, retry_count,
                       (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms,
                       (EXTRACT(EPOCH FROM resolved_at) * 1000)::BIGINT AS resolved_at_ms,
                       resolution
                FROM incidents
                WHERE tenant_id = $1 AND process_instance_id = $2
                ORDER BY created_at
                "#,
                )
                .bind(&tenant_id_query)
                .bind(instance_id)
                .fetch_all(&mut *tx.tx)
                .await
                .persistence()?;

                let mut incidents = Vec::with_capacity(rows.len());
                for row in rows {
                    use sqlx::Row;
                    let bytecode_addr: i32 = row.get("bytecode_addr");
                    let error_class_json: serde_json::Value = row.get("error_class");
                    let retry_count: i32 = row.get("retry_count");
                    let created_at_ms: i64 = row.get("created_at_ms");
                    let resolved_at_ms: Option<i64> = row.get("resolved_at_ms");

                    incidents.push(Incident {
                        incident_id: row.get("incident_id"),
                        process_instance_id: row.get("process_instance_id"),
                        fiber_id: row.get("fiber_id"),
                        service_task_id: row.get("service_task_id"),
                        bytecode_addr: Addr::new(bytecode_addr as u32),
                        error_class: serde_json::from_value(error_class_json).persistence()?,
                        message: row.get("message"),
                        retry_count: retry_count as u32,
                        created_at: created_at_ms,
                        resolved_at: resolved_at_ms,
                        resolution: row.get("resolution"),
                    });
                }
                Ok(incidents)
            })
        })
        .await
    }

    // ── Durability maintenance ──

    async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> StoreResult<u32> {
        let lease_owner = "unused";
        let tenants = self.list_tenants().await.persistence()?;
        let mut total_count = 0;
        for tenant_id in tenants {
            let reclaims = self
                .execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
                    Box::pin(async move { Self::reclaim_stale_jobs_inner(tx, timeout_ms).await })
                })
                .await
                .persistence()?;

            total_count += reclaims.len() as u32;
        }
        Ok(total_count)
    }

    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32> {
        let row = sqlx::query(
            r#"
            WITH deleted AS (
                DELETE FROM dedupe_cache
                WHERE created_at < now() - make_interval(secs => $1::float / 1000.0)
                RETURNING job_key
            )
            SELECT count(*) AS cnt FROM deleted
            "#,
        )
        .bind(older_than_ms as f64)
        .fetch_one(&self.pool)
        .await
        .persistence()?;

        use sqlx::Row;
        let cnt: i64 = row.get("cnt");
        Ok(cnt as u32)
    }

    async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>> {
        let tenant_id_owned = tenant_id.to_string();
        let tenant_id_query = tenant_id.to_string();
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id_owned, lease_owner, |tx| Box::pin(async move {
            let rows = sqlx::query(
                r#"SELECT instance_id FROM workflow_instances WHERE tenant_id = $1 AND state = '"Running"'::jsonb"#,
            )
            .bind(&tenant_id_query)
            .fetch_all(&mut *tx.tx)
            .await.persistence()?;

            use sqlx::Row;
            Ok(rows.iter().map(|r| r.get("instance_id")).collect())
        })).await
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<Uuid>> {
        let tenant_id_owned = tenant_id.to_string();
        let owner_owned = owner.to_string();
        let tenant_id_query = tenant_id.to_string();
        let owner_query = owner.to_string();
        self.execute_tenant_scoped(&tenant_id_owned, &owner_owned, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                WITH candidates AS (
                    SELECT instance_id
                    FROM workflow_instances
                    WHERE tenant_id = $1
                      AND state = '"Running"'::jsonb
                      AND quarantine_state IS NULL
                      AND (lease_until IS NULL OR lease_until < now() OR lease_owner = $2)
                    ORDER BY updated_at
                    LIMIT $3
                    FOR UPDATE SKIP LOCKED
                )
                UPDATE workflow_instances
                SET lease_owner = $2,
                    lease_until = now() + make_interval(secs => $4::float / 1000.0),
                    last_tick_at = now(),
                    fence = CASE
                        WHEN lease_owner = $2 AND lease_until >= now() THEN fence
                        ELSE fence + 1
                    END
                FROM candidates
                WHERE workflow_instances.instance_id = candidates.instance_id
                RETURNING workflow_instances.instance_id
                "#,
                )
                .bind(&tenant_id_query)
                .bind(&owner_query)
                .bind(limit as i64)
                .bind(lease_ms as f64)
                .fetch_all(&mut *tx.tx)
                .await
                .persistence()?;

                use sqlx::Row;
                Ok(rows.iter().map(|r| r.get("instance_id")).collect())
            })
        })
        .await
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> std::result::Result<Option<Claim>, ClaimError> {
        self.claim_work_for_transition(
            tenant_id,
            instance_id,
            owner,
            lease_ms,
            Command::Tick { fiber_id: None },
        )
        .await
        .map(|work| work.map(|work| work.claim().clone()))
    }

    async fn claim_work_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
        command: Command,
    ) -> std::result::Result<Option<ClaimedWork>, ClaimError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .map_err(|error| ClaimError::Unavailable(error.to_string()))?;

        let row = sqlx::query(
            r#"
            UPDATE workflow_instances
            SET lease_owner = $3,
                lease_until = now() + make_interval(secs => $4::float / 1000.0),
                last_tick_at = now(),
                fence = CASE
                    WHEN lease_owner = $3 AND lease_until >= now() THEN fence
                    ELSE fence + 1
                END
            WHERE tenant_id = $1
              AND instance_id = $2
              AND quarantine_state IS NULL
              AND (lease_until IS NULL OR lease_until < now() OR lease_owner = $3)
            RETURNING revision, fence, bytecode_version, snapshot_schema_version,
                      artifact_abi, snapshot_envelope, frame_hash
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .bind(owner)
        .bind(lease_ms as f64)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ClaimError::Unavailable(error.to_string()))?;

        let Some(row) = row else {
            tx.commit()
                .await
                .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
            return Ok(None);
        };
        use sqlx::Row;
        let revision: i64 = row.get("revision");
        let fence: i64 = row.get("fence");
        let bytecode_version: Vec<u8> = row.get("bytecode_version");
        let snapshot_schema_version: Option<i16> = row.get("snapshot_schema_version");
        let artifact_abi: Option<i32> = row.get("artifact_abi");
        let snapshot_bytes: Option<Vec<u8>> = row.get("snapshot_envelope");
        let stored_frame_hash: Option<Vec<u8>> = row.get("frame_hash");
        let integrity_result = async {
            let revision_u64 =
                u64::try_from(revision).map_err(|_| "negative instance revision".to_string())?;
            let snapshot_bytes = snapshot_bytes
                .as_deref()
                .ok_or_else(|| "missing snapshot envelope".to_string())?;
            // D3 Ring 1: verify the physical-integrity hash over the RAW
            // bytes before any deserialization is attempted — a corrupted
            // frame never reaches the deserializer.
            let stored_frame_hash = stored_frame_hash
                .as_deref()
                .ok_or_else(|| "missing frame hash".to_string())?;
            if blake3::hash(snapshot_bytes).as_bytes().as_slice() != stored_frame_hash {
                return Err(IntegrityError::Ring1Physical("frame hash mismatch on raw bytes (pre-decode)".to_string()).to_string());
            }
            let snapshot =
                SnapshotEnvelope::decode(snapshot_bytes).map_err(|error| error.to_string())?;
            let canonical = snapshot
                .canonical_bytes()
                .map_err(|error| error.to_string())?;
            if canonical != snapshot_bytes {
                return Err("non-canonical snapshot envelope".to_string());
            }
            if snapshot_schema_version != i16::try_from(snapshot.schema_version()).ok()
                || artifact_abi != i32::try_from(snapshot.artifact_abi()).ok()
            {
                return Err("snapshot envelope metadata columns diverge".to_string());
            }
            let artifact_hash =
                bytes_to_hash(bytecode_version.clone()).map_err(|error| error.to_string())?;
            if snapshot.revision() != revision_u64
                || snapshot.state().instance().instance_id != instance_id
                || snapshot.state().instance().tenant_id != tenant_id.as_str()
                || snapshot.state().instance().bytecode_version != artifact_hash
            {
                return Err("snapshot aggregate binding diverges".to_string());
            }
            let journal_row = sqlx::query(
                r#"
                SELECT new_revision, prior_state_hash, state_hash, artifact_hash, record_envelope
                FROM workflow_journal
                WHERE tenant_id = $1 AND instance_id = $2
                ORDER BY new_revision DESC
                LIMIT 1
                "#,
            )
            .bind(tenant_id.as_str())
            .bind(instance_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing journal head".to_string())?;
            let journal_revision: i64 = journal_row.get("new_revision");
            let journal_prior_state_hash: Vec<u8> = journal_row.get("prior_state_hash");
            let journal_state_hash: Vec<u8> = journal_row.get("state_hash");
            let journal_artifact_hash: Vec<u8> = journal_row.get("artifact_hash");
            let journal_bytes: Vec<u8> = journal_row.get("record_envelope");
            let journal =
                JournalRecord::decode(&journal_bytes).map_err(|error| error.to_string())?;
            if journal_revision != revision
                || journal.new_revision() != revision_u64
                || bytes_to_hash(journal_state_hash).map_err(|error| error.to_string())?
                    != snapshot.state_hash().map_err(|error| error.to_string())?
                || bytes_to_hash(journal_artifact_hash).map_err(|error| error.to_string())?
                    != artifact_hash
                || journal.state_hash()
                    != snapshot.state_hash().map_err(|error| error.to_string())?
                || journal.artifact_hash() != artifact_hash
                || bytes_to_hash(journal_prior_state_hash).map_err(|error| error.to_string())?
                    != journal.prior_state_hash()
            {
                return Err(IntegrityError::Ring2Frame("snapshot and journal head diverge".to_string()).to_string());
            }
            // D3 Ring 2 chain: this record's prior_state_hash must equal the
            // previous record's state_hash (genesis: prior_state_hash is zero).
            if journal.prior_revision() < 0 {
                if journal.prior_state_hash() != [0u8; 32] {
                    return Err(IntegrityError::Ring2Frame("genesis journal record has non-zero prior_state_hash".to_string()).to_string());
                }
            } else {
                let previous_state_hash: Vec<u8> = sqlx::query_scalar(
                    "SELECT state_hash FROM workflow_journal WHERE tenant_id = $1 AND instance_id = $2 AND new_revision = $3",
                )
                .bind(tenant_id.as_str())
                .bind(instance_id)
                .bind(journal.prior_revision())
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| IntegrityError::Ring2Frame("journal chain broken: prior revision missing".to_string()).to_string())?;
                if bytes_to_hash(previous_state_hash).map_err(|error| error.to_string())?
                    != journal.prior_state_hash()
                {
                    return Err(IntegrityError::Ring2Frame("journal chain broken: prior_state_hash does not match previous record's state_hash".to_string()).to_string());
                }
            }
            Ok::<(), String>(())
        }
        .await;

        if let Err(reason) = integrity_result {
            sqlx::query(
                r#"
                UPDATE workflow_instances
                SET quarantine_state = 'replay_integrity_violation',
                    lease_owner = NULL, lease_until = NULL
                WHERE tenant_id = $1 AND instance_id = $2
                "#,
            )
            .bind(tenant_id.as_str())
            .bind(instance_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
            // Detection is fail-stop; recovery is point-in-time restore. The
            // operator only learns to restore from the audit log, so the
            // claim-path quarantine must emit the same InstanceQuarantined
            // event as the explicit op — never a silent column set.
            let event = RuntimeEvent::InstanceQuarantined {
                instance_id,
                tenant_id: tenant_id.as_str().to_string(),
                detection_point: "scheduler_claim".to_string(),
                failure_reason: reason.clone(),
                detected_at: chrono::Utc::now().timestamp_millis(),
            };
            let event_json = serde_json::to_value(&event)
                .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
            sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                    VALUES ($1, 1, $3)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq, tenant_id
                )
                INSERT INTO event_log (instance_id, seq, event, tenant_id)
                SELECT $1, seq.next_seq, $2, seq.tenant_id
                FROM seq
                "#,
            )
            .bind(instance_id)
            .bind(&event_json)
            .bind(tenant_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
            notify_event_tx(&mut tx, instance_id)
                .await
                .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
            tx.commit()
                .await
                .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
            return Err(ClaimError::Integrity(reason));
        }

        tx.commit()
            .await
            .map_err(|error| ClaimError::Unavailable(error.to_string()))?;
        let revision = u64::try_from(revision).map_err(|_| {
            ClaimError::Invalid("negative revision in workflow_instances".to_string())
        })?;
        let fence = u64::try_from(fence)
            .map_err(|_| ClaimError::Invalid("negative fence in workflow_instances".to_string()))?;
        let tenant_id = tenant_id.clone();
        let snapshot_bytes = snapshot_bytes
            .as_deref()
            .ok_or_else(|| ClaimError::Integrity("missing snapshot envelope".to_string()))?;
        let snapshot = SnapshotEnvelope::decode(snapshot_bytes)
            .map_err(|error| ClaimError::Integrity(error.to_string()))?
            .state()
            .to_runtime_snapshot();
        let claim = Claim::new(tenant_id, instance_id, revision, fence);
        Ok(Some(ClaimedWork::new(claim, snapshot, command)))
    }

    async fn commit_transition(
        &self,
        claim: &Claim,
        transition: &Transition,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        let snapshot = transition.next_snapshot();
        if snapshot.instance_id != claim.instance_id()
            || snapshot.tenant_id != claim.tenant_id().as_str()
        {
            return Err(CommitError::Integrity(
                "claim and transition aggregate identity differ".to_string(),
            ));
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        Self::set_tenant_context(&mut tx, claim.tenant_id().as_str())
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;

        let flags = serde_json::to_value(&snapshot.flags)
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let counters = serde_json::to_value(&snapshot.counters)
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let join_expected = serde_json::to_value(&snapshot.join_expected)
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let state = serde_json::to_value(transition.state_override().unwrap_or(&snapshot.state))
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let session_stack = serde_json::to_value(&snapshot.session_stack)
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let integrity_hash = compute_instance_integrity_hash(snapshot);

        let update =
            sqlx::query(
                r#"
            UPDATE workflow_instances
            SET domain_payload = $1,
                domain_payload_hash = $2,
                session_stack = $3,
                flags = $4,
                counters = $5,
                join_expected = $6,
                state = $7,
                correlation_id = $8,
                plan_hash = $9,
                current_node_id = $10,
                placeholder_values = $11,
                integrity_hash = $12,
                revision = revision + 1
            WHERE tenant_id = $13
              AND instance_id = $14
              AND revision = $15
              AND fence = $16
            "#,
            )
            .bind(snapshot.domain_payload.as_ref())
            .bind(&snapshot.domain_payload_hash[..])
            .bind(&session_stack)
            .bind(&flags)
            .bind(&counters)
            .bind(&join_expected)
            .bind(&state)
            .bind(&snapshot.correlation_id)
            .bind(snapshot.plan_hash.as_ref().map(|hash| hash.as_slice()))
            .bind(snapshot.current_node_id.as_deref())
            .bind(snapshot.placeholder_values.as_ref())
            .bind(&integrity_hash[..])
            .bind(claim.tenant_id().as_str())
            .bind(claim.instance_id())
            .bind(i64::try_from(claim.expected_revision()).map_err(|_| {
                CommitError::Integrity("revision exceeds PostgreSQL BIGINT".to_string())
            })?)
            .bind(i64::try_from(claim.fence()).map_err(|_| {
                CommitError::Integrity("fence exceeds PostgreSQL BIGINT".to_string())
            })?)
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;

        let mut inserted_start = false;
        if update.rows_affected() != 1 {
            let current = sqlx::query(
                "SELECT revision, fence FROM workflow_instances WHERE tenant_id = $1 AND instance_id = $2",
            )
            .bind(claim.tenant_id().as_str())
            .bind(claim.instance_id())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if let Some(current) = current {
                use sqlx::Row;
                let current_fence: i64 = current.get("fence");
                let claimed_fence = i64::try_from(claim.fence()).map_err(|_| {
                    CommitError::Integrity("fence exceeds PostgreSQL BIGINT".to_string())
                })?;
                if current_fence != claimed_fence {
                    return Err(CommitError::StaleFence);
                }
                return Err(CommitError::Conflict);
            }
            if claim.expected_revision() != 0 || claim.fence() != 0 {
                return Err(CommitError::Integrity("instance not found".to_string()));
            }

            let created_at = epoch_ms_to_datetime(snapshot.created_at);
            sqlx::query(
                r#"
                INSERT INTO workflow_instances (
                    instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                    domain_payload_hash, session_stack, flags, counters, join_expected, state,
                    correlation_id, entry_id, runbook_id, created_at, integrity_hash,
                    plan_hash, current_node_id, placeholder_values, revision, fence
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,0,0)
                "#,
            )
            .bind(snapshot.instance_id)
            .bind(&snapshot.tenant_id)
            .bind(&snapshot.process_key)
            .bind(&snapshot.bytecode_version[..])
            .bind(snapshot.domain_payload.as_ref())
            .bind(&snapshot.domain_payload_hash[..])
            .bind(&session_stack)
            .bind(&flags)
            .bind(&counters)
            .bind(&join_expected)
            .bind(&state)
            .bind(&snapshot.correlation_id)
            .bind(snapshot.entry_id)
            .bind(snapshot.runbook_id)
            .bind(created_at)
            .bind(&integrity_hash[..])
            .bind(snapshot.plan_hash.as_ref().map(|hash| hash.as_slice()))
            .bind(snapshot.current_node_id.as_deref())
            .bind(snapshot.placeholder_values.as_ref())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            inserted_start = true;
        }

        if let Some(start) = transition.start_dedupe() {
            let command = start.command();
            if !inserted_start {
                let existing: Option<Uuid> = sqlx::query_scalar(
                    "SELECT instance_id FROM bpmn_spawn_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
                )
                .bind(command.tenant_id().as_str())
                .bind(command.idempotency_key())
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                tx.rollback()
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                return if existing == Some(command.instance_id()) {
                    Ok(CommitOutcome::IdempotentNoOp)
                } else {
                    Err(CommitError::Conflict)
                };
            }
            if command.tenant_id() != claim.tenant_id()
                || command.instance_id() != claim.instance_id()
                || command.artifact_hash() != snapshot.bytecode_version
                || command.initial_payload_hash() != snapshot.domain_payload_hash
                || command.entry_id() != snapshot.entry_id
                || command.runbook_id() != snapshot.runbook_id
                || command.correlation_id() != snapshot.correlation_id
                || command.initial_payload() != snapshot.domain_payload.as_ref()
            {
                return Err(CommitError::Integrity(
                    "start command lineage does not match initial snapshot".to_string(),
                ));
            }
            let inserted = sqlx::query(
                r#"
                INSERT INTO bpmn_spawn_idempotency
                    (tenant_id, idempotency_key, instance_id, schema_version,
                     artifact_hash, entry_id, runbook_id, initial_payload_hash, created_at)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,to_timestamp($9::float / 1000.0))
                ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
                "#,
            )
            .bind(command.tenant_id().as_str())
            .bind(command.idempotency_key())
            .bind(command.instance_id())
            .bind(i16::try_from(command.schema_version()).map_err(|_| {
                CommitError::Integrity("start command schema version exceeds SMALLINT".to_string())
            })?)
            .bind(&command.artifact_hash()[..])
            .bind(command.entry_id())
            .bind(command.runbook_id())
            .bind(&command.initial_payload_hash()[..])
            .bind(command.logical_time() as f64)
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if inserted.rows_affected() != 1 {
                tx.rollback()
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                return Ok(CommitOutcome::IdempotentNoOp);
            }
        }

        for fiber in transition.fibers_upsert() {
            let stack = serde_json::to_value(&fiber.stack)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let regs = serde_json::to_value(Vec::<bpmn_lite_types::Value>::new())
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let wait = serde_json::to_value(&fiber.wait)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            sqlx::query(
                r#"
                INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (instance_id, fiber_id) DO UPDATE SET
                    pc = EXCLUDED.pc, stack = EXCLUDED.stack, regs = EXCLUDED.regs,
                    wait_state = EXCLUDED.wait_state, loop_epoch = EXCLUDED.loop_epoch
                "#,
            )
            .bind(claim.instance_id())
            .bind(fiber.fiber_id)
            .bind(fiber.pc.get() as i32)
            .bind(stack)
            .bind(regs)
            .bind(wait)
            .bind(fiber.loop_epoch as i32)
            .bind(claim.tenant_id().as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for fiber_id in transition.fibers_delete() {
            sqlx::query("DELETE FROM fibers WHERE instance_id = $1 AND fiber_id = $2")
                .bind(claim.instance_id())
                .bind(fiber_id)
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for job in transition.jobs_enqueue() {
            let payload_ref_hash = persist_payload_ref(
                &mut tx,
                claim.tenant_id().as_str(),
                job.domain_payload.as_bytes(),
            )
            .await
            .persistence()?;
            if payload_ref_hash != job.domain_payload_hash {
                return Err(CommitError::Integrity(
                    "job payload hash does not match payload bytes".to_string(),
                ));
            }
            let session_stack = serde_json::to_value(&job.session_stack)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let flags = serde_json::to_value(&job.orch_flags)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            sqlx::query(
                r#"
                INSERT INTO job_queue
                    (job_key, tenant_id, process_instance_id, task_type, service_task_id,
                     domain_payload, domain_payload_hash, payload_ref_hash, session_stack, orch_flags,
                     retries_remaining, entry_id, runbook_id, status)
                VALUES ($1,$2,$3,$4,$5,'',$6,$6,$7,$8,$9,$10,$11,'pending')
                ON CONFLICT (job_key) DO NOTHING
                "#,
            )
            .bind(&job.job_key)
            .bind(&job.tenant_id)
            .bind(job.process_instance_id)
            .bind(&job.task_type)
            .bind(&job.service_task_id)
            .bind(&job.domain_payload_hash[..])
            .bind(session_stack)
            .bind(flags)
            .bind(job.retries_remaining as i32)
            .bind(job.entry_id)
            .bind(job.runbook_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for job_key in transition.jobs_ack() {
            sqlx::query("DELETE FROM job_queue WHERE tenant_id = $1 AND job_key = $2")
                .bind(claim.tenant_id().as_str())
                .bind(job_key)
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for mutation in transition.job_mutations() {
            let result = match mutation {
                JobMutation::RetryClaimed {
                    job_key,
                    worker_id,
                    claim_token,
                    error_class,
                    error_message,
                    not_before_ms,
                } => {
                    sqlx::query(
                        r#"
                        UPDATE job_queue
                        SET status = 'pending', claimed_at = NULL, worker_id = NULL,
                            claim_token = NULL, claim_expires_at = NULL, not_before = $6,
                            retries_remaining = GREATEST(retries_remaining - 1, 0),
                            failure_count = failure_count + 1, last_failed_at = now(),
                            last_error_class = $7, last_error_message = $8, last_error = $8
                        WHERE tenant_id = $1 AND job_key = $2 AND status = 'claimed'
                          AND worker_id = $3 AND claim_token = $4 AND claim_expires_at > now()
                          AND retries_remaining > $5
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(job_key)
                    .bind(worker_id)
                    .bind(claim_token)
                    .bind(1i32)
                    .bind(epoch_ms_to_datetime(*not_before_ms))
                    .bind(error_class)
                    .bind(error_message)
                    .execute(&mut *tx)
                    .await
                }
                JobMutation::DeadLetterClaimed {
                    job_key,
                    worker_id,
                    claim_token,
                    error_class,
                    error_message,
                    incident_id,
                } => {
                    sqlx::query(
                        r#"
                        UPDATE job_queue
                        SET status = 'dead_lettered', claimed_at = NULL, worker_id = NULL,
                            claim_token = NULL, claim_expires_at = NULL,
                            failure_count = failure_count + 1, last_failed_at = now(),
                            dead_lettered_at = now(), last_error_class = $5,
                            last_error_message = $6, last_error = $6, incident_id = $7
                        WHERE tenant_id = $1 AND job_key = $2 AND status = 'claimed'
                          AND worker_id = $3 AND claim_token = $4 AND claim_expires_at > now()
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(job_key)
                    .bind(worker_id)
                    .bind(claim_token)
                    .bind(error_class)
                    .bind(error_message)
                    .bind(incident_id)
                    .execute(&mut *tx)
                    .await
                }
            }
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if result.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
        }
        for write in transition.dedupe() {
            let completion = serde_json::to_value(write.completion())
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let result = sqlx::query(
                "INSERT INTO dedupe_cache (job_key, completion, tenant_id) VALUES ($1,$2,$3) ON CONFLICT (job_key) DO NOTHING",
            )
            .bind(write.key()).bind(completion).bind(claim.tenant_id().as_str())
            .execute(&mut *tx).await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if result.rows_affected() == 0 {
                tx.rollback()
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                return Ok(CommitOutcome::IdempotentNoOp);
            }
        }
        for incident in transition.incidents() {
            let error_class = serde_json::to_value(&incident.error_class)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            sqlx::query(
                r#"
                INSERT INTO incidents
                    (incident_id, process_instance_id, fiber_id, service_task_id,
                     bytecode_addr, error_class, message, retry_count, created_at, tenant_id,
                     resolved_at, resolution)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,to_timestamp($9::float / 1000.0),$10,$11,$12)
                ON CONFLICT (incident_id) DO UPDATE SET
                    resolved_at = EXCLUDED.resolved_at,
                    resolution = EXCLUDED.resolution,
                    retry_count = EXCLUDED.retry_count,
                    message = EXCLUDED.message
                "#,
            )
            .bind(incident.incident_id)
            .bind(incident.process_instance_id)
            .bind(incident.fiber_id)
            .bind(&incident.service_task_id)
            .bind(incident.bytecode_addr.get() as i32)
            .bind(error_class)
            .bind(&incident.message)
            .bind(incident.retry_count as i32)
            .bind(incident.created_at as f64)
            .bind(claim.tenant_id().as_str())
            .bind(incident.resolved_at.map(epoch_ms_to_datetime))
            .bind(incident.resolution.as_deref())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for mutation in transition.join_mutations() {
            let (join_id, arrive_count, increment) = match mutation {
                JoinMutation::Arrive(join_id) => (*join_id, 1i16, true),
                JoinMutation::Reset(join_id) => (*join_id, 0i16, false),
            };
            let update = if increment {
                "join_barriers.arrive_count + 1"
            } else {
                "0"
            };
            let query = format!(
                "INSERT INTO join_barriers (instance_id, join_id, arrive_count, tenant_id) \
                 VALUES ($1,$2,$3,$4) ON CONFLICT (instance_id, join_id) DO UPDATE \
                 SET arrive_count = {update}"
            )
            .to_string();
            sqlx::query(&query)
                .bind(claim.instance_id())
                .bind(join_id as i32)
                .bind(arrive_count)
                .bind(claim.tenant_id().as_str())
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        if transition.terminal_cleanup().delete_all_fibers() {
            sqlx::query("DELETE FROM fibers WHERE instance_id = $1")
                .bind(claim.instance_id())
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            sqlx::query(
                "UPDATE workflow_timers SET state = 'cancelled', claim_owner = NULL, claim_token = NULL, claim_until = NULL, updated_at = now() WHERE tenant_id = $1 AND instance_id = $2 AND state = 'armed'",
            )
            .bind(claim.tenant_id().as_str()).bind(claim.instance_id())
            .execute(&mut *tx).await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        if transition.terminal_cleanup().delete_all_joins() {
            sqlx::query("DELETE FROM join_barriers WHERE instance_id = $1")
                .bind(claim.instance_id())
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        if transition.terminal_cleanup().cancel_jobs() {
            sqlx::query("DELETE FROM job_queue WHERE process_instance_id = $1")
                .bind(claim.instance_id())
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for mutation in transition.buffered_messages() {
            let result = match mutation {
                BufferedMessageMutation::Insert(message)
                | BufferedMessageMutation::Deliver(message) => {
                    let status = if matches!(mutation, BufferedMessageMutation::Deliver(_)) {
                        "consumed"
                    } else {
                        "pending"
                    };
                    sqlx::query(
                        r#"
                        INSERT INTO message_buffer (
                            tenant_id, message_name, correlation_key, msg_id, payload,
                            payload_hash, received_at, expires_at, consumed_at,
                            process_instance_id, status
                        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,
                            CASE WHEN $10 = 'consumed' THEN $7 ELSE NULL END,$9,$10)
                        ON CONFLICT (tenant_id, message_name, correlation_key, msg_id) DO NOTHING
                        "#,
                    )
                    .bind(&message.tenant_id)
                    .bind(&message.message_name)
                    .bind(&message.correlation_key)
                    .bind(&message.msg_id)
                    .bind(&message.payload)
                    .bind(message.payload_hash.as_ref().map(|hash| hash.as_slice()))
                    .bind(epoch_ms_to_datetime(message.received_at))
                    .bind(epoch_ms_to_datetime(message.expires_at))
                    .bind(message.process_instance_id)
                    .bind(status)
                    .execute(&mut *tx)
                    .await
                }
                BufferedMessageMutation::Release(message) => {
                    sqlx::query(
                        r#"
                        UPDATE message_buffer SET claim_token = NULL, claim_until = NULL
                        WHERE tenant_id = $1 AND message_name = $2 AND correlation_key = $3
                          AND msg_id = $4 AND claim_token = $5
                        "#,
                    )
                    .bind(&message.message.tenant_id)
                    .bind(&message.message.message_name)
                    .bind(&message.message.correlation_key)
                    .bind(&message.message.msg_id)
                    .bind(&message.claim_token)
                    .execute(&mut *tx)
                    .await
                }
                BufferedMessageMutation::Consume(message) => {
                    sqlx::query(
                        r#"
                        UPDATE message_buffer SET consumed_at = now(), status = 'consumed'
                        WHERE tenant_id = $1 AND message_name = $2 AND correlation_key = $3
                          AND msg_id = $4 AND claim_token = $5 AND claim_until = $6
                          AND consumed_at IS NULL
                        "#,
                    )
                    .bind(&message.message.tenant_id)
                    .bind(&message.message.message_name)
                    .bind(&message.message.correlation_key)
                    .bind(&message.message.msg_id)
                    .bind(&message.claim_token)
                    .bind(epoch_ms_to_datetime(message.claim_until))
                    .execute(&mut *tx)
                    .await
                }
            }
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if matches!(
                mutation,
                BufferedMessageMutation::Insert(_) | BufferedMessageMutation::Deliver(_)
            ) && result.rows_affected() == 0
            {
                tx.rollback()
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                return Ok(CommitOutcome::IdempotentNoOp);
            }
            if matches!(mutation, BufferedMessageMutation::Consume(_))
                && result.rows_affected() != 1
            {
                return Err(CommitError::Conflict);
            }
        }
        for pending in transition.pending_invocations() {
            sqlx::query(
                r#"
                INSERT INTO bpmn_pending_invocation (
                    callout_id, process_instance_id, node_id, target_domain, verb_id,
                    idempotency_key, execution_id, submitted_at, ack_received_at,
                    timeout_at, tenant_id
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(pending.callout_id())
            .bind(pending.process_instance_id())
            .bind(pending.node_id())
            .bind(pending.target_domain())
            .bind(pending.verb_id())
            .bind(pending.idempotency_key())
            .bind(pending.execution_id())
            .bind(epoch_ms_to_datetime(pending.submitted_at()))
            .bind(pending.ack_received_at().map(epoch_ms_to_datetime))
            .bind(pending.timeout_at().map(epoch_ms_to_datetime))
            .bind(claim.tenant_id().as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for outbox in transition.outbox() {
            let now = chrono::Utc::now();
            sqlx::query(
                r#"
                INSERT INTO dsl_bus.outbox (
                    id, target_domain, target_endpoint, payload, idempotency_key,
                    execution_id, callout_id, status, attempt_count, next_attempt_at,
                    last_error, created_at, submitted_at, tenant_id
                ) VALUES ($1,$2,$3,$4,$5,NULL,$6,'pending',0,$7,NULL,$7,NULL,$8)
                ON CONFLICT (idempotency_key, target_endpoint) DO NOTHING
                "#,
            )
            .bind(outbox.id())
            .bind(outbox.target_domain())
            .bind(outbox.target_endpoint())
            .bind(outbox.payload())
            .bind(outbox.idempotency_key())
            .bind(outbox.callout_id())
            .bind(now)
            .bind(claim.tenant_id().as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for pending in transition.pending_invocations() {
            let Some(outbox) = transition
                .outbox()
                .iter()
                .find(|outbox| outbox.callout_id() == pending.callout_id())
            else {
                return Err(CommitError::Integrity(
                    "pending bus invocation has no matching outbox effect".to_string(),
                ));
            };
            let input_ref_hash =
                persist_payload_ref(&mut tx, claim.tenant_id().as_str(), outbox.payload())
                    .await
                    .persistence()?;
            let inserted = sqlx::query(
                r#"
                INSERT INTO workflow_effects (
                    tenant_id, effect_id, instance_id, schema_version, kind,
                    state, operation, input, input_ref_hash, idempotency_key
                ) VALUES ($1,$2,$3,$4,'bus_invocation','pending',$5,''::bytea,$6,$7)
                ON CONFLICT (effect_id) DO NOTHING
                "#,
            )
            .bind(claim.tenant_id().as_str())
            .bind(pending.callout_id())
            .bind(claim.instance_id())
            .bind(i16::try_from(EFFECT_SCHEMA_VERSION).map_err(|_| {
                CommitError::Integrity("effect schema version exceeds SMALLINT".to_string())
            })?)
            .bind(pending.verb_id())
            .bind(&input_ref_hash[..])
            .bind(pending.idempotency_key().to_string())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if inserted.rows_affected() == 0 {
                let matches_identity: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(
                        SELECT 1 FROM workflow_effects
                        WHERE tenant_id = $1 AND effect_id = $2 AND instance_id = $3
                          AND kind = 'bus_invocation' AND input_ref_hash = $4
                    )
                    "#,
                )
                .bind(claim.tenant_id().as_str())
                .bind(pending.callout_id())
                .bind(claim.instance_id())
                .bind(&input_ref_hash[..])
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                if !matches_identity {
                    return Err(CommitError::Integrity(
                        "deterministic bus effect identity collision".to_string(),
                    ));
                }
            }
        }
        for ack in transition.bus_submission_acks() {
            let outbox = sqlx::query(
                r#"
                UPDATE dsl_bus.outbox
                SET status = 'submitted', execution_id = $1, submitted_at = now(),
                    claim_owner = NULL, claim_token = NULL, claim_until = NULL
                WHERE tenant_id = $2 AND id = $3 AND callout_id = $4
                  AND claim_token = $5 AND status = 'dispatching'
                "#,
            )
            .bind(ack.execution_id())
            .bind(claim.tenant_id().as_str())
            .bind(ack.outbox_id())
            .bind(ack.callout_id())
            .bind(ack.dispatch_claim_token())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if outbox.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
            let pending = sqlx::query(
                r#"
                UPDATE bpmn_pending_invocation
                SET execution_id = $1, ack_received_at = now()
                WHERE tenant_id = $2 AND callout_id = $3 AND execution_id IS NULL
                "#,
            )
            .bind(ack.execution_id())
            .bind(claim.tenant_id().as_str())
            .bind(ack.callout_id())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if pending.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
            let effect = sqlx::query(
                r#"
                UPDATE workflow_effects
                SET state = 'accepted', execution_id = $1, updated_at = now()
                WHERE tenant_id = $2 AND effect_id = $3
                  AND kind = 'bus_invocation' AND state = 'pending'
                  AND terminal = FALSE
                "#,
            )
            .bind(ack.execution_id())
            .bind(claim.tenant_id().as_str())
            .bind(ack.callout_id())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if effect.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
        }
        for execution_id in transition.pending_take() {
            let result = sqlx::query(
                "DELETE FROM bpmn_pending_invocation WHERE tenant_id = $1 AND execution_id = $2",
            )
            .bind(claim.tenant_id().as_str())
            .bind(execution_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if result.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
        }
        for effect in transition.effects() {
            match effect {
                DurableEffect::ScheduleTimer {
                    timer_id,
                    fiber_id,
                    due_at,
                    kind,
                    repeat_spec,
                } => {
                    let kind_json = serde_json::to_value(kind)
                        .map_err(|error| CommitError::Integrity(error.to_string()))?;
                    let repeat_json = repeat_spec
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|error| CommitError::Integrity(error.to_string()))?;
                    let inserted = sqlx::query(
                        r#"
                        INSERT INTO workflow_timers (
                            tenant_id, timer_id, instance_id, fiber_id, due_at,
                            kind, repeat_spec, state
                        ) VALUES ($1,$2,$3,$4,$5,$6,$7,'armed')
                        ON CONFLICT (timer_id) DO NOTHING
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(timer_id.as_uuid())
                    .bind(claim.instance_id())
                    .bind(fiber_id)
                    .bind(epoch_ms_to_datetime(i64::try_from(*due_at).map_err(
                        |_| CommitError::Integrity("timer due_at exceeds i64".to_string()),
                    )?))
                    .bind(&kind_json)
                    .bind(&repeat_json)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                    if inserted.rows_affected() == 0 {
                        let matches_identity: bool = sqlx::query_scalar(
                            r#"
                            SELECT EXISTS(
                                SELECT 1 FROM workflow_timers
                                WHERE tenant_id = $1 AND timer_id = $2 AND instance_id = $3
                                  AND fiber_id = $4 AND kind = $5
                            )
                            "#,
                        )
                        .bind(claim.tenant_id().as_str())
                        .bind(timer_id.as_uuid())
                        .bind(claim.instance_id())
                        .bind(fiber_id)
                        .bind(&kind_json)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                        if !matches_identity {
                            return Err(CommitError::Integrity(
                                "deterministic timer identity collision".to_string(),
                            ));
                        }
                    }
                }
                DurableEffect::Invoke {
                    effect_id,
                    operation,
                    idempotency_key,
                    ..
                } => {
                    let encoded = serde_json::to_vec(effect)
                        .map_err(|error| CommitError::Integrity(error.to_string()))?;
                    let input_ref_hash =
                        persist_payload_ref(&mut tx, claim.tenant_id().as_str(), &encoded)
                            .await
                            .persistence()?;
                    let inserted = sqlx::query(
                        r#"
                        INSERT INTO workflow_effects (
                            tenant_id, effect_id, instance_id, schema_version,
                            kind, state, operation, input, input_ref_hash, idempotency_key
                        ) VALUES ($1,$2,$3,$4,'ffi','pending',$5,''::bytea,$6,$7)
                        ON CONFLICT (effect_id) DO NOTHING
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(effect_id.as_uuid())
                    .bind(claim.instance_id())
                    .bind(i16::try_from(EFFECT_SCHEMA_VERSION).map_err(|_| {
                        CommitError::Integrity("effect schema version exceeds SMALLINT".to_string())
                    })?)
                    .bind(operation)
                    .bind(&input_ref_hash[..])
                    .bind(idempotency_key)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                    if inserted.rows_affected() == 0 {
                        let matches_identity: bool = sqlx::query_scalar(
                            r#"
                            SELECT EXISTS(
                                SELECT 1 FROM workflow_effects
                                WHERE tenant_id = $1 AND effect_id = $2
                                  AND instance_id = $3 AND input_ref_hash = $4
                            )
                            "#,
                        )
                        .bind(claim.tenant_id().as_str())
                        .bind(effect_id.as_uuid())
                        .bind(claim.instance_id())
                        .bind(&input_ref_hash[..])
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                        if !matches_identity {
                            return Err(CommitError::Integrity(
                                "deterministic effect identity collision".to_string(),
                            ));
                        }
                    }
                }
            }
        }
        for mutation in transition.effect_mutations() {
            let state = match mutation.terminal_state() {
                EffectTerminalState::Completed => "completed",
                EffectTerminalState::Failed => "failed",
            };
            let updated = sqlx::query(
                r#"
                UPDATE workflow_effects
                SET state = $1, terminal = TRUE, updated_at = now(),
                    claim_owner = NULL, claim_token = NULL, claim_until = NULL
                WHERE tenant_id = $2 AND effect_id = $3 AND instance_id = $4
                  AND state = 'accepted' AND terminal = FALSE
                "#,
            )
            .bind(state)
            .bind(claim.tenant_id().as_str())
            .bind(mutation.effect_id().as_uuid())
            .bind(claim.instance_id())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if updated.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
            let consumed =
                sqlx::query("DELETE FROM workflow_inbox WHERE tenant_id = $1 AND effect_id = $2")
                    .bind(claim.tenant_id().as_str())
                    .bind(mutation.effect_id().as_uuid())
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if consumed.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }
        }
        for mutation in transition.timer_mutations() {
            let result = match mutation {
                TimerMutation::Consume {
                    timer_id,
                    claim_token,
                } => {
                    sqlx::query(
                        r#"
                        UPDATE workflow_timers
                        SET state = 'consumed', consumed_at = now(), updated_at = now(),
                            claim_owner = NULL, claim_token = NULL, claim_until = NULL
                        WHERE tenant_id = $1 AND timer_id = $2 AND instance_id = $3
                          AND state = 'armed' AND claim_token = $4
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(timer_id.as_uuid())
                    .bind(claim.instance_id())
                    .bind(claim_token)
                    .execute(&mut *tx)
                    .await
                }
                TimerMutation::Rearm {
                    timer_id,
                    claim_token,
                    due_at,
                    repeat_spec,
                } => {
                    let repeat_json = serde_json::to_value(repeat_spec)
                        .map_err(|error| CommitError::Integrity(error.to_string()))?;
                    sqlx::query(
                        r#"
                        UPDATE workflow_timers
                        SET due_at = $5, repeat_spec = $6, state = 'armed', updated_at = now(),
                            claim_owner = NULL, claim_token = NULL, claim_until = NULL
                        WHERE tenant_id = $1 AND timer_id = $2 AND instance_id = $3
                          AND state = 'armed' AND claim_token = $4
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(timer_id.as_uuid())
                    .bind(claim.instance_id())
                    .bind(claim_token)
                    .bind(epoch_ms_to_datetime(i64::try_from(*due_at).map_err(
                        |_| CommitError::Integrity("timer due_at exceeds i64".to_string()),
                    )?))
                    .bind(&repeat_json)
                    .execute(&mut *tx)
                    .await
                }
                TimerMutation::CancelRace {
                    fiber_id,
                    race_id,
                    except,
                } => {
                    sqlx::query(
                        r#"
                        UPDATE workflow_timers
                        SET state = 'cancelled', updated_at = now(),
                            claim_owner = NULL, claim_token = NULL, claim_until = NULL
                        WHERE tenant_id = $1 AND instance_id = $2 AND fiber_id = $3
                          AND state = 'armed' AND timer_id <> $4
                          AND kind #>> '{Race,race_id}' = $5
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(claim.instance_id())
                    .bind(fiber_id)
                    .bind(except.as_uuid())
                    .bind(race_id.to_string())
                    .execute(&mut *tx)
                    .await
                }
                TimerMutation::V2CancelRace {
                    fiber_id,
                    record_id,
                    except,
                } => {
                    sqlx::query(
                        r#"
                        UPDATE workflow_timers
                        SET state = 'cancelled', updated_at = now(),
                            claim_owner = NULL, claim_token = NULL, claim_until = NULL
                        WHERE tenant_id = $1 AND instance_id = $2 AND fiber_id = $3
                          AND state = 'armed' AND timer_id <> $4
                          AND kind #>> '{V2Race,record_id}' = $5
                        "#,
                    )
                    .bind(claim.tenant_id().as_str())
                    .bind(claim.instance_id())
                    .bind(fiber_id)
                    .bind(except.as_uuid())
                    .bind(record_id.as_uuid().to_string())
                    .execute(&mut *tx)
                    .await
                }
            }
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if !matches!(
                mutation,
                TimerMutation::CancelRace { .. } | TimerMutation::V2CancelRace { .. }
            ) && result.rows_affected() != 1
            {
                tx.rollback()
                    .await
                    .map_err(|error| CommitError::Unavailable(error.to_string()))?;
                return Ok(CommitOutcome::IdempotentNoOp);
            }
        }
        for child in transition.child_starts() {
            let instance = child.instance();
            if instance.tenant_id != claim.tenant_id().as_str() {
                return Err(CommitError::Integrity(
                    "parent and child tenant identities differ".to_string(),
                ));
            }
            let session_stack = serde_json::to_value(&instance.session_stack)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let flags = serde_json::to_value(&instance.flags)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let counters = serde_json::to_value(&instance.counters)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let join_expected = serde_json::to_value(&instance.join_expected)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let state = serde_json::to_value(&instance.state)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let integrity_hash = compute_instance_integrity_hash(instance);
            let inserted = sqlx::query(
                r#"
                INSERT INTO workflow_instances (
                    instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                    domain_payload_hash, session_stack, flags, counters, join_expected, state,
                    correlation_id, entry_id, runbook_id, created_at, integrity_hash,
                    plan_hash, current_node_id, placeholder_values, revision, fence
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,0,0)
                ON CONFLICT (instance_id) DO NOTHING
                "#,
            )
            .bind(instance.instance_id)
            .bind(&instance.tenant_id)
            .bind(&instance.process_key)
            .bind(&instance.bytecode_version[..])
            .bind(instance.domain_payload.as_ref())
            .bind(&instance.domain_payload_hash[..])
            .bind(session_stack)
            .bind(flags)
            .bind(counters)
            .bind(join_expected)
            .bind(state)
            .bind(&instance.correlation_id)
            .bind(instance.entry_id)
            .bind(instance.runbook_id)
            .bind(epoch_ms_to_datetime(instance.created_at))
            .bind(&integrity_hash[..])
            .bind(instance.plan_hash.as_ref().map(|hash| hash.as_slice()))
            .bind(instance.current_node_id.as_deref())
            .bind(instance.placeholder_values.as_ref())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if inserted.rows_affected() != 1 {
                return Err(CommitError::Conflict);
            }

            let fiber = child.root_fiber();
            let stack = serde_json::to_value(&fiber.stack)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let regs = serde_json::to_value(Vec::<bpmn_lite_types::Value>::new())
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let wait = serde_json::to_value(&fiber.wait)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let fiber_insert = sqlx::query(
                r#"
                INSERT INTO fibers
                    (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                "#,
            )
            .bind(instance.instance_id)
            .bind(fiber.fiber_id)
            .bind(fiber.pc.get() as i32)
            .bind(stack)
            .bind(regs)
            .bind(wait)
            .bind(fiber.loop_epoch as i32)
            .bind(claim.tenant_id().as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if fiber_insert.rows_affected() != 1 {
                return Err(CommitError::Integrity(
                    "child root fiber was not inserted".to_string(),
                ));
            }

            let event = serde_json::to_value(child.start_event())
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let event_insert = sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                    VALUES ($1, 1, $3)
                    RETURNING next_seq, tenant_id
                )
                INSERT INTO event_log (instance_id, seq, event, tenant_id)
                SELECT $1, seq.next_seq, $2, seq.tenant_id FROM seq
                "#,
            )
            .bind(instance.instance_id)
            .bind(event)
            .bind(claim.tenant_id().as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if event_insert.rows_affected() != 1 {
                return Err(CommitError::Integrity(
                    "child start event was not inserted".to_string(),
                ));
            }

            let child_snapshot = SnapshotEnvelope::new(
                CURRENT_ARTIFACT_ABI,
                instance.bytecode_version,
                0,
                PersistedSnapshotState::new(
                    instance.clone(),
                    [fiber.clone()],
                    std::collections::BTreeMap::new(),
                    [],
                    ConcurrencyTable::new(),
                    [],
                ),
            );
            let child_state_hash = child_snapshot
                .state_hash()
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let child_command = CommandEnvelope::new(
                EffectId::for_transition(instance.instance_id, 0, u32::MAX).as_uuid(),
                instance.created_at,
                JournalCommand::Administrative {
                    kind: "child_start".to_string(),
                },
            );
            let child_journal = JournalRecord::new(
                child_command,
                -1,
                0,
                instance.bytecode_version,
                [0u8; 32],
                child_state_hash,
                std::slice::from_ref(child.start_event()),
                &[],
            );
            let child_snapshot_bytes = child_snapshot
                .canonical_bytes()
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let child_frame_hash: [u8; 32] = *blake3::hash(&child_snapshot_bytes).as_bytes();
            let child_snapshot_update =
                sqlx::query(
                    r#"
                UPDATE workflow_instances
                SET snapshot_schema_version = $1, artifact_abi = $2, snapshot_envelope = $3, frame_hash = $4
                WHERE tenant_id = $5 AND instance_id = $6 AND revision = 0
                "#,
                )
                .bind(i16::try_from(child_snapshot.schema_version()).map_err(|_| {
                    CommitError::Integrity("snapshot schema version exceeds SMALLINT".to_string())
                })?)
                .bind(i32::try_from(child_snapshot.artifact_abi()).map_err(|_| {
                    CommitError::Integrity("artifact ABI exceeds INTEGER".to_string())
                })?)
                .bind(child_snapshot_bytes)
                .bind(&child_frame_hash[..])
                .bind(claim.tenant_id().as_str())
                .bind(instance.instance_id)
                .execute(&mut *tx)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if child_snapshot_update.rows_affected() != 1 {
                return Err(CommitError::Integrity(
                    "child snapshot envelope was not stored".to_string(),
                ));
            }
            let child_journal_insert = sqlx::query(
                r#"
                INSERT INTO workflow_journal (
                    tenant_id, instance_id, schema_version, command_schema_version,
                    command_id, command_type, logical_time, prior_revision, new_revision,
                    artifact_hash, prior_state_hash, state_hash, record_envelope
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,-1,0,$8,$9,$10,$11)
                "#,
            )
            .bind(claim.tenant_id().as_str())
            .bind(instance.instance_id)
            .bind(i16::try_from(child_journal.schema_version()).map_err(|_| {
                CommitError::Integrity("journal schema version exceeds SMALLINT".to_string())
            })?)
            .bind(
                i16::try_from(child_journal.command().schema_version()).map_err(|_| {
                    CommitError::Integrity("command schema version exceeds SMALLINT".to_string())
                })?,
            )
            .bind(child_journal.command().command_id())
            .bind(child_journal.command().command_type())
            .bind(child_journal.command().logical_time())
            .bind(&child_journal.artifact_hash()[..])
            .bind(&child_journal.prior_state_hash()[..])
            .bind(&child_journal.state_hash()[..])
            .bind(
                child_journal
                    .canonical_bytes()
                    .map_err(|error| CommitError::Integrity(error.to_string()))?,
            )
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if child_journal_insert.rows_affected() != 1 {
                return Err(CommitError::Integrity(
                    "child journal genesis was not inserted".to_string(),
                ));
            }
        }
        for event in transition.events() {
            let event = serde_json::to_value(event)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                    VALUES ($1, 1, $3)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq, tenant_id
                )
                INSERT INTO event_log (instance_id, seq, event, tenant_id)
                SELECT $1, seq.next_seq, $2, seq.tenant_id FROM seq
                "#,
            )
            .bind(claim.instance_id())
            .bind(event)
            .bind(claim.tenant_id().as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        sqlx::query(
            r#"
            INSERT INTO payload_history (instance_id, payload_hash, domain_payload, tenant_id)
            VALUES ($1,$2,$3,$4)
            ON CONFLICT (instance_id, payload_hash) DO NOTHING
            "#,
        )
        .bind(claim.instance_id())
        .bind(&snapshot.domain_payload_hash[..])
        .bind(snapshot.domain_payload.as_ref())
        .bind(claim.tenant_id().as_str())
        .execute(&mut *tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;

        let new_revision = if inserted_start {
            0
        } else {
            claim.expected_revision().checked_add(1).ok_or_else(|| {
                CommitError::Integrity("revision overflow while journaling".to_string())
            })?
        };
        let fiber_rows = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE instance_id = $1 ORDER BY fiber_id",
        )
        .bind(claim.instance_id())
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        let mut persisted_fibers = Vec::with_capacity(fiber_rows.len());
        for row in fiber_rows {
            use sqlx::Row;
            let pc: i32 = row.get("pc");
            let loop_epoch: i32 = row.get("loop_epoch");
            persisted_fibers.push(Fiber {
                fiber_id: row.get("fiber_id"),
                pc: Addr::new(u32::try_from(pc).map_err(|_| {
                    CommitError::Integrity("negative persisted program counter".to_string())
                })?),
                stack: serde_json::from_value(row.get("stack"))
                    .map_err(|error| CommitError::Integrity(error.to_string()))?,
                wait: serde_json::from_value(row.get("wait_state"))
                    .map_err(|error| CommitError::Integrity(error.to_string()))?,
                loop_epoch: u32::try_from(loop_epoch).map_err(|_| {
                    CommitError::Integrity("negative persisted loop epoch".to_string())
                })?,
                control_stack: Vec::new(),
            });
        }
        let join_rows = sqlx::query(
            "SELECT join_id, arrive_count FROM join_barriers WHERE instance_id = $1 ORDER BY join_id",
        )
        .bind(claim.instance_id())
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        let mut persisted_joins = std::collections::BTreeMap::new();
        for row in join_rows {
            use sqlx::Row;
            let join_id: i32 = row.get("join_id");
            let count: i16 = row.get("arrive_count");
            persisted_joins.insert(
                u32::try_from(join_id).map_err(|_| {
                    CommitError::Integrity("negative persisted join id".to_string())
                })?,
                u16::try_from(count).map_err(|_| {
                    CommitError::Integrity("negative persisted join count".to_string())
                })?,
            );
        }
        let incident_rows = sqlx::query(
            r#"
            SELECT incident_id, process_instance_id, fiber_id, service_task_id,
                   bytecode_addr, error_class, message, retry_count,
                   (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms,
                   (EXTRACT(EPOCH FROM resolved_at) * 1000)::BIGINT AS resolved_at_ms,
                   resolution
            FROM incidents
            WHERE process_instance_id = $1
            ORDER BY incident_id
            "#,
        )
        .bind(claim.instance_id())
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        let mut persisted_incidents = Vec::with_capacity(incident_rows.len());
        for row in incident_rows {
            use sqlx::Row;
            let bytecode_addr: i32 = row.get("bytecode_addr");
            let retry_count: i32 = row.get("retry_count");
            persisted_incidents.push(Incident {
                incident_id: row.get("incident_id"),
                process_instance_id: row.get("process_instance_id"),
                fiber_id: row.get("fiber_id"),
                service_task_id: row.get("service_task_id"),
                bytecode_addr: Addr::new(u32::try_from(bytecode_addr).map_err(|_| {
                    CommitError::Integrity("negative persisted bytecode address".to_string())
                })?),
                error_class: serde_json::from_value(row.get("error_class"))
                    .map_err(|error| CommitError::Integrity(error.to_string()))?,
                message: row.get("message"),
                retry_count: u32::try_from(retry_count).map_err(|_| {
                    CommitError::Integrity("negative persisted retry count".to_string())
                })?,
                created_at: row.get("created_at_ms"),
                resolved_at: row.get("resolved_at_ms"),
                resolution: row.get("resolution"),
            });
        }
        let mut persisted_instance = snapshot.clone();
        if let Some(state) = transition.state_override() {
            persisted_instance.state = state.clone();
        }
        // D1 concurrency table: baseline is whatever the prior commit
        // persisted (empty for the first commit after start); apply this
        // transition's mutations on top. `concurrency_mutations()` is
        // always empty until V4's words exist to produce them — this path
        // is exercised now so V4 lands on already-correct plumbing.
        let prior_envelope_bytes: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT snapshot_envelope FROM workflow_instances WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(claim.tenant_id().as_str())
        .bind(claim.instance_id())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?
        .flatten();
        // A decode failure here means the persisted row exists but is
        // corrupt — that must reject the commit, not be treated as if no
        // prior snapshot existed (which would silently drop live
        // concurrency records and desync the Ring 2 hash chain).
        let prior_envelope: Option<SnapshotEnvelope> = prior_envelope_bytes
            .as_deref()
            .map(|bytes| {
                SnapshotEnvelope::decode(bytes).map_err(|error| {
                    CommitError::Integrity(format!(
                        "prior snapshot envelope failed to decode on commit: {error}"
                    ))
                })
            })
            .transpose()?;
        let mut concurrency_table = prior_envelope
            .as_ref()
            .map(|envelope| envelope.state().concurrency_table().clone())
            .unwrap_or_default();
        for mutation in transition.concurrency_mutations() {
            match mutation {
                ConcurrencyMutation::Insert(record) => concurrency_table.insert((**record).clone()),
                ConcurrencyMutation::Retire(id) => {
                    if let Some(record) = concurrency_table.get_mut(*id) {
                        record.state = RecordState::Retired;
                    }
                }
                ConcurrencyMutation::Remove(id) => {
                    concurrency_table.remove(*id);
                }
            }
        }
        // V&S §15 (v0.7) ruling F: read BEFORE `concurrency_table` moves
        // into `PersistedSnapshotState` below — this transition's own
        // mutations are already applied, so a just-cancelled Guard's
        // `opened_at` is still resolvable here even though its record is
        // now `Retired`.
        self.apply_guard_failure_budget(
            &mut tx,
            claim.tenant_id().as_str(),
            claim.instance_id(),
            transition,
            &concurrency_table,
        )
        .await
        // Preserve the error kind: a fail-closed integrity violation (e.g. a
        // guard cancellation whose pinned artifact is absent) must NOT be
        // reported as `Unavailable`, which reads as transient/retryable.
        .map_err(|error| match error {
            StoreError::Integrity(message) => CommitError::Integrity(message),
            other => CommitError::Unavailable(other.to_string()),
        })?;
        let snapshot_envelope = SnapshotEnvelope::new(
            CURRENT_ARTIFACT_ABI,
            persisted_instance.bytecode_version,
            new_revision,
            PersistedSnapshotState::new(
                persisted_instance,
                persisted_fibers,
                persisted_joins,
                persisted_incidents,
                concurrency_table,
                [],
            ),
        );
        let snapshot_bytes = snapshot_envelope
            .canonical_bytes()
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        // D3 Ring 1: physical-integrity hash over the exact bytes stored.
        // Verified on load BEFORE decode (see claim_work_for_transition) —
        // a corrupted frame never reaches the deserializer.
        let frame_hash: [u8; 32] = *blake3::hash(&snapshot_bytes).as_bytes();
        // R1 mitigation (c): sampled runtime round-trip assertion on commit.
        // Golden-bytes fixtures and property tests (canonical.rs) prove the
        // fixed point in CI on a handful of frames; this samples *live*
        // commits so a canonical-form regression (dependency upgrade
        // changing field order, a new nondeterministic field type slipping
        // past the CI lint) is caught in production, not just at merge time.
        if should_sample_canonical_round_trip(new_revision) {
            let decoded = SnapshotEnvelope::decode(&snapshot_bytes).map_err(|error| {
                CommitError::Integrity(format!(
                    "R1 sampled round-trip: decode of just-encoded envelope failed: {error}"
                ))
            })?;
            let recanonicalized = decoded.canonical_bytes().map_err(|error| {
                CommitError::Integrity(format!(
                    "R1 sampled round-trip: re-canonicalization failed: {error}"
                ))
            })?;
            if recanonicalized != snapshot_bytes {
                return Err(CommitError::Integrity(
                    "R1 sampled round-trip: canonicalize(decode(bytes)) != bytes — canonical-form drift detected on commit".to_string(),
                ));
            }
        }
        let state_hash = snapshot_envelope
            .state_hash()
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let prior_state_hash = prior_envelope
            .as_ref()
            .map(|envelope| {
                envelope
                    .state_hash()
                    .map_err(|error| CommitError::Integrity(error.to_string()))
            })
            .transpose()?
            .unwrap_or([0u8; 32]);
        let command = transition.command_envelope().cloned().unwrap_or_else(|| {
            transition.start_dedupe().map_or_else(
                || {
                    CommandEnvelope::new(
                        EffectId::for_transition(claim.instance_id(), new_revision, u32::MAX)
                            .as_uuid(),
                        snapshot.created_at,
                        JournalCommand::Administrative {
                            kind: "fixture_or_admin_commit".to_string(),
                        },
                    )
                },
                |start| {
                    CommandEnvelope::new(
                        start.command().idempotency_key(),
                        start.command().logical_time(),
                        JournalCommand::Start(start.command().clone()),
                    )
                },
            )
        });
        let prior_revision = if inserted_start {
            -1
        } else {
            i64::try_from(claim.expected_revision()).map_err(|_| {
                CommitError::Integrity("revision exceeds signed journal range".to_string())
            })?
        };
        let journal = JournalRecord::new(
            command,
            prior_revision,
            new_revision,
            snapshot.bytecode_version,
            prior_state_hash,
            state_hash,
            transition.events(),
            transition.effects(),
        );
        let journal_bytes = journal
            .canonical_bytes()
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let snapshot_update = sqlx::query(
            r#"
            UPDATE workflow_instances
            SET snapshot_schema_version = $1, artifact_abi = $2, snapshot_envelope = $3, frame_hash = $4
            WHERE tenant_id = $5 AND instance_id = $6 AND revision = $7
            "#,
        )
        .bind(
            i16::try_from(snapshot_envelope.schema_version()).map_err(|_| {
                CommitError::Integrity("snapshot schema version exceeds SMALLINT".to_string())
            })?,
        )
        .bind(
            i32::try_from(snapshot_envelope.artifact_abi())
                .map_err(|_| CommitError::Integrity("artifact ABI exceeds INTEGER".to_string()))?,
        )
        .bind(snapshot_bytes)
        .bind(&frame_hash[..])
        .bind(claim.tenant_id().as_str())
        .bind(claim.instance_id())
        .bind(i64::try_from(new_revision).map_err(|_| {
            CommitError::Integrity("revision exceeds PostgreSQL BIGINT".to_string())
        })?)
        .execute(&mut *tx)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        if snapshot_update.rows_affected() != 1 {
            return Err(CommitError::Conflict);
        }
        let journal_insert =
            sqlx::query(
                r#"
            INSERT INTO workflow_journal (
                tenant_id, instance_id, schema_version, command_schema_version,
                command_id, command_type, logical_time, prior_revision, new_revision,
                artifact_hash, prior_state_hash, state_hash, record_envelope
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            "#,
            )
            .bind(claim.tenant_id().as_str())
            .bind(claim.instance_id())
            .bind(i16::try_from(journal.schema_version()).map_err(|_| {
                CommitError::Integrity("journal schema version exceeds SMALLINT".to_string())
            })?)
            .bind(
                i16::try_from(journal.command().schema_version()).map_err(|_| {
                    CommitError::Integrity("command schema version exceeds SMALLINT".to_string())
                })?,
            )
            .bind(journal.command().command_id())
            .bind(journal.command().command_type())
            .bind(journal.command().logical_time())
            .bind(journal.prior_revision())
            .bind(i64::try_from(journal.new_revision()).map_err(|_| {
                CommitError::Integrity("journal revision exceeds BIGINT".to_string())
            })?)
            .bind(&journal.artifact_hash()[..])
            .bind(&journal.prior_state_hash()[..])
            .bind(&journal.state_hash()[..])
            .bind(journal_bytes)
            .execute(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        if journal_insert.rows_affected() != 1 {
            return Err(CommitError::Integrity(
                "journal record was not inserted".to_string(),
            ));
        }

        if !transition.events().is_empty() {
            notify_event_tx(&mut tx, claim.instance_id())
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }

        if !inserted_start {
            let state_value: serde_json::Value = sqlx::query_scalar(
                "SELECT state FROM workflow_instances WHERE tenant_id = $1 AND instance_id = $2",
            )
            .bind(claim.tenant_id().as_str())
            .bind(claim.instance_id())
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            let process_state: ProcessState = serde_json::from_value(state_value)
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let has_running_fiber: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM fibers WHERE instance_id = $1 AND wait_state = '\"Running\"'::jsonb)",
            )
            .bind(claim.instance_id()).fetch_one(&mut *tx).await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if !matches!(process_state, ProcessState::Running) || !has_running_fiber {
                sqlx::query(
                    "UPDATE workflow_instances SET lease_owner = NULL, lease_until = now() - interval '1 second' WHERE tenant_id = $1 AND instance_id = $2 AND fence = $3",
                )
                .bind(claim.tenant_id().as_str()).bind(claim.instance_id())
                .bind(i64::try_from(claim.fence()).map_err(|_| {
                    CommitError::Integrity("fence exceeds PostgreSQL BIGINT".to_string())
                })?)
                .execute(&mut *tx).await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            }
        }

        tx.commit()
            .await
            .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        Ok(CommitOutcome::Committed { new_revision })
    }

    async fn lookup_start_instance(
        &self,
        tenant_id: &TenantId,
        idempotency_key: Uuid,
    ) -> StoreResult<Option<Uuid>> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;
        let instance_id = sqlx::query_scalar(
            "SELECT instance_id FROM bpmn_spawn_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
        )
        .bind(tenant_id.as_str())
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await.persistence()?;
        tx.commit().await.persistence()?;
        Ok(instance_id)
    }

    async fn claim_due_timers(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedTimer>> {
        let now_ms = i64::try_from(now_ms)
            .map_err(|_| StoreError::Integrity("timer now exceeds i64".to_string()))?;
        let lease_ms = i64::try_from(lease_ms)
            .map_err(|_| StoreError::Integrity("timer lease exceeds i64".to_string()))?;
        let limit = i64::try_from(limit)
            .map_err(|_| StoreError::Integrity("timer batch limit exceeds i64".to_string()))?;
        let tenant = tenant_id.clone();
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;
        let rows = sqlx::query(
            r#"
            SELECT timer_id, instance_id, fiber_id, due_at, kind, repeat_spec
            FROM workflow_timers
            WHERE tenant_id = $1 AND state = 'armed' AND due_at <= $2
              AND (claim_until IS NULL OR claim_until <= $2)
            ORDER BY due_at, timer_id
            FOR UPDATE SKIP LOCKED
            LIMIT $3
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(epoch_ms_to_datetime(now_ms))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .persistence()?;

        use sqlx::Row;
        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            let timer_id: Uuid = row.get("timer_id");
            let claim_token = Uuid::now_v7();
            let result = sqlx::query(
                r#"
                UPDATE workflow_timers
                SET claim_owner = $3, claim_token = $4,
                    claim_until = $5, updated_at = now()
                WHERE tenant_id = $1 AND timer_id = $2 AND state = 'armed'
                "#,
            )
            .bind(tenant_id.as_str())
            .bind(timer_id)
            .bind(owner)
            .bind(claim_token)
            .bind(epoch_ms_to_datetime(now_ms.saturating_add(lease_ms)))
            .execute(&mut *tx)
            .await
            .persistence()?;
            if result.rows_affected() != 1 {
                return Err(StoreError::Integrity(
                    "due timer claim lost while row lock was held".to_string(),
                ));
            }
            let due_at: chrono::DateTime<chrono::Utc> = row.get("due_at");
            let due_at = u64::try_from(datetime_to_epoch_ms(due_at)).map_err(|_| {
                StoreError::Integrity("timer due_at is before the Unix epoch".to_string())
            })?;
            let kind: TimerKind = serde_json::from_value(row.get("kind")).persistence()?;
            let repeat_spec: Option<serde_json::Value> = row.get("repeat_spec");
            let repeat_spec = repeat_spec
                .map(serde_json::from_value)
                .transpose()
                .persistence()?;
            claimed.push(ClaimedTimer::new(
                bpmn_lite_types::ClaimedTimerIdentity::new(
                    tenant.clone(),
                    EffectId::from_uuid(timer_id),
                    row.get("instance_id"),
                    row.get("fiber_id"),
                ),
                due_at,
                kind,
                repeat_spec,
                claim_token,
            ));
        }
        tx.commit().await.persistence()?;
        Ok(claimed)
    }

    async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, timer.tenant_id().as_str())
            .await
            .persistence()?;
        sqlx::query(
            r#"
            UPDATE workflow_timers
            SET claim_owner = NULL, claim_token = NULL, claim_until = NULL, updated_at = now()
            WHERE tenant_id = $1 AND timer_id = $2 AND state = 'armed' AND claim_token = $3
            "#,
        )
        .bind(timer.tenant_id().as_str())
        .bind(timer.timer_id().as_uuid())
        .bind(timer.claim_token())
        .execute(&mut *tx)
        .await
        .persistence()?;
        tx.commit().await.persistence()?;
        Ok(())
    }

    async fn claim_pending_effects(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedEffect>> {
        let now_ms = i64::try_from(now_ms)
            .map_err(|_| StoreError::Integrity("effect clock exceeds i64".to_string()))?;
        let lease_ms = i64::try_from(lease_ms)
            .map_err(|_| StoreError::Integrity("effect lease exceeds i64".to_string()))?;
        let limit = i64::try_from(limit)
            .map_err(|_| StoreError::Integrity("effect batch limit exceeds i64".to_string()))?;
        let tenant = tenant_id.clone();
        let claim_token = Uuid::now_v7();
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;
        let rows = sqlx::query(
            r#"
            WITH candidates AS (
                SELECT effect_id
                FROM workflow_effects
                WHERE tenant_id = $1 AND kind = 'ffi' AND terminal = FALSE
                  AND state IN ('pending', 'dispatching')
                  AND next_due_at <= $2
                  AND (claim_until IS NULL OR claim_until <= $2)
                ORDER BY next_due_at, effect_id
                FOR UPDATE SKIP LOCKED
                LIMIT $3
            )
            , updated AS (
                UPDATE workflow_effects AS effect
                SET state = 'dispatching', claim_owner = $4, claim_token = $5,
                    claim_until = $6, updated_at = now()
                FROM candidates
                WHERE effect.effect_id = candidates.effect_id AND effect.tenant_id = $1
                RETURNING effect.effect_id, effect.instance_id, effect.input,
                          effect.input_ref_hash,
                          effect.attempt, effect.policy_version
            )
            SELECT updated.effect_id, updated.instance_id,
                   COALESCE(payload.payload, updated.input) AS input,
                   updated.attempt, updated.policy_version
            FROM updated
            LEFT JOIN workflow_payloads AS payload
              ON payload.tenant_id = $1
             AND payload.payload_hash = updated.input_ref_hash
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(epoch_ms_to_datetime(now_ms))
        .bind(limit)
        .bind(owner)
        .bind(claim_token)
        .bind(epoch_ms_to_datetime(now_ms.saturating_add(lease_ms)))
        .fetch_all(&mut *tx)
        .await
        .persistence()?;
        tx.commit().await.persistence()?;

        use sqlx::Row;
        rows.into_iter()
            .map(|row| {
                let encoded: Vec<u8> = row.get("input");
                let effect: DurableEffect = serde_json::from_slice(&encoded).persistence()?;
                Ok(ClaimedEffect::new(
                    tenant.clone(),
                    row.get("instance_id"),
                    effect,
                    claim_token,
                    u32::try_from(row.get::<i32, _>("attempt")).persistence()?,
                    u32::try_from(row.get::<i32, _>("policy_version")).persistence()?,
                ))
            })
            .collect()
    }

    async fn record_effect_response(
        &self,
        effect: &ClaimedEffect,
        response: &EffectResponse,
    ) -> StoreResult<bool> {
        let encoded = serde_json::to_vec(response).persistence()?;
        let effect_id = effect.effect().effect_id().as_uuid();
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, effect.tenant_id().as_str())
            .await
            .persistence()?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO workflow_inbox (tenant_id, effect_id, schema_version, response)
            SELECT tenant_id, effect_id, $4, $5
            FROM workflow_effects
            WHERE tenant_id = $1 AND effect_id = $2 AND claim_token = $3
              AND state = 'dispatching' AND terminal = FALSE
            ON CONFLICT (effect_id) DO NOTHING
            "#,
        )
        .bind(effect.tenant_id().as_str())
        .bind(effect_id)
        .bind(effect.claim_token())
        .bind(
            i16::try_from(EFFECT_SCHEMA_VERSION)
                .map_err(|_| StoreError::Integrity("effect version exceeds i16".to_string()))?,
        )
        .bind(&encoded)
        .execute(&mut *tx)
        .await
        .persistence()?;
        if inserted.rows_affected() == 0 {
            let existing: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT response FROM workflow_inbox WHERE tenant_id = $1 AND effect_id = $2",
            )
            .bind(effect.tenant_id().as_str())
            .bind(effect_id)
            .fetch_optional(&mut *tx)
            .await
            .persistence()?;
            if let Some(existing) = existing {
                if existing != encoded {
                    return Err(StoreError::Integrity(
                        "effect response identity collision".to_string(),
                    ));
                }
                tx.commit().await.persistence()?;
                return Ok(false);
            }
            return Err(StoreError::Integrity(
                "effect dispatch lease is stale".to_string(),
            ));
        }
        let updated = sqlx::query(
            r#"
            UPDATE workflow_effects
            SET state = 'accepted', claim_owner = NULL, claim_token = NULL,
                claim_until = NULL, updated_at = now()
            WHERE tenant_id = $1 AND effect_id = $2 AND claim_token = $3
              AND state = 'dispatching' AND terminal = FALSE
            "#,
        )
        .bind(effect.tenant_id().as_str())
        .bind(effect_id)
        .bind(effect.claim_token())
        .execute(&mut *tx)
        .await
        .persistence()?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Integrity(
                "effect response accepted without owning dispatch lease".to_string(),
            ));
        }
        tx.commit().await.persistence()?;
        Ok(true)
    }

    async fn load_effect_responses(
        &self,
        tenant_id: &TenantId,
        limit: usize,
    ) -> StoreResult<Vec<PendingEffectResponse>> {
        let limit = i64::try_from(limit)
            .map_err(|_| StoreError::Integrity("effect batch limit exceeds i64".to_string()))?;
        let tenant = tenant_id.clone();
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;
        let rows = sqlx::query(
            r#"
            SELECT effect.instance_id, COALESCE(payload.payload, effect.input) AS input,
                   inbox.response
            FROM workflow_effects AS effect
            JOIN workflow_inbox AS inbox
              ON inbox.tenant_id = effect.tenant_id AND inbox.effect_id = effect.effect_id
            LEFT JOIN workflow_payloads AS payload
              ON payload.tenant_id = effect.tenant_id
             AND payload.payload_hash = effect.input_ref_hash
            WHERE effect.tenant_id = $1 AND effect.state = 'accepted'
              AND effect.terminal = FALSE
            ORDER BY inbox.received_at, effect.effect_id
            LIMIT $2
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .persistence()?;
        tx.commit().await.persistence()?;
        use sqlx::Row;
        rows.into_iter()
            .map(|row| {
                let response: Vec<u8> = row.get("response");
                let effect: Vec<u8> = row.get("input");
                Ok(PendingEffectResponse::new(
                    tenant.clone(),
                    row.get("instance_id"),
                    serde_json::from_slice(&effect).persistence()?,
                    serde_json::from_slice(&response).persistence()?,
                ))
            })
            .collect()
    }

    async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, effect.tenant_id().as_str())
            .await
            .persistence()?;
        sqlx::query(
            r#"
            UPDATE workflow_effects
            SET state = 'pending', claim_owner = NULL, claim_token = NULL,
                claim_until = NULL, updated_at = now()
            WHERE tenant_id = $1 AND effect_id = $2 AND claim_token = $3
              AND state = 'dispatching' AND terminal = FALSE
            "#,
        )
        .bind(effect.tenant_id().as_str())
        .bind(effect.effect().effect_id().as_uuid())
        .bind(effect.claim_token())
        .execute(&mut *tx)
        .await
        .persistence()?;
        tx.commit().await.persistence()?;
        Ok(())
    }

    async fn schedule_effect_retry(
        &self,
        effect: &ClaimedEffect,
        decision: RetryDecision,
        error: &str,
    ) -> StoreResult<()> {
        let (state, terminal, attempt, due_at) = match decision {
            RetryDecision::At { attempt, due_at } => (
                "pending",
                false,
                i32::try_from(attempt)
                    .map_err(|_| StoreError::Integrity("effect attempt exceeds i32".to_string()))?,
                epoch_ms_to_datetime(
                    i64::try_from(due_at).map_err(|_| {
                        StoreError::Integrity("effect due_at exceeds i64".to_string())
                    })?,
                ),
            ),
            RetryDecision::Exhausted => (
                "dead_letter",
                true,
                i32::try_from(effect.attempt().saturating_add(1))
                    .map_err(|_| StoreError::Integrity("effect attempt exceeds i32".to_string()))?,
                chrono::Utc::now(),
            ),
            RetryDecision::Terminal => (
                "failed",
                true,
                i32::try_from(effect.attempt())
                    .map_err(|_| StoreError::Integrity("effect attempt exceeds i32".to_string()))?,
                chrono::Utc::now(),
            ),
        };
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, effect.tenant_id().as_str())
            .await
            .persistence()?;
        let updated = sqlx::query(
            r#"
            UPDATE workflow_effects
            SET state = $1, terminal = $2, attempt = $3, next_due_at = $4,
                last_error = $5, claim_owner = NULL, claim_token = NULL,
                claim_until = NULL, updated_at = now()
            WHERE tenant_id = $6 AND effect_id = $7 AND claim_token = $8
              AND state = 'dispatching' AND terminal = FALSE
            "#,
        )
        .bind(state)
        .bind(terminal)
        .bind(attempt)
        .bind(due_at)
        .bind(error)
        .bind(effect.tenant_id().as_str())
        .bind(effect.effect().effect_id().as_uuid())
        .bind(effect.claim_token())
        .execute(&mut *tx)
        .await
        .persistence()?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Integrity(
                "effect dispatch lease is stale".to_string(),
            ));
        }
        tx.commit().await.persistence()?;
        Ok(())
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
    ) -> StoreResult<()> {
        self.execute_tenant_scoped(tenant_id.as_str(), owner, |tx| {
            Box::pin(async move { Self::release_instance_transition_inner(tx, instance_id).await })
        })
        .await
    }

    async fn join_get(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        join_id: JoinId,
    ) -> StoreResult<u16> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;

        let row = sqlx::query(
            "SELECT arrive_count FROM join_barriers WHERE tenant_id = $1 AND instance_id = $2 AND join_id = $3",
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .bind(join_id as i32)
        .fetch_optional(&mut *tx)
        .await.persistence()?;

        let count = match row {
            None => 0,
            Some(row) => {
                use sqlx::Row;
                let count: i16 = row.get("arrive_count");
                count as u16
            }
        };

        tx.commit().await.persistence()?;
        Ok(count)
    }
}

#[async_trait]
impl ArtifactRepository for PostgresWorkflowStore {
    // ── Program store ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> StoreResult<()> {
        let json = serde_json::to_value(program).persistence()?;
        let result = sqlx::query(
            r#"
            INSERT INTO compiled_programs (bytecode_version, program)
            VALUES ($1, $2)
            ON CONFLICT (bytecode_version) DO UPDATE
            SET program = compiled_programs.program
            WHERE compiled_programs.program = EXCLUDED.program
            "#,
        )
        .bind(&version[..])
        .bind(&json)
        .execute(&self.pool)
        .await
        .persistence()?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Integrity(
                "legacy artifact hash collision".to_string(),
            ));
        }
        Ok(())
    }

    async fn store_artifact(
        &self,
        artifact: &ExecutableWorkflow,
    ) -> std::result::Result<(), ArtifactStoreError> {
        let bytes = artifact
            .canonical_bytes()
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        let program: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        let hash = artifact.hash().into_bytes();
        let abi = i32::try_from(artifact.envelope().abi_version())
            .map_err(|_| ArtifactStoreError::InvalidArtifact("ABI exceeds i32".to_string()))?;
        let result = sqlx::query(
            r#"
            INSERT INTO compiled_programs (
                bytecode_version, program, artifact_hash, canonical_bytes, abi_version
            ) VALUES ($1, $2, $1, $3, $4)
            ON CONFLICT (bytecode_version) DO UPDATE
            SET canonical_bytes = compiled_programs.canonical_bytes
            WHERE compiled_programs.canonical_bytes = EXCLUDED.canonical_bytes
              AND compiled_programs.artifact_hash = EXCLUDED.artifact_hash
              AND compiled_programs.abi_version = EXCLUDED.abi_version
            "#,
        )
        .bind(&hash[..])
        .bind(program)
        .bind(&bytes)
        .bind(abi)
        .execute(&self.pool)
        .await
        .map_err(|error| ArtifactStoreError::Unavailable(error.to_string()))?;
        if result.rows_affected() != 1 {
            return Err(ArtifactStoreError::ArtifactCollision { hash });
        }
        Ok(())
    }

    async fn load_artifact(
        &self,
        hash: ArtifactHash,
    ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError> {
        use sqlx::Row;
        let requested = hash.into_bytes();
        let row = sqlx::query(
            r#"
            SELECT canonical_bytes
            FROM compiled_programs
            WHERE artifact_hash = COALESCE(
                (SELECT new_hash FROM artifact_lineage WHERE old_hash = $1),
                $1
            )
            "#,
        )
        .bind(&requested[..])
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| ArtifactStoreError::Unavailable(error.to_string()))?;
        if let Some(row) = row {
            let bytes: Vec<u8> = row.get("canonical_bytes");
            return ExecutableWorkflow::verify(&bytes)
                .map(Some)
                .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()));
        }

        let legacy_row =
            sqlx::query("SELECT program FROM compiled_programs WHERE bytecode_version = $1")
                .bind(&requested[..])
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| ArtifactStoreError::Unavailable(error.to_string()))?;
        let Some(legacy_row) = legacy_row else {
            return Ok(None);
        };
        let legacy_json: serde_json::Value = legacy_row.get("program");
        let legacy: CompiledProgram = serde_json::from_value(legacy_json)
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        let envelope = ArtifactEnvelope::from_legacy_program(legacy, "legacy-adapter")
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        let artifact = ExecutableWorkflow::from_verified_envelope(envelope)
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        self.store_artifact(&artifact).await.persistence()?;
        let new_hash = artifact.hash().into_bytes();
        let lineage = sqlx::query(
            r#"
            INSERT INTO artifact_lineage (old_hash, new_hash)
            VALUES ($1, $2)
            ON CONFLICT (old_hash) DO UPDATE
            SET new_hash = artifact_lineage.new_hash
            WHERE artifact_lineage.new_hash = EXCLUDED.new_hash
            "#,
        )
        .bind(&requested[..])
        .bind(&new_hash[..])
        .execute(&self.pool)
        .await
        .map_err(|error| ArtifactStoreError::Unavailable(error.to_string()))?;
        if lineage.rows_affected() != 1 {
            return Err(ArtifactStoreError::ArtifactCollision { hash: requested });
        }
        Ok(Some(artifact))
    }

    async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>> {
        let row = sqlx::query(
            "SELECT program, canonical_bytes FROM compiled_programs WHERE bytecode_version = $1",
        )
        .bind(&version[..])
        .fetch_optional(&self.pool)
        .await
        .persistence()?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let canonical: Option<Vec<u8>> = row.get("canonical_bytes");
                if let Some(bytes) = canonical {
                    let artifact = ExecutableWorkflow::verify(&bytes).persistence()?;
                    return Ok(Some(artifact.to_legacy_program()));
                }
                let json: serde_json::Value = row.get("program");
                Ok(Some(serde_json::from_value(json).persistence()?))
            }
        }
    }

    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()> {
        let plan_json_value: serde_json::Value =
            serde_json::from_str(plan_json).map_err(|error| {
                StoreError::Unavailable(format!("store_plan: invalid JSON: {error}"))
            })?;
        sqlx::query(
            r#"
            INSERT INTO workflow_plans (plan_hash, plan_body)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(&plan_hash[..])
        .bind(&plan_json_value)
        .execute(&self.pool)
        .await
        .map_err(|error| StoreError::Unavailable(format!("store_plan: insert failed: {error}")))?;
        Ok(())
    }

    async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>> {
        let row: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT plan_body FROM workflow_plans WHERE plan_hash = $1")
                .bind(&plan_hash[..])
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("load_plan: query failed: {error}"))
                })?;
        Ok(row.map(|v| v.to_string()))
    }
}

#[async_trait]
impl JournalReader for PostgresWorkflowStore {
    // ── Event log ──

    async fn read_events(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        from_seq: u64,
    ) -> StoreResult<Vec<(u64, RuntimeEvent)>> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;

        let rows = sqlx::query(
            "SELECT seq, event FROM event_log WHERE tenant_id = $1 AND instance_id = $2 AND seq >= $3 ORDER BY seq",
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .bind(from_seq as i64)
        .fetch_all(&mut *tx)
        .await.persistence()?;

        tx.commit().await.persistence()?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row;
            let seq: i64 = row.get("seq");
            let event_json: serde_json::Value = row.get("event");
            let event: RuntimeEvent = serde_json::from_value(event_json).persistence()?;
            events.push((seq as u64, event));
        }
        Ok(events)
    }

    async fn load_snapshot_envelope(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Option<SnapshotEnvelope>> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;
        let bytes: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT snapshot_envelope FROM workflow_instances WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .fetch_optional(&mut *tx)
        .await.persistence()?
        .flatten();
        tx.commit().await.persistence()?;
        bytes
            .map(|bytes| SnapshotEnvelope::decode(&bytes).map_err(StoreError::integrity))
            .transpose()
    }

    async fn read_journal(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        after_revision: Option<u64>,
    ) -> StoreResult<Vec<JournalRecord>> {
        let after_revision = after_revision
            .map(|revision| {
                i64::try_from(revision).map_err(|error| {
                    StoreError::Invalid(format!("journal revision exceeds BIGINT: {error}"))
                })
            })
            .transpose()?;
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;
        let rows: Vec<Vec<u8>> = sqlx::query_scalar(
            r#"
            SELECT record_envelope
            FROM workflow_journal
            WHERE tenant_id = $1 AND instance_id = $2
              AND ($3::BIGINT IS NULL OR new_revision > $3)
            ORDER BY new_revision
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .bind(after_revision)
        .fetch_all(&mut *tx)
        .await
        .persistence()?;
        tx.commit().await.persistence()?;
        rows.into_iter()
            .map(|bytes| JournalRecord::decode(&bytes).map_err(StoreError::integrity))
            .collect()
    }

    // ── Payload history ──

    async fn load_payload_version(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> StoreResult<Option<String>> {
        let mut tx = self.pool.begin().await.persistence()?;
        Self::set_tenant_context(&mut tx, tenant_id.as_str())
            .await
            .persistence()?;

        let row = sqlx::query(
            "SELECT domain_payload FROM payload_history WHERE tenant_id = $1 AND instance_id = $2 AND payload_hash = $3",
        )
        .bind(tenant_id.as_str())
        .bind(instance_id)
        .bind(&hash[..])
        .fetch_optional(&mut *tx)
        .await.persistence()?;

        tx.commit().await.persistence()?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                Ok(Some(row.get("domain_payload")))
            }
        }
    }
}

#[async_trait]
impl AdminProjectionStore for PostgresWorkflowStore {
    async fn store_template(
        &self,
        name: &str,
        version: u32,
        plan_hash: [u8; 32],
        dsl_body: &str,
    ) -> StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO workflow_template_catalog (template_name, version, plan_hash, dsl_body)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (template_name, version) DO NOTHING
            "#,
        )
        .bind(name)
        .bind(version as i32)
        .bind(&plan_hash[..])
        .bind(dsl_body)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            StoreError::Unavailable(format!("store_template: insert failed: {error}"))
        })?;
        Ok(())
    }

    async fn load_template_version(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, [u8; 32])>> {
        let row = sqlx::query(
            "SELECT dsl_body, plan_hash FROM workflow_template_catalog WHERE template_name = $1 AND version = $2",
        )
        .bind(name)
        .bind(version as i32)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Unavailable(format!("load_template_version: query failed: {error}")))?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let dsl_body: String = row.get("dsl_body");
                let hash_bytes: Vec<u8> = row.get("plan_hash");
                let mut hash = [0u8; 32];
                if hash_bytes.len() == 32 {
                    hash.copy_from_slice(&hash_bytes);
                }
                Ok(Some((dsl_body, hash)))
            }
        }
    }

    async fn load_latest_template_version(
        &self,
        name: &str,
    ) -> StoreResult<Option<(u32, String, [u8; 32])>> {
        let row = sqlx::query(
            "SELECT version, dsl_body, plan_hash FROM workflow_template_catalog WHERE template_name = $1 ORDER BY version DESC LIMIT 1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Unavailable(format!("load_latest_template_version: query failed: {error}")))?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let version: i32 = row.get("version");
                let dsl_body: String = row.get("dsl_body");
                let hash_bytes: Vec<u8> = row.get("plan_hash");
                let mut hash = [0u8; 32];
                if hash_bytes.len() == 32 {
                    hash.copy_from_slice(&hash_bytes);
                }
                Ok(Some((version as u32, dsl_body, hash)))
            }
        }
    }

    async fn list_templates(&self) -> StoreResult<Vec<bpmn_lite_store::TemplateSummary>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT ON (template_name) template_name, version, plan_hash, created_at
            FROM workflow_template_catalog
            ORDER BY template_name, version DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            StoreError::Unavailable(format!("list_templates: query failed: {error}"))
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            use sqlx::Row;
            let name: String = row.get("template_name");
            let version: i32 = row.get("version");
            let hash_bytes: Vec<u8> = row.get("plan_hash");
            let created_at_time: chrono::DateTime<chrono::Utc> = row.get("created_at");
            let mut hash = [0u8; 32];
            if hash_bytes.len() == 32 {
                hash.copy_from_slice(&hash_bytes);
            }
            summaries.push(bpmn_lite_store::TemplateSummary {
                name,
                latest_version: version as u32,
                plan_hash: hash,
                created_at: created_at_time.to_rfc3339(),
            });
        }
        Ok(summaries)
    }

    async fn health_check(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .persistence()?;
        Ok(())
    }

    async fn ensure_tenant(&self, tenant_id: &TenantId) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO tenants (tenant_id, pool_id) VALUES ($1, 'default') ON CONFLICT DO NOTHING",
        )
        .bind(tenant_id.as_str())
        .execute(&self.pool)
        .await.persistence()?;
        Ok(())
    }

    async fn list_tenants(&self) -> StoreResult<Vec<String>> {
        let rows = sqlx::query("SELECT tenant_id FROM tenants ORDER BY first_seen_at")
            .fetch_all(&self.pool)
            .await
            .persistence()?;
        use sqlx::Row;
        Ok(rows
            .iter()
            .map(|r| r.get::<String, _>("tenant_id"))
            .collect())
    }

    async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>> {
        let rows =
            sqlx::query("SELECT tenant_id FROM tenants WHERE pool_id = $1 ORDER BY first_seen_at")
                .bind(pool_id)
                .fetch_all(&self.pool)
                .await
                .persistence()?;
        use sqlx::Row;
        Ok(rows
            .iter()
            .map(|r| r.get::<String, _>("tenant_id"))
            .collect())
    }
}

impl PostgresWorkflowStore {
    /// V6.4 cutover-readiness gate: verify EVERY stored artifact, not just the
    /// ones a running instance happens to load. Closes the lazy-verification
    /// gap — `load_artifact` verifies on demand, so a corrupt or pre-canonical
    /// row can sit undetected until claimed. Fails closed, naming every
    /// offending `bytecode_version`; a NULL `canonical_bytes` (pre-canonical
    /// legacy row) is itself a failure — a cutover-clean corpus has none.
    /// Returns the count verified.
    pub async fn verify_artifact_corpus(&self) -> Result<usize> {
        use sqlx::Row;
        let rows = sqlx::query("SELECT bytecode_version, canonical_bytes FROM compiled_programs")
            .fetch_all(&self.pool)
            .await
            .persistence()?;
        let mut failures = Vec::new();
        for row in &rows {
            let version: Vec<u8> = row.get("bytecode_version");
            let canonical: Option<Vec<u8>> = row.get("canonical_bytes");
            let hash_hex: String = version.iter().map(|b| format!("{b:02x}")).collect();
            match canonical {
                None => failures.push(format!("{hash_hex}: pre-canonical (canonical_bytes IS NULL)")),
                Some(bytes) => {
                    if let Err(error) = ExecutableWorkflow::verify(&bytes) {
                        failures.push(format!("{hash_hex}: {error}"));
                    }
                }
            }
        }
        if !failures.is_empty() {
            return Err(StoreError::Integrity(format!(
                "artifact corpus verification failed for {} of {} artifact(s): {}",
                failures.len(),
                rows.len(),
                failures.join("; ")
            )));
        }
        Ok(rows.len())
    }

    async fn dequeue_jobs_inner(
        tx: &mut TenantTx<'_>,
        task_types: &[String],
        max: usize,
        lease_ms: u64,
    ) -> Result<Vec<JobActivation>> {
        let rows = sqlx::query(
            r#"
            WITH claimed AS (
                SELECT job_key
                FROM job_queue
                WHERE status = 'pending'
                  AND tenant_id = $3
                  AND task_type = ANY($1)
                  AND (not_before IS NULL OR not_before <= now())
                ORDER BY created_at
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            ), updated AS (
                UPDATE job_queue
                SET status = 'claimed',
                    claimed_at = now(),
                    worker_id = $4,
                    claim_token = md5(random()::text || clock_timestamp()::text),
                    claim_expires_at = now() + make_interval(secs => $5::float / 1000.0),
                    attempt_count = attempt_count + 1
                FROM claimed
                WHERE job_queue.job_key = claimed.job_key
                RETURNING job_queue.job_key,
                          job_queue.tenant_id,
                          job_queue.process_instance_id,
                          job_queue.task_type,
                          job_queue.service_task_id,
                          job_queue.domain_payload,
                          job_queue.domain_payload_hash,
                          job_queue.payload_ref_hash,
                          job_queue.session_stack,
                          job_queue.orch_flags,
                          job_queue.retries_remaining,
                          job_queue.entry_id,
                          job_queue.runbook_id,
                          job_queue.worker_id,
                          job_queue.claim_token,
                          job_queue.claim_expires_at,
                          job_queue.attempt_count,
                          job_queue.failure_count,
                          job_queue.not_before
            )
            SELECT updated.*,
                   COALESCE(convert_from(payload.payload, 'UTF8'), updated.domain_payload)
                       AS hydrated_domain_payload
            FROM updated
            LEFT JOIN workflow_payloads AS payload
              ON payload.tenant_id = updated.tenant_id
             AND payload.payload_hash = updated.payload_ref_hash
            "#,
        )
        .bind(task_types)
        .bind(max as i64)
        .bind(&tx.tenant_id)
        .bind(&tx.lease_owner)
        .bind(lease_ms as f64)
        .fetch_all(&mut *tx.tx)
        .await
        .persistence()?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row;
            let hash: Vec<u8> = row.get("domain_payload_hash");
            let session_stack_json: serde_json::Value = row.get("session_stack");
            let orch_flags_json: serde_json::Value = row.get("orch_flags");
            let retries: i32 = row.get("retries_remaining");
            let claim_expires_at: Option<chrono::DateTime<chrono::Utc>> =
                row.get("claim_expires_at");
            let not_before: Option<chrono::DateTime<chrono::Utc>> = row.get("not_before");
            let attempt_count: i32 = row.get("attempt_count");
            let failure_count: i32 = row.get("failure_count");

            result.push(JobActivation {
                job_key: row.get("job_key"),
                tenant_id: row.get("tenant_id"),
                process_instance_id: row.get("process_instance_id"),
                task_type: row.get("task_type"),
                service_task_id: row.get("service_task_id"),
                domain_payload: row.get("hydrated_domain_payload"),
                domain_payload_hash: bytes_to_hash(hash)?,
                session_stack: serde_json::from_value(session_stack_json).persistence()?,
                orch_flags: serde_json::from_value(orch_flags_json).persistence()?,
                retries_remaining: retries as u32,
                entry_id: row.get("entry_id"),
                runbook_id: row.get("runbook_id"),
                worker_id: row.get("worker_id"),
                claim_token: row.get("claim_token"),
                claim_expires_at: claim_expires_at.map(datetime_to_epoch_ms),
                attempt_count: attempt_count as u32,
                failure_count: failure_count as u32,
                not_before: not_before.map(datetime_to_epoch_ms),
            });
        }
        Ok(result)
    }

    async fn reclaim_stale_jobs_inner(
        tx: &mut TenantTx<'_>,
        timeout_ms: u64,
    ) -> Result<Vec<StaleReclaimInfo>> {
        let rows = sqlx::query(
            r#"
            WITH stale AS (
                SELECT job_key, process_instance_id, worker_id AS previous_worker_id, retries_remaining
                FROM job_queue
                WHERE status = 'claimed'
                  AND claimed_at < now() - make_interval(secs => $1::float / 1000.0)
                FOR UPDATE SKIP LOCKED
            ),
            dead_lettered AS (
                UPDATE job_queue
                SET status = 'dead_lettered',
                    claimed_at = NULL,
                    worker_id = NULL,
                    claim_token = NULL,
                    claim_expires_at = NULL,
                    dead_lettered_at = now(),
                    last_failed_at = now(),
                    last_error = 'stale claimed job exhausted retry budget'
                FROM stale
                WHERE job_queue.job_key = stale.job_key
                  AND stale.retries_remaining <= 1
                RETURNING job_queue.job_key, job_queue.process_instance_id, stale.previous_worker_id
            ),
            reclaimed AS (
                UPDATE job_queue
                SET status = 'pending',
                    claimed_at = NULL,
                    worker_id = NULL,
                    claim_token = NULL,
                    claim_expires_at = NULL,
                    retries_remaining = job_queue.retries_remaining - 1,
                    last_failed_at = now(),
                    last_error = 'stale claimed job reclaimed'
                FROM stale
                WHERE job_queue.job_key = stale.job_key
                  AND stale.retries_remaining > 1
                RETURNING job_queue.job_key, job_queue.process_instance_id, stale.previous_worker_id
            )
            SELECT job_key, process_instance_id, previous_worker_id FROM reclaimed
            UNION ALL
            SELECT job_key, process_instance_id, previous_worker_id FROM dead_lettered
            "#,
        )
        .bind(timeout_ms as f64)
        .fetch_all(&mut *tx.tx)
        .await.persistence()?;

        use sqlx::Row;
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let item = StaleReclaimInfo {
                job_key: row.get("job_key"),
                process_instance_id: row.get("process_instance_id"),
                previous_worker_id: row.get("previous_worker_id"),
            };
            let event = serde_json::to_value(RuntimeEvent::JobReclaimed {
                job_key: item.job_key.clone(),
                previous_worker_id: item.previous_worker_id.clone(),
            })
            .persistence()?;
            let inserted = sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                    VALUES ($1, 1, $3)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq, tenant_id
                )
                INSERT INTO event_log (instance_id, seq, event, tenant_id)
                SELECT $1, seq.next_seq, $2, seq.tenant_id FROM seq
                "#,
            )
            .bind(item.process_instance_id)
            .bind(event)
            .bind(&tx.tenant_id)
            .execute(&mut *tx.tx)
            .await
            .persistence()?;
            tx.assert_rows_affected(&inserted, 1, "reclaim_stale_job_event")?;
            results.push(item);
        }
        Ok(results)
    }

    async fn release_instance_transition_inner(
        tx: &mut TenantTx<'_>,
        instance_id: Uuid,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE workflow_instances
            SET lease_owner = NULL,
                lease_until = NULL
            WHERE tenant_id = $1
              AND instance_id = $2
              AND lease_owner = $3
            "#,
        )
        .bind(&tx.tenant_id)
        .bind(instance_id)
        .bind(&tx.lease_owner)
        .execute(&mut *tx.tx)
        .await
        .persistence()?;

        Ok(())
    }
}

// The whole crate is postgres-only — no need for the inner cfg-gate
// that store_postgres used when it lived inside bpmn-lite-core
// behind `cfg(feature = "postgres")`. Tests still need a real
// database (`BPMN_LITE_TEST_DATABASE_URL`) and run in the mandatory
// PostgreSQL CI job.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pending_store::PostgresPendingInvocationStore;
    use bpmn_lite_engine::BpmnLiteEngine;
    use bpmn_lite_store::pending::{PendingInvocation, PendingInvocationStore};
    use sqlx::PgPool;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

    /// V2.6 (R1 mitigation c): the sampling decision itself is a pure,
    /// deterministic function of revision number — no env mutation needed
    /// to prove the gate. Genesis (revision 0) always samples under the
    /// default rate, so every test in this module that commits at least
    /// once already exercises the live round-trip path at
    /// `commit_transition`'s frame_hash computation site; this test proves
    /// the *rate itself* is config-gated and revision-keyed, not that a
    /// single commit happened to pass.
    #[test]
    fn canonical_round_trip_sampling_is_deterministic_and_rate_gated() {
        assert!(
            should_sample_canonical_round_trip(0),
            "genesis must always sample"
        );
        assert!(should_sample_canonical_round_trip(DEFAULT_CANONICAL_SAMPLE_RATE));
        assert!(should_sample_canonical_round_trip(2 * DEFAULT_CANONICAL_SAMPLE_RATE));
        assert!(!should_sample_canonical_round_trip(1));
        assert!(!should_sample_canonical_round_trip(DEFAULT_CANONICAL_SAMPLE_RATE - 1));
        assert!(!should_sample_canonical_round_trip(
            DEFAULT_CANONICAL_SAMPLE_RATE + 1
        ));
    }

    async fn setup() -> (
        PgPool,
        PostgresWorkflowStore,
        tokio::sync::MutexGuard<'static, ()>,
    ) {
        let guard = crate::test_lock::get_mutex().lock().await;
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
        let pool = PgPool::connect(&url).await.expect("connect to db");

        // Run migrations
        let migrator = sqlx::migrate!("./migrations");
        migrator.run(&pool).await.expect("run migrations");

        let db_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        let grant_sql = format!(
            "GRANT CONNECT, TEMPORARY ON DATABASE \"{}\" TO bpmn_lite_app",
            db_name
        );
        sqlx::query(&grant_sql).execute(&pool).await.unwrap();
        sqlx::query("GRANT USAGE ON SCHEMA public TO bpmn_lite_app")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO bpmn_lite_app",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO bpmn_lite_app")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("DROP SCHEMA IF EXISTS dsl_bus CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE SCHEMA dsl_bus")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("GRANT USAGE ON SCHEMA dsl_bus TO bpmn_lite_app")
            .execute(&pool)
            .await
            .unwrap();

        // Run bus migrations under admin role with search_path=dsl_bus
        let mut bus_admin_url = url.clone();
        if bus_admin_url.contains('?') {
            bus_admin_url.push_str("&options=-csearch_path%3Ddsl_bus");
        } else {
            bus_admin_url.push_str("?options=-csearch_path%3Ddsl_bus");
        }
        let bus_admin_pool = PgPool::connect(&bus_admin_url)
            .await
            .expect("connect to db for bus migrations");
        dsl_bus_storage::migrate(&bus_admin_pool)
            .await
            .expect("run bus migrations");
        bus_admin_pool.close().await;

        // Grant bpmn_lite_app DML-only privileges on dsl_bus tables and sequences
        sqlx::query(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dsl_bus TO bpmn_lite_app",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA dsl_bus TO bpmn_lite_app")
            .execute(&pool)
            .await
            .unwrap();

        use std::str::FromStr;
        let mut options = sqlx::postgres::PgConnectOptions::from_str(&url).expect("parse db url");
        options = options
            .username("bpmn_lite_app")
            .password("bpmn_lite_app_dev_password");
        let app_pool = PgPool::connect_with(options)
            .await
            .expect("connect as bpmn_lite_app");

        // Truncate all tables
        sqlx::query("TRUNCATE dsl_bus.outbox CASCADE")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("TRUNCATE workflow_instances CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE compiled_programs")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE job_queue")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE dedupe_cache")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE message_dedupe")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE dead_letter_queue")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE event_sequences")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE event_log")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE payload_history")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE incidents")
            .execute(&pool)
            .await
            .unwrap();
        // Artifact migration lineage persists a new_hash per old_hash; without
        // this truncation a re-run of the migration tests collides on a hash
        // stored by the prior run (the artifact hash is content-deterministic).
        sqlx::query("TRUNCATE artifact_lineage")
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresWorkflowStore::new(app_pool);
        (pool, store, guard)
    }

    fn test_hash(data: &str) -> [u8; 32] {
        blake3::hash(data.as_bytes()).into()
    }

    async fn commit_ops(
        store: &PostgresWorkflowStore,
        instance_id: Uuid,
        tenant_id: &str,
        owner: &str,
        ops: &[TickOperation],
    ) -> Result<()> {
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                owner,
                30_000,
            )
            .await
            .map_err(StoreError::integrity)?
            .ok_or_else(|| StoreError::Integrity("test instance is not claimable".to_string()))?;
        let current = store
            .load_instance(&TenantId::new("default").unwrap(), instance_id)
            .await
            .persistence()?
            .ok_or_else(|| StoreError::Integrity("test instance is missing".to_string()))?;
        let transition = transition_from_tick_ops(&current, ops);
        store
            .commit_transition(&claim, &transition)
            .await
            .map(|_| ())
            .map_err(StoreError::integrity)
    }

    async fn complete_via_transition(
        store: &PostgresWorkflowStore,
        tenant_id: &str,
        owner: &str,
        instance: &ProcessInstance,
        completion: &JobCompletion,
        events: &[RuntimeEvent],
    ) -> Result<()> {
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance.instance_id,
                owner,
                30_000,
            )
            .await
            .map_err(StoreError::integrity)?
            .ok_or_else(|| StoreError::Integrity("test instance is not claimable".to_string()))?;
        let mut builder = TransitionBuilder::new(instance.clone())
            .dedupe(DedupeWrite::new(
                completion.job_key.clone(),
                completion.clone(),
            ))
            .ack_job(completion.job_key.clone());
        for event in events {
            builder = builder.event(event.clone());
        }
        store
            .commit_transition(&claim, &builder.build())
            .await
            .map(|_| ())
            .map_err(StoreError::integrity)
    }

    fn make_instance(id: Uuid) -> ProcessInstance {
        let payload = r#"{"case_id":"abc"}"#;
        let hash = test_hash(payload);
        ProcessInstance {
            instance_id: id,
            tenant_id: "default".to_string(),
            process_key: "test-process".to_string(),
            bytecode_version: [0u8; 32],
            domain_payload: payload.to_string().into(),
            domain_payload_hash: hash,
            session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
            flags: BTreeMap::from([(0, Value::Bool(true)), (1, Value::I64(42))]),
            counters: BTreeMap::from([(0, 5), (1, 10)]),
            join_expected: BTreeMap::from([(0, 3)]),
            state: ProcessState::Running,
            correlation_id: "runbook-entry-1".to_string(),
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            created_at: 1700000000000,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        }
    }

    /// V6.4 fail-closed budget read: a guard cancellation requires the
    /// instance's pinned artifact to be present. Stores a minimal verifying
    /// guarded artifact with an EMPTY per-guard budget table and the given
    /// workflow-level default, returning its hash for the instance to pin.
    async fn store_default_budget_artifact(
        store: &PostgresWorkflowStore,
        default_max: u32,
    ) -> [u8; 32] {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [0u8; 32],
            program: vec![
                Instr::V2Guard { handler: Addr::new(3) },
                Instr::V2GuardEnd,
                Instr::End,
                Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        .with_v2_guard_budgets(
            BTreeMap::new(),
            bpmn_lite_types::ScopeFailureBudget::new(1, default_max).unwrap(),
        );
        let workflow = bpmn_lite_types::ExecutableWorkflow::from_verified_envelope(
            bpmn_lite_types::ArtifactEnvelope::from_legacy_program(program, "v6-4-default-budget")
                .unwrap(),
        )
        .unwrap();
        store.store_artifact(&workflow).await.unwrap();
        workflow.hash().into_bytes()
    }

    /// T-PG-1: Instance round-trip
    #[tokio::test]
    async fn test_pg_instance_round_trip() {
        let (_pool, store, _lock) = setup().await;
        let id = Uuid::now_v7();
        let inst = make_instance(id);

        store.save_instance("default", &inst).await.unwrap();
        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.instance_id, id);
        assert_eq!(loaded.process_key, "test-process");
        assert_eq!(loaded.domain_payload, inst.domain_payload);
        assert_eq!(loaded.domain_payload_hash, inst.domain_payload_hash);
        assert_eq!(loaded.bytecode_version, [0u8; 32]);
        assert_eq!(loaded.flags.len(), 2);
        assert_eq!(loaded.flags[&0], Value::Bool(true));
        assert_eq!(loaded.flags[&1], Value::I64(42));
        assert_eq!(loaded.counters[&0], 5);
        assert_eq!(loaded.counters[&1], 10);
        assert_eq!(loaded.join_expected[&0], 3);
        assert_eq!(loaded.state, ProcessState::Running);
        assert_eq!(loaded.correlation_id, "runbook-entry-1");
        // Timestamp round-trip: allow 1s drift due to ms→timestamptz→ms
        assert!((loaded.created_at - inst.created_at).abs() < 1000);
    }

    /// T-PG-1b: Session stack persists independently as a copied value.
    #[tokio::test]
    async fn test_pg_instance_session_stack_copy_round_trip() {
        let (_pool, store, _lock) = setup().await;
        let id = Uuid::now_v7();
        let original_scope_id = Uuid::new_v4();
        let mutated_scope_id = Uuid::new_v4();

        let mut inst = make_instance(id);
        inst.session_stack = bpmn_lite_types::session_stack::SessionStackState {
            session_id: Uuid::now_v7(),
            scope: Some(bpmn_lite_types::session_stack::SessionScopeState {
                client_group_id: original_scope_id,
                client_group_name: Some("Original".to_string()),
            }),
            active_workspace: Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Kyc),
            workspace_stack: Vec::new(),
            trace_sequence: 17,
        };

        store.save_instance("default", &inst).await.unwrap();

        inst.session_stack.scope = Some(bpmn_lite_types::session_stack::SessionScopeState {
            client_group_id: mutated_scope_id,
            client_group_name: Some("Mutated".to_string()),
        });
        inst.session_stack.active_workspace =
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Deal);
        inst.session_stack.trace_sequence = 99;

        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded
                .session_stack
                .scope
                .as_ref()
                .map(|scope| scope.client_group_id),
            Some(original_scope_id)
        );
        assert_eq!(
            loaded.session_stack.active_workspace,
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Kyc)
        );
        assert_eq!(loaded.session_stack.trace_sequence, 17);
    }

    /// T-PG-2: Fiber round-trip
    #[tokio::test]
    async fn test_pg_fiber_round_trip() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let fid = Uuid::now_v7();

        // Need an instance first (FK constraint)
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        let mut fiber = Fiber::new(fid, 10);
        fiber.wait = WaitState::Job {
            job_key: "job-123".to_string(),
        };
        fiber.stack.push(Value::I64(99));
        fiber.loop_epoch = 3;

        store.save_fiber(iid, &fiber).await.unwrap();
        let loaded = store
            .load_fiber(&TenantId::new("default").unwrap(), iid, fid)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.fiber_id, fid);
        assert_eq!(loaded.pc, 10.into());
        assert_eq!(
            loaded.wait,
            WaitState::Job {
                job_key: "job-123".to_string()
            }
        );
        assert_eq!(loaded.stack, vec![Value::I64(99)]);
        assert_eq!(loaded.loop_epoch, 3);

        // Delete
        store.delete_fiber(iid, fid).await.unwrap();
        assert!(store
            .load_fiber(&TenantId::new("default").unwrap(), iid, fid)
            .await
            .unwrap()
            .is_none());
    }

    /// T-PG-3: Join barrier
    #[tokio::test]
    async fn test_pg_join_barrier() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        let join_id: JoinId = 0;
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 1);
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 2);
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 3);

        store.join_reset(iid, join_id).await.unwrap();
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 1);
    }

    /// T-PG-4: Dedupe
    #[tokio::test]
    async fn test_pg_dedupe() {
        let (_pool, store, _lock) = setup().await;
        let completion = JobCompletion {
            job_key: "job-abc".to_string(),
            domain_payload: r#"{"done":true}"#.to_string(),
            expected_instance_payload_hash: test_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };

        assert!(store
            .dedupe_get(&TenantId::new("default").unwrap(), "job-abc")
            .await
            .unwrap()
            .is_none());
        store
            .dedupe_put("default", "job-abc", &completion)
            .await
            .unwrap();

        let cached = store
            .dedupe_get(&TenantId::new("default").unwrap(), "job-abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.job_key, "job-abc");
        assert_eq!(cached.domain_payload, r#"{"done":true}"#);

        // Idempotent put
        store
            .dedupe_put("default", "job-abc", &completion)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_pg_dedupe_rls_isolation() {
        let (_pool, store, _lock) = setup().await;
        let completion = JobCompletion {
            job_key: "job-tenant-isolation".to_string(),
            domain_payload: r#"{"done":true}"#.to_string(),
            expected_instance_payload_hash: test_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };

        // Initially both tenants see nothing
        assert!(store
            .dedupe_get(&TenantId::new("tenant-A").unwrap(), "job-tenant-isolation")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .dedupe_get(&TenantId::new("tenant-B").unwrap(), "job-tenant-isolation")
            .await
            .unwrap()
            .is_none());

        // Put under tenant-A
        store
            .dedupe_put("tenant-A", "job-tenant-isolation", &completion)
            .await
            .unwrap();

        // tenant-B MUST still see nothing (RSL isolation)
        assert!(store
            .dedupe_get(&TenantId::new("tenant-B").unwrap(), "job-tenant-isolation")
            .await
            .unwrap()
            .is_none());

        // tenant-A MUST see the completion
        let cached = store
            .dedupe_get(&TenantId::new("tenant-A").unwrap(), "job-tenant-isolation")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.job_key, "job-tenant-isolation");
    }

    /// T-PG-5: Job queue
    #[tokio::test]
    async fn test_pg_job_queue() {
        let (_pool, store, _lock) = setup().await;
        let task_type = "create_case".to_string();
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        for i in 0..3 {
            store
                .enqueue_job(&JobActivation {
                    job_key: format!("job-{i}"),
                    tenant_id: "default".to_string(),
                    process_instance_id: iid,
                    task_type: task_type.clone(),
                    service_task_id: format!("task-{i}"),
                    domain_payload: "{}".to_string(),
                    domain_payload_hash: [0u8; 32],
                    session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
                    orch_flags: BTreeMap::new(),
                    retries_remaining: 3,
                    entry_id: Uuid::nil(),
                    runbook_id: Uuid::nil(),
                    worker_id: String::new(),
                    claim_token: String::new(),
                    claim_expires_at: None,
                    attempt_count: 0,
                    failure_count: 0,
                    not_before: None,
                })
                .await
                .unwrap();
        }

        // Dequeue 2
        let batch1 = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                2,
                &TenantId::default(),
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch1.len(), 2);

        // Ack one
        store.ack_job("default", &batch1[0].job_key).await.unwrap();

        // Dequeue remaining
        let batch2 = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                10,
                &TenantId::default(),
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch2.len(), 1);
    }

    /// T-PG-6: Event log
    #[tokio::test]
    async fn test_pg_event_log() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        for i in 0..5 {
            let event = RuntimeEvent::FlagSet {
                key: i,
                value: Value::I64(i as i64),
            };
            let seq = store.append_event(iid, &event).await.unwrap();
            assert_eq!(seq, (i + 1) as u64);
        }

        let events = store
            .read_events(&TenantId::new("default").unwrap(), iid, 3)
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].0, 3);
        assert_eq!(events[1].0, 4);
        assert_eq!(events[2].0, 5);
    }

    /// T-PG-7: Payload history
    #[tokio::test]
    async fn test_pg_payload_history() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        let payload_v1 = r#"{"version":1}"#;
        let hash_v1 = test_hash(payload_v1);
        store
            .save_payload_version(iid, &hash_v1, payload_v1)
            .await
            .unwrap();

        let payload_v2 = r#"{"version":2}"#;
        let hash_v2 = test_hash(payload_v2);
        store
            .save_payload_version(iid, &hash_v2, payload_v2)
            .await
            .unwrap();

        let loaded_v1 = store
            .load_payload_version(&TenantId::new("default").unwrap(), iid, &hash_v1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v1, payload_v1);

        let loaded_v2 = store
            .load_payload_version(&TenantId::new("default").unwrap(), iid, &hash_v2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v2, payload_v2);

        let bad_hash = [0xFFu8; 32];
        assert!(store
            .load_payload_version(&TenantId::new("default").unwrap(), iid, &bad_hash)
            .await
            .unwrap()
            .is_none());
    }

    /// T-PG-8: Program store
    #[tokio::test]
    async fn test_pg_program_store() {
        let (_pool, store, _lock) = setup().await;

        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: test_hash("test-program"),
            program: vec![Instr::End],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let version = program.bytecode_version();
        store.store_program(version, &program).await.unwrap();

        let loaded = store.load_program(version).await.unwrap().unwrap();
        assert_eq!(loaded.bytecode_version(), version);
        assert_eq!(loaded.program().len(), 1);

        // Idempotent store
        store.store_program(version, &program).await.unwrap();
    }

    /// T-PG-9: Dead letter
    #[tokio::test]
    async fn test_pg_dead_letter() {
        let (_pool, store, _lock) = setup().await;
        let name = 1u32;
        let corr_key = Value::Str(42);
        let payload = b"test-payload";

        // Put with 5s TTL
        store
            .dead_letter_put(name, &corr_key, payload, 5000)
            .await
            .unwrap();

        // Take immediately — should succeed
        let taken = store.dead_letter_take(name, &corr_key).await.unwrap();
        assert_eq!(taken, Some(payload.to_vec()));

        // Take again — gone
        let taken2 = store.dead_letter_take(name, &corr_key).await.unwrap();
        assert!(taken2.is_none());

        // Put with 0ms TTL (already expired)
        store
            .dead_letter_put(name, &corr_key, payload, 0)
            .await
            .unwrap();

        // Small delay to ensure expires_at is in the past
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Take expired — should be None
        let taken3 = store.dead_letter_take(name, &corr_key).await.unwrap();
        assert!(taken3.is_none());
    }

    /// T-PG-10: Incidents
    #[tokio::test]
    async fn test_pg_incidents() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        for i in 0..2 {
            store
                .save_incident(&Incident {
                    incident_id: Uuid::now_v7(),
                    process_instance_id: iid,
                    fiber_id: Uuid::now_v7(),
                    service_task_id: format!("task-{i}"),
                    bytecode_addr: (i * 10).into(),
                    error_class: ErrorClass::Transient,
                    message: format!("error {i}"),
                    retry_count: 0,
                    created_at: 1700000000000 + (i as i64 * 1000),
                    resolved_at: None,
                    resolution: None,
                })
                .await
                .unwrap();
        }

        let loaded = store
            .load_incidents(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap();
        assert_eq!(loaded.len(), 2);
    }

    /// T-PG-11: Instance updates
    #[tokio::test]
    async fn test_pg_instance_updates() {
        let (_pool, store, _lock) = setup().await;
        let id = Uuid::now_v7();
        store
            .save_instance("test-owner", &make_instance(id))
            .await
            .unwrap();

        // Claim transition
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                id,
                "test-owner",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        // Update state
        let new_state = ProcessState::Completed { at: 1700001000000 };
        store
            .update_instance_state("default", "test-owner", id, new_state.clone())
            .await
            .unwrap();
        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, new_state);

        // Update flags
        let new_flags = BTreeMap::from([(5, Value::Bool(false))]);
        store
            .update_instance_flags("default", "test-owner", id, &new_flags)
            .await
            .unwrap();
        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.flags.len(), 1);
        assert_eq!(loaded.flags[&5], Value::Bool(false));

        // Update payload
        let new_payload = r#"{"updated":true}"#;
        let new_hash = test_hash(new_payload);
        store
            .update_instance_payload("default", "test-owner", id, new_payload, &new_hash)
            .await
            .unwrap();
        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.domain_payload.as_ref(), new_payload);
        assert_eq!(loaded.domain_payload_hash, new_hash);
    }

    /// T-PG-12: Teardown (delete_all_fibers + join_delete_all)
    #[tokio::test]
    async fn test_pg_teardown() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        // Save 3 fibers
        for _ in 0..3 {
            let fid = Uuid::now_v7();
            store.save_fiber(iid, &Fiber::new(fid, 0)).await.unwrap();
        }

        // Save 2 join barriers
        store.join_arrive(iid, 0).await.unwrap();
        store.join_arrive(iid, 1).await.unwrap();

        // delete_all_fibers
        store.delete_all_fibers(iid).await.unwrap();
        let fibers = store
            .load_fibers(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap();
        assert!(fibers.is_empty());

        // join_delete_all
        store.join_delete_all(iid).await.unwrap();
        // Arrive again — should start at 1
        assert_eq!(store.join_arrive(iid, 0).await.unwrap(), 1);
    }

    /// T-PG-13: Concurrent dequeue (SKIP LOCKED)
    #[tokio::test]
    async fn test_pg_concurrent_dequeue() {
        let (_pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let task_type = "concurrent_task".to_string();
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        // Enqueue 3 jobs
        for i in 0..3 {
            store
                .enqueue_job(&JobActivation {
                    job_key: format!("conc-job-{i}"),
                    tenant_id: "default".to_string(),
                    process_instance_id: iid,
                    task_type: task_type.clone(),
                    service_task_id: format!("task-{i}"),
                    domain_payload: "{}".to_string(),
                    domain_payload_hash: [0u8; 32],
                    session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
                    orch_flags: BTreeMap::new(),
                    retries_remaining: 3,
                    entry_id: Uuid::nil(),
                    runbook_id: Uuid::nil(),
                    worker_id: String::new(),
                    claim_token: String::new(),
                    claim_expires_at: None,
                    attempt_count: 0,
                    failure_count: 0,
                    not_before: None,
                })
                .await
                .unwrap();
        }

        // Spawn 3 concurrent dequeue tasks
        let mut handles = Vec::new();
        for _ in 0..3 {
            let s = store.clone();
            let tt = task_type.clone();
            handles.push(tokio::spawn(async move {
                s.dequeue_jobs(&[tt], 1, &TenantId::default(), "test-worker", 300_000)
                    .await
                    .unwrap()
            }));
        }

        let mut all_keys = Vec::new();
        for h in handles {
            let jobs = h.await.unwrap();
            for j in jobs {
                all_keys.push(j.job_key);
            }
        }

        // Exactly 3 jobs, no duplicates
        all_keys.sort();
        all_keys.dedup();
        assert_eq!(all_keys.len(), 3);
    }

    /// Minimal single-task BPMN for T-PG-14.
    const SMOKE_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="smoke_proc" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="do_work" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

    /// T-PG-14: Full engine smoke test
    #[tokio::test]
    async fn test_pg_full_engine_smoke() {
        let (_pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let engine = BpmnLiteEngine::new(store.clone());

        // Compile
        let compiled = engine.compile(SMOKE_BPMN).await.unwrap();
        let version = compiled.bytecode_version;

        // Start process
        let payload = r#"{"case_id":"test-123"}"#;
        let hash = bpmn_lite_vm::compute_hash(payload);
        let instance_id = engine
            .start("smoke_proc", version, payload, hash, "test-corr-1")
            .await
            .unwrap();
        let genesis = store
            .load_snapshot_envelope(&TenantId::new("default").unwrap(), instance_id)
            .await
            .unwrap()
            .unwrap();

        // Tick to advance (produces jobs)
        engine.tick_instance(instance_id).await.unwrap();

        // Get task types from compile result
        let task_types = compiled.task_types;
        assert!(
            !task_types.is_empty(),
            "program should have at least one task"
        );

        // Dequeue job
        let jobs = store
            .dequeue_jobs(&task_types, 1, &TenantId::default(), "test-worker", 300_000)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1, "should have 1 job");

        let job = &jobs[0];

        // Complete job
        let completion_payload = r#"{"result":"ok"}"#;
        engine
            .complete_job(
                &job.job_key,
                completion_payload,
                job.domain_payload_hash,
                BTreeMap::new(),
            )
            .await
            .unwrap();

        // Tick again to advance past the completed job
        engine.tick_instance(instance_id).await.unwrap();

        // Check instance state
        let inst = store
            .load_instance(&TenantId::new("default").unwrap(), instance_id)
            .await
            .unwrap()
            .unwrap();

        // Read events — should have at least InstanceStarted
        let events = store
            .read_events(&TenantId::new("default").unwrap(), instance_id, 1)
            .await
            .unwrap();
        assert!(
            events.len() >= 2,
            "should have multiple events, got {}",
            events.len()
        );

        // First event should be InstanceStarted
        match &events[0].1 {
            RuntimeEvent::InstanceStarted { .. } => {}
            other => panic!("expected InstanceStarted, got {:?}", other),
        }

        // Single-task process should be Completed after completing the one job
        assert!(
            matches!(inst.state, ProcessState::Completed { .. }),
            "expected Completed, got {:?}",
            inst.state
        );

        let artifact = store
            .load_artifact(ArtifactHash::from_bytes(version))
            .await
            .unwrap()
            .unwrap();
        let journal_tail = store
            .read_journal(
                &TenantId::new("default").unwrap(),
                instance_id,
                Some(genesis.revision()),
            )
            .await
            .unwrap();
        let replayed = bpmn_lite_kernel::replay(&artifact, &genesis, &journal_tail).unwrap();
        let committed = store
            .load_snapshot_envelope(&TenantId::new("default").unwrap(), instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            replayed.state_hash().unwrap(),
            committed.state_hash().unwrap()
        );
    }

    /// T-PG-15: cancel_jobs_for_instance
    #[tokio::test]
    async fn test_pg_cancel_jobs_for_instance() {
        let (_pool, store, _lock) = setup().await;
        let task_type = "cancel_test".to_string();

        let iid_a = Uuid::now_v7();
        let iid_b = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid_a))
            .await
            .unwrap();

        let mut inst_b = make_instance(iid_b);
        inst_b.instance_id = iid_b;
        store.save_instance("default", &inst_b).await.unwrap();

        // 2 jobs for instance A, 1 for instance B
        for i in 0..2 {
            store
                .enqueue_job(&JobActivation {
                    job_key: format!("cancel-a-{i}"),
                    tenant_id: "default".to_string(),
                    process_instance_id: iid_a,
                    task_type: task_type.clone(),
                    service_task_id: format!("task-a-{i}"),
                    domain_payload: "{}".to_string(),
                    domain_payload_hash: [0u8; 32],
                    session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
                    orch_flags: BTreeMap::new(),
                    retries_remaining: 3,
                    entry_id: Uuid::nil(),
                    runbook_id: Uuid::nil(),
                    worker_id: String::new(),
                    claim_token: String::new(),
                    claim_expires_at: None,
                    attempt_count: 0,
                    failure_count: 0,
                    not_before: None,
                })
                .await
                .unwrap();
        }
        store
            .enqueue_job(&JobActivation {
                job_key: "cancel-b-0".to_string(),
                tenant_id: "default".to_string(),
                process_instance_id: iid_b,
                task_type: task_type.clone(),
                service_task_id: "task-b-0".to_string(),
                domain_payload: "{}".to_string(),
                domain_payload_hash: [0u8; 32],
                session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
                orch_flags: BTreeMap::new(),
                retries_remaining: 3,
                entry_id: Uuid::nil(),
                runbook_id: Uuid::nil(),
                worker_id: String::new(),
                claim_token: String::new(),
                claim_expires_at: None,
                attempt_count: 0,
                failure_count: 0,
                not_before: None,
            })
            .await
            .unwrap();

        // Cancel instance A's jobs
        let cancelled = store.cancel_jobs_for_instance(iid_a).await.unwrap();
        assert_eq!(cancelled.len(), 2);

        // Dequeue remaining — should only get B's job
        let remaining = store
            .dequeue_jobs(
                &[task_type],
                10,
                &TenantId::default(),
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].job_key, "cancel-b-0");
    }

    // ── A18-Session-1: rows_affected validation tests ──
    //
    // These tests deliberately provoke 0-row write outcomes and assert
    // that the named write methods either error (save_instance, save_fiber,
    // enqueue_job, save_incident) or fall through cleanly (ack_job's
    // soft-signal case).

    /// T-A18-1: enqueue_job against a non-existent parent instance errors.
    /// The job_queue tenant_id is derived via subquery on workflow_instances;
    /// a missing parent yields NULL tenant_id which violates NOT NULL.
    #[tokio::test]
    async fn test_a18_enqueue_job_missing_parent_errors() {
        let (_pool, store, _lock) = setup().await;
        let fake_parent = Uuid::now_v7();

        let activation = JobActivation {
            job_key: "a18-orphan-job".to_string(),
            tenant_id: "default".to_string(),
            process_instance_id: fake_parent,
            task_type: "a18_test".to_string(),
            service_task_id: "a18-task".to_string(),
            domain_payload: "{}".to_string(),
            domain_payload_hash: [0u8; 32],
            session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
            orch_flags: BTreeMap::new(),
            retries_remaining: 3,
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            worker_id: String::new(),
            claim_token: String::new(),
            claim_expires_at: None,
            attempt_count: 0,
            failure_count: 0,
            not_before: None,
        };

        let err = store.enqueue_job(&activation).await.unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("a18-orphan-job") || msg.contains("tenant_id") || msg.contains("violates"),
            "expected enqueue_job error to surface the failure cause, got: {msg}"
        );
    }

    /// T-A18-2: enqueue_job duplicate job_key (idempotent) does NOT error.
    /// `ON CONFLICT DO NOTHING` is benign when the row already exists.
    #[tokio::test]
    async fn test_a18_enqueue_job_duplicate_is_idempotent() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        let activation = JobActivation {
            job_key: "a18-dup-job".to_string(),
            tenant_id: "default".to_string(),
            process_instance_id: iid,
            task_type: "a18_test".to_string(),
            service_task_id: "a18-task".to_string(),
            domain_payload: "{}".to_string(),
            domain_payload_hash: [0u8; 32],
            session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
            orch_flags: BTreeMap::new(),
            retries_remaining: 3,
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            worker_id: String::new(),
            claim_token: String::new(),
            claim_expires_at: None,
            attempt_count: 0,
            failure_count: 0,
            not_before: None,
        };

        // First insert succeeds; second is a benign duplicate.
        store.enqueue_job(&activation).await.unwrap();
        store
            .enqueue_job(&activation)
            .await
            .expect("duplicate enqueue_job must be idempotent, not an error");
    }

    /// T-A18-3: save_incident with a missing parent instance errors.
    #[tokio::test]
    async fn test_a18_save_incident_missing_parent_errors() {
        let (_pool, store, _lock) = setup().await;
        let fake_parent = Uuid::now_v7();

        let incident = Incident {
            incident_id: Uuid::now_v7(),
            process_instance_id: fake_parent,
            fiber_id: Uuid::now_v7(),
            service_task_id: "a18-task".to_string(),
            bytecode_addr: 0.into(),
            error_class: ErrorClass::Transient,
            message: "test".to_string(),
            retry_count: 0,
            created_at: 1700000000000,
            resolved_at: None,
            resolution: None,
        };

        let err = store.save_incident(&incident).await.unwrap_err();
        let msg = format!("{:#}", err);
        // Either our validation error fires, or the FK constraint surfaces.
        assert!(
            msg.contains(&incident.incident_id.to_string())
                || msg.contains("foreign key")
                || msg.contains("violates"),
            "expected save_incident error to mention incident or FK, got: {msg}"
        );
    }

    /// T-A18-4: ack_job for an already-acked job returns Ok (soft signal).
    #[tokio::test]
    async fn test_a18_ack_job_already_acked_is_ok() {
        let (_pool, store, _lock) = setup().await;
        // No setup needed — job_key simply doesn't exist.
        store
            .ack_job("default", "a18-nonexistent-job-key")
            .await
            .expect("ack_job of nonexistent key must be Ok (soft signal)");
    }

    /// T-A18-5: save_instance + save_fiber happy path still works.
    /// Regression guard so rows_affected validation doesn't break the
    /// normal path.
    #[tokio::test]
    async fn test_a18_happy_path_writes_succeed() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let fid = Uuid::now_v7();

        store
            .save_instance("default", &make_instance(iid))
            .await
            .expect("save_instance happy path must succeed");

        let fiber = Fiber::new(fid, 0);
        store
            .save_fiber(iid, &fiber)
            .await
            .expect("save_fiber happy path must succeed");
    }

    // ── A19-Session-1: integrity hash tests ──
    //
    // These tests require BPMN_LITE_TEST_DATABASE_URL and run in the PostgreSQL CI gate.
    // They verify: hash stored at creation; load returns it; tampering surfaces;
    // quarantined instances are skipped by claim_running_instances.

    /// T-A19-PG-1: save_instance stores an integrity hash; load_instance returns it.
    #[tokio::test]
    async fn test_a19_hash_stored_on_save_and_loaded() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            loaded.integrity_hash.is_some(),
            "integrity_hash must be set after save_instance"
        );

        // Verify the hash is correct (matches recomputation).
        use bpmn_lite_types::integrity::compute_instance_integrity_hash;
        assert_eq!(
            loaded.integrity_hash,
            Some(compute_instance_integrity_hash(&loaded))
        );
    }

    /// T-A19-PG-2: hash is NOT updated when save_instance is called again (ON CONFLICT branch).
    #[tokio::test]
    async fn test_a19_hash_not_overwritten_on_update() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store
            .save_instance("test-owner", &make_instance(iid))
            .await
            .unwrap();

        // Claim it
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                iid,
                "test-owner",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        let original_hash = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap()
            .integrity_hash;

        // Re-save (simulates tick updating state/flags).
        let inst = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        store.save_instance("test-owner", &inst).await.unwrap();

        let after_hash = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap()
            .integrity_hash;

        assert_eq!(original_hash, after_hash, "hash must not change on update");
    }

    /// T-A19-PG-3: deliberate DB-level tamper is detected at the mandatory claim boundary.
    #[tokio::test]
    async fn test_a19_tamper_tenant_id_detected() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        // The immutability trigger (migration 029) blocks tenant_id mutation
        // at the DB level — defense in depth above the application integrity
        // check. Verify the trigger fires with a P0001 RAISE EXCEPTION.
        let tamper_result = sqlx::query(
            "UPDATE workflow_instances SET tenant_id = 'evil-tenant' WHERE instance_id = $1",
        )
        .bind(iid)
        .execute(&pool)
        .await;

        let err = tamper_result.expect_err("trigger must reject tenant_id mutation");
        let msg = err.to_string();
        assert!(
            msg.contains("immutable") || msg.contains("P0001"),
            "expected immutability rejection, got: {msg}"
        );
    }

    /// V&S §15 (v0.7) ruling F: 5 automatic-rollback `V2ScopeCancelled`
    /// events for the *same* guard `Addr` (a fresh `RecordId` each time,
    /// mirroring reality — the record retires and is fresh on re-open)
    /// exhaust the built-in budget and quarantine the instance via the
    /// existing `quarantine_state` mechanism. `fiber_id` is included in
    /// `fibers_delete()` on every commit — the signal that distinguishes
    /// ruling C's automatic rollback (`RollbackCaller::Dies`) from an
    /// in-line, explicit `V2CancelScope` (`RollbackCaller::Continues`),
    /// which must NOT count against the budget.
    #[tokio::test]
    async fn test_guard_failure_budget_quarantines_after_repeated_automatic_rollback() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.bytecode_version = store_default_budget_artifact(&store, 5).await;
        store.save_instance("guard-budget-fixture", &instance).await.unwrap();
        let guard_addr = Addr::new(7);

        for _ in 0..5u32 {
            let claim = store
                .claim_instance_for_transition(
                    &TenantId::new("default").unwrap(),
                    instance_id,
                    "apply",
                    30_000,
                )
                .await
                .unwrap()
                .unwrap();
            let record_id = RecordId::new(Uuid::now_v7());
            let fiber_id = Uuid::now_v7();
            let mut record =
                ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
            record.opened_at = Some(guard_addr);
            record.state = RecordState::Retired;
            let transition = TransitionBuilder::new(instance.clone())
                .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
                .delete_fiber(fiber_id)
                .event(RuntimeEvent::V2ScopeCancelled {
                    record_id,
                    fiber_id,
                    cancelled_records: vec![],
                    cancelled_fibers: vec![],
                })
                .build();
            store.commit_transition(&claim, &transition).await.unwrap();
        }

        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            quarantine.as_deref(),
            Some("guard_failure_budget_exhausted"),
            "5 automatic rollbacks of the same guard address must exhaust the built-in budget"
        );
    }

    /// V6.4 (§31.1) fail-closed: a guard cancellation whose pinned artifact is
    /// absent (or pre-canonical) must ABORT the commit with an integrity error,
    /// never silently fall back to a lenient default that could weaken a
    /// stricter declared guard. `make_instance` pins `[0u8; 32]`, and `setup()`
    /// truncates `compiled_programs`, so the tx-scoped budget read finds nothing.
    #[tokio::test]
    async fn test_guard_cancellation_with_missing_artifact_fails_closed() {
        let (_pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("no-artifact", &instance).await.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        let record_id = RecordId::new(Uuid::now_v7());
        let fiber_id = Uuid::now_v7();
        let mut record =
            ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
        record.opened_at = Some(Addr::new(0));
        record.state = RecordState::Retired;
        let transition = TransitionBuilder::new(instance.clone())
            .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
            .delete_fiber(fiber_id)
            .event(RuntimeEvent::V2ScopeCancelled {
                record_id,
                fiber_id,
                cancelled_records: vec![],
                cancelled_fibers: vec![],
            })
            .build();
        let result = store.commit_transition(&claim, &transition).await;
        assert!(
            matches!(result, Err(CommitError::Integrity(_))),
            "a guard cancellation with no pinned artifact must fail closed, got {result:?}"
        );
    }

    /// V6.4 whole-corpus verify gate: every stored artifact verifies
    /// (`Ok(count)`), and a corrupted `canonical_bytes` row is caught by the
    /// gate — fail-closed at cutover, not surfaced lazily only when the
    /// instance is eventually claimed.
    #[tokio::test]
    async fn test_verify_artifact_corpus_catches_a_corrupt_row() {
        let (pool, store, _lock) = setup().await;
        // Two valid artifacts (distinct defaults => distinct canonical bytes
        // => distinct hashes => two rows).
        let _h1 = store_default_budget_artifact(&store, 3).await;
        let good_hash = store_default_budget_artifact(&store, 7).await;

        let count = store.verify_artifact_corpus().await.unwrap();
        assert_eq!(count, 2, "both stored artifacts must verify");

        // Corrupt one row's canonical_bytes; the gate must now fail closed.
        sqlx::query("UPDATE compiled_programs SET canonical_bytes = $1 WHERE bytecode_version = $2")
            .bind(&b"not a valid canonical envelope"[..])
            .bind(&good_hash[..])
            .execute(&pool)
            .await
            .unwrap();
        let result = store.verify_artifact_corpus().await;
        assert!(
            matches!(&result, Err(StoreError::Integrity(message)) if message.contains("corpus verification failed")),
            "a corrupt artifact row must fail the corpus gate closed, got {result:?}"
        );
    }

    /// V6.4 destructive cutover: the shipped `scripts/cutover-wipe.sql` clears
    /// the artifact corpus and all per-instance runtime state — including the
    /// FORCE-RLS V8 `guard_failure_budget` table, transitively via
    /// `TRUNCATE workflow_instances CASCADE` — and the store then cold-starts
    /// ready: the corpus gate passes on the empty corpus and a freshly stored
    /// artifact + instance is immediately claimable.
    #[tokio::test]
    async fn test_cutover_wipe_clears_runtime_state_and_store_cold_starts() {
        let (pool, store, _lock) = setup().await;

        // Seed: an artifact, an instance pinned to it, and — on a tenant-scoped
        // connection, since guard_failure_budget FORCEs RLS — a budget row.
        let hash = store_default_budget_artifact(&store, 5).await;
        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.bytecode_version = hash;
        store.save_instance("cutover-seed", &instance).await.unwrap();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.current_tenant', 'default', false)")
            .execute(conn.as_mut())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO guard_failure_budget (tenant_id, instance_id, guard_addr, failure_count, updated_at) \
             VALUES ('default', $1, 0, 1, now())",
        )
        .bind(instance_id)
        .execute(conn.as_mut())
        .await
        .unwrap();
        let pre_budget: i64 = sqlx::query_scalar("SELECT count(*) FROM guard_failure_budget")
            .fetch_one(conn.as_mut())
            .await
            .unwrap();
        assert_eq!(pre_budget, 1, "seed must create a guard_failure_budget row");
        assert_eq!(
            store.verify_artifact_corpus().await.unwrap(),
            1,
            "seed must store one artifact"
        );

        // Execute the shipped cutover wipe script statement-by-statement.
        let sql = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/cutover-wipe.sql"
        ))
        .unwrap();
        for statement in sql.split(';') {
            let trimmed = statement.trim();
            if trimmed.is_empty()
                || trimmed
                    .lines()
                    .all(|line| line.trim().is_empty() || line.trim_start().starts_with("--"))
            {
                continue;
            }
            sqlx::query(trimmed).execute(&pool).await.unwrap();
        }

        // Runtime state and corpus cleared; guard_failure_budget cascaded away.
        let post_budget: i64 = sqlx::query_scalar("SELECT count(*) FROM guard_failure_budget")
            .fetch_one(conn.as_mut())
            .await
            .unwrap();
        assert_eq!(
            post_budget, 0,
            "TRUNCATE workflow_instances CASCADE must clear guard_failure_budget"
        );
        assert_eq!(
            store.verify_artifact_corpus().await.unwrap(),
            0,
            "the artifact corpus must be empty and verify as ready after the wipe"
        );

        // Cold start: a freshly stored artifact + instance is immediately claimable.
        let fresh_hash = store_default_budget_artifact(&store, 4).await;
        let fresh_id = Uuid::now_v7();
        let mut fresh = make_instance(fresh_id);
        fresh.bytecode_version = fresh_hash;
        store.save_instance("cold-start", &fresh).await.unwrap();
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                fresh_id,
                "apply",
                30_000,
            )
            .await
            .unwrap();
        assert!(
            claim.is_some(),
            "a fresh instance must be claimable on the cold-started store"
        );
    }

    /// V8 (§31): the escalation ceiling is read from the instance's pinned
    /// artifact, not a hardcoded constant. An artifact whose workflow-level
    /// default budget is 2 must quarantine after *2* rollbacks — proving the
    /// retired `ScopeFailureBudget::new(1, 5)` placeholder is gone.
    #[tokio::test]
    async fn test_guard_failure_budget_ceiling_comes_from_the_artifact() {
        let (pool, store, _lock) = setup().await;
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [0u8; 32],
            program: vec![Instr::End],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        .with_v2_guard_budgets(
            BTreeMap::new(),
            bpmn_lite_types::ScopeFailureBudget::new(1, 2).unwrap(),
        );
        let workflow = bpmn_lite_types::ExecutableWorkflow::from_verified_envelope(
            bpmn_lite_types::ArtifactEnvelope::from_legacy_program(program, "v8-budget-2").unwrap(),
        )
        .unwrap();
        store.store_artifact(&workflow).await.unwrap();

        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.bytecode_version = workflow.hash().into_bytes();
        store.save_instance("guard-budget-artifact", &instance).await.unwrap();
        let guard_addr = Addr::new(7);

        let mut quarantine_after = None;
        for attempt in 1..=2u32 {
            let claim = store
                .claim_instance_for_transition(
                    &TenantId::new("default").unwrap(),
                    instance_id,
                    "apply",
                    30_000,
                )
                .await
                .unwrap()
                .unwrap();
            let record_id = RecordId::new(Uuid::now_v7());
            let fiber_id = Uuid::now_v7();
            let mut record =
                ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
            record.opened_at = Some(guard_addr);
            record.state = RecordState::Retired;
            let transition = TransitionBuilder::new(instance.clone())
                .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
                .delete_fiber(fiber_id)
                .event(RuntimeEvent::V2ScopeCancelled {
                    record_id,
                    fiber_id,
                    cancelled_records: vec![],
                    cancelled_fibers: vec![],
                })
                .build();
            store.commit_transition(&claim, &transition).await.unwrap();

            let quarantine: Option<String> = sqlx::query_scalar(
                "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
            )
            .bind(instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            if quarantine.as_deref() == Some("guard_failure_budget_exhausted") {
                quarantine_after = Some(attempt);
                break;
            }
        }
        assert_eq!(
            quarantine_after,
            Some(2),
            "the artifact's default budget of 2 must quarantine after exactly 2 rollbacks, \
             not the retired hardcoded 5"
        );
    }

    /// V&S §31 per-guard store read: a NON-empty `v2_guard_budgets` entry for
    /// the cancelling guard's address overrides the workflow default — the
    /// headline "per-guard, not just default" claim, proven at the store (the
    /// sibling test above only exercises the `unwrap_or(default)` fallback).
    /// A strict per-guard budget of 1 must quarantine after ONE rollback even
    /// though the workflow default is a lenient 9.
    #[tokio::test]
    async fn test_per_guard_budget_entry_overrides_the_workflow_default() {
        let (pool, store, _lock) = setup().await;
        // Address 0 is a real guard-open in this minimal verifying program, so
        // the V8.3 admission gate (budget key must resolve to a guard-opener)
        // accepts the non-empty table — mirrors v2_verifier's minimal guard.
        let guard_addr = Addr::new(0);
        let mut budgets = BTreeMap::new();
        budgets.insert(guard_addr, bpmn_lite_types::ScopeFailureBudget::new(1, 1).unwrap());
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [0u8; 32],
            program: vec![
                Instr::V2Guard { handler: Addr::new(3) },
                Instr::V2GuardEnd,
                Instr::End,
                Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        .with_v2_guard_budgets(
            budgets,
            // Lenient workflow default — the per-guard entry must win over it.
            bpmn_lite_types::ScopeFailureBudget::new(1, 9).unwrap(),
        );
        let workflow = bpmn_lite_types::ExecutableWorkflow::from_verified_envelope(
            bpmn_lite_types::ArtifactEnvelope::from_legacy_program(program, "v8-per-guard-1").unwrap(),
        )
        .unwrap();
        store.store_artifact(&workflow).await.unwrap();

        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.bytecode_version = workflow.hash().into_bytes();
        store.save_instance("per-guard-budget", &instance).await.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        let record_id = RecordId::new(Uuid::now_v7());
        let fiber_id = Uuid::now_v7();
        let mut record =
            ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
        record.opened_at = Some(guard_addr);
        record.state = RecordState::Retired;
        let transition = TransitionBuilder::new(instance.clone())
            .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
            .delete_fiber(fiber_id)
            .event(RuntimeEvent::V2ScopeCancelled {
                record_id,
                fiber_id,
                cancelled_records: vec![],
                cancelled_fibers: vec![],
            })
            .build();
        store.commit_transition(&claim, &transition).await.unwrap();

        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            quarantine.as_deref(),
            Some("guard_failure_budget_exhausted"),
            "the per-guard budget of 1 must quarantine after ONE rollback, \
             overriding the lenient workflow default of 9"
        );
    }

    /// V&S §15 (v0.7) ruling F: an explicit, in-line `V2CancelScope`
    /// (the triggering fibre survives — not in `fibers_delete()`) must
    /// never count against the budget, however many times it fires.
    #[tokio::test]
    async fn test_guard_failure_budget_ignores_explicit_cancel_scope() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.bytecode_version = store_default_budget_artifact(&store, 5).await;
        store.save_instance("guard-budget-fixture", &instance).await.unwrap();
        let guard_addr = Addr::new(9);

        for _ in 0..10u32 {
            let claim = store
                .claim_instance_for_transition(
                    &TenantId::new("default").unwrap(),
                    instance_id,
                    "apply",
                    30_000,
                )
                .await
                .unwrap()
                .unwrap();
            let record_id = RecordId::new(Uuid::now_v7());
            let surviving_fiber_id = Uuid::now_v7();
            let mut record =
                ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
            record.opened_at = Some(guard_addr);
            record.state = RecordState::Retired;
            let transition = TransitionBuilder::new(instance.clone())
                .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
                // No `.delete_fiber(surviving_fiber_id)` — the cancelling
                // fibre continues in place (`RollbackCaller::Continues`).
                .event(RuntimeEvent::V2ScopeCancelled {
                    record_id,
                    fiber_id: surviving_fiber_id,
                    cancelled_records: vec![],
                    cancelled_fibers: vec![],
                })
                .build();
            store.commit_transition(&claim, &transition).await.unwrap();
        }

        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            quarantine, None,
            "10 explicit V2CancelScope firings must never quarantine — only automatic rollback counts"
        );
    }

    /// V&S §15 (v0.7) ruling F: a successful guard close (`V2GuardRetired`)
    /// resets the budget — 4 automatic-rollback failures (under the
    /// built-in 5-failure budget), a successful close, then 4 more
    /// failures must NOT quarantine, proving the counter actually reset
    /// rather than merely not yet having reached 8.
    #[tokio::test]
    async fn test_guard_failure_budget_resets_on_successful_close() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.bytecode_version = store_default_budget_artifact(&store, 5).await;
        store.save_instance("guard-budget-fixture", &instance).await.unwrap();
        let guard_addr = Addr::new(11);

        async fn fail_once(
            store: &PostgresWorkflowStore,
            instance: &ProcessInstance,
            instance_id: Uuid,
            guard_addr: Addr,
        ) {
            let claim = store
                .claim_instance_for_transition(
                    &TenantId::new("default").unwrap(),
                    instance_id,
                    "apply",
                    30_000,
                )
                .await
                .unwrap()
                .unwrap();
            let record_id = RecordId::new(Uuid::now_v7());
            let fiber_id = Uuid::now_v7();
            let mut record =
                ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
            record.opened_at = Some(guard_addr);
            record.state = RecordState::Retired;
            let transition = TransitionBuilder::new(instance.clone())
                .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
                .delete_fiber(fiber_id)
                .event(RuntimeEvent::V2ScopeCancelled {
                    record_id,
                    fiber_id,
                    cancelled_records: vec![],
                    cancelled_fibers: vec![],
                })
                .build();
            store.commit_transition(&claim, &transition).await.unwrap();
        }

        for _ in 0..4u32 {
            fail_once(&store, &instance, instance_id, guard_addr).await;
        }

        // A successful close resets the counter.
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        let record_id = RecordId::new(Uuid::now_v7());
        let fiber_id = Uuid::now_v7();
        let mut record = ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true });
        record.opened_at = Some(guard_addr);
        record.state = RecordState::Retired;
        let transition = TransitionBuilder::new(instance.clone())
            .concurrency_mutation(ConcurrencyMutation::Insert(Box::new(record)))
            .event(RuntimeEvent::V2GuardRetired { record_id, fiber_id })
            .build();
        store.commit_transition(&claim, &transition).await.unwrap();

        for _ in 0..4u32 {
            fail_once(&store, &instance, instance_id, guard_addr).await;
        }

        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            quarantine, None,
            "4 + 4 failures with a successful close in between must not quarantine — the close must have reset the counter, not merely paused it short of 8"
        );
    }

    /// T-A19-PG-4: quarantine_instance marks the row and logs an event.
    #[tokio::test]
    async fn test_a19_quarantine_marks_row_and_logs_event() {
        use sqlx::Row;
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();
        store
            .quarantine_instance(iid, "default", "default", "grpc_handler")
            .await
            .expect("quarantine_instance must succeed");

        // Check quarantine_state column.
        let row =
            sqlx::query("SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1")
                .bind(iid)
                .fetch_one(&pool)
                .await
                .unwrap();
        let state: Option<String> = row.get("quarantine_state");
        assert_eq!(
            state.as_deref(),
            Some("integrity_violation"),
            "quarantine_state must be 'integrity_violation'"
        );

        // Check InstanceQuarantined event was appended.
        let events = store
            .read_events(&TenantId::new("default").unwrap(), iid, 0)
            .await
            .unwrap();
        let has_quarantine_event = events.iter().any(|(_, ev)| {
            matches!(
                ev,
                bpmn_lite_types::events::RuntimeEvent::InstanceQuarantined { .. }
            )
        });
        assert!(
            has_quarantine_event,
            "InstanceQuarantined event must be logged"
        );
    }

    /// T-A19-PG-5: quarantined instance is skipped by claim_running_instances.
    #[tokio::test]
    async fn test_a19_quarantined_instance_skipped_by_scheduler() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        // Quarantine the instance.
        store
            .quarantine_instance(iid, "default", "default", "scheduler_claim")
            .await
            .unwrap();

        // Claim batch — quarantined instance should not be returned.
        let claimed = store
            .claim_running_instances(
                &TenantId::new("default").unwrap(),
                "test-scheduler",
                10,
                5_000,
            )
            .await
            .unwrap();
        assert!(
            !claimed.contains(&iid),
            "quarantined instance must not appear in scheduler claim"
        );
    }

    // ── L0 — Pool schema tests ──────────────────────────────────────────────

    /// T-L0-PG-1: default pool row is present after migrations.
    #[tokio::test]
    async fn test_l0_default_pool_exists() {
        let (pool, _store, _lock) = setup().await;
        let row: (String,) =
            sqlx::query_as("SELECT pool_id FROM tenant_pools WHERE pool_id = 'default'")
                .fetch_one(&pool)
                .await
                .expect("default pool row must exist after migration 032");
        assert_eq!(row.0, "default");
    }

    /// T-L0-PG-2: ensure_tenant assigns pool_id = 'default'.
    #[tokio::test]
    async fn test_l0_ensure_tenant_sets_pool_id() {
        let (pool, store, _lock) = setup().await;
        store
            .ensure_tenant(&TenantId::new("l0_test_tenant").unwrap())
            .await
            .unwrap();
        let row: (String,) =
            sqlx::query_as("SELECT pool_id FROM tenants WHERE tenant_id = 'l0_test_tenant'")
                .fetch_one(&pool)
                .await
                .expect("tenant row must exist");
        assert_eq!(row.0, "default");
    }

    /// T-L0-PG-3: list_tenants_in_pool returns only tenants in that pool.
    #[tokio::test]
    async fn test_l0_list_tenants_in_pool() {
        let (_pool, store, _lock) = setup().await;
        store
            .ensure_tenant(&TenantId::new("l0_pool_tenant_a").unwrap())
            .await
            .unwrap();
        store
            .ensure_tenant(&TenantId::new("l0_pool_tenant_b").unwrap())
            .await
            .unwrap();

        let in_default = store.list_tenants_in_pool("default").await.unwrap();
        assert!(
            in_default.contains(&"l0_pool_tenant_a".to_string()),
            "l0_pool_tenant_a must be in default pool"
        );
        assert!(
            in_default.contains(&"l0_pool_tenant_b".to_string()),
            "l0_pool_tenant_b must be in default pool"
        );

        let in_nonexistent = store.list_tenants_in_pool("does_not_exist").await.unwrap();
        assert!(
            in_nonexistent.is_empty(),
            "unknown pool must return empty vec"
        );
    }

    /// T-L0-PG-4: FK constraint prevents assigning a tenant to a nonexistent pool.
    #[tokio::test]
    async fn test_l0_fk_rejects_unknown_pool() {
        let (pool, _store, _lock) = setup().await;
        let result = sqlx::query(
            "INSERT INTO tenants (tenant_id, pool_id) VALUES ('fk_test_tenant', 'nonexistent_pool')",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "FK constraint must reject unknown pool_id");
    }

    /// E-invariant T1.1: Verify RLS mutations fail without tenant context or with wrong tenant context.
    #[tokio::test]
    async fn test_t1_1_rls_mutations_fail_without_tenant_context() {
        let (admin_pool, admin_store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let template_id = *blake3::hash(iid.as_bytes()).as_bytes();
        let tenant_id = "tenant-t1-1";

        // 1. Insert a process instance as superuser (bypassing RLS)
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        admin_store.save_instance("default", &inst).await.unwrap();

        // 2. Connect to the database as the non-superuser bpmn_lite_app
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
        let app_url = if url.contains("@") {
            let parts: Vec<&str> = url.split('@').collect();
            let host_part = parts[1];
            format!(
                "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@{}",
                host_part
            )
        } else {
            "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@localhost/bpmn_lite_test"
                .to_string()
        };

        let app_pool = PgPool::connect(&app_url)
            .await
            .expect("Failed to connect as bpmn_lite_app");
        let app_store = PostgresWorkflowStore::new(app_pool.clone());

        // Seed data in the other 4 tables as admin (superuser) under tenant-t1-1
        sqlx::query(
            "INSERT INTO job_queue (job_key, tenant_id, process_instance_id, task_type, service_task_id, domain_payload, domain_payload_hash) VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind("job-t1-1")
        .bind(tenant_id)
        .bind(iid)
        .bind("task")
        .bind("service")
        .bind("payload")
        .bind(b"hash-12345678-hash-12345678-hash") // 32 bytes
        .execute(&admin_pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO ffi_template (template_id, template_uuidv7, owner_type, owner_metadata, input_schema_json, output_schema_json, idempotency_json, tenant_id, publisher) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        )
        .bind(template_id.as_slice())
        .bind(Uuid::now_v7())
        .bind("owner")
        .bind(b"metadata")
        .bind(serde_json::json!([]))
        .bind(serde_json::json!([]))
        .bind(serde_json::json!("Idempotent"))
        .bind(tenant_id)
        .bind("publisher")
        .execute(&admin_pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO ffi_invocation_record (invocation_id, caller_process_instance_id, caller_task_id, caller_pc, template_id, owner_type, tenant_id, invoked_at, input_payload, outcome_kind) VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8, $9)"
        )
        .bind(Uuid::now_v7())
        .bind(iid)
        .bind("task")
        .bind(0)
        .bind(template_id.as_slice())
        .bind("owner")
        .bind(tenant_id)
        .bind(b"input")
        .bind("pending")
        .execute(&admin_pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO incidents (incident_id, tenant_id, process_instance_id, fiber_id, service_task_id, bytecode_addr, error_class, message) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(iid)
        .bind(Uuid::now_v7())
        .bind("task")
        .bind(0)
        .bind(serde_json::json!("MemoryLimit"))
        .bind("out of memory")
        .execute(&admin_pool)
        .await
        .unwrap();

        // 3. Verify fail-closed without context (direct queries return zero rows for all 5 RLS tables)
        let row_pi: Option<(Uuid,)> =
            sqlx::query_as("SELECT instance_id FROM workflow_instances WHERE instance_id = $1")
                .bind(iid)
                .fetch_optional(&app_pool)
                .await
                .unwrap();
        assert!(
            row_pi.is_none(),
            "workflow_instances query without tenant context must return zero rows"
        );

        let row_jq: Option<(String,)> =
            sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = 'job-t1-1'")
                .fetch_optional(&app_pool)
                .await
                .unwrap();
        assert!(
            row_jq.is_none(),
            "job_queue query without tenant context must return zero rows"
        );

        let row_tmpl: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT template_id FROM ffi_template WHERE template_id = $1")
                .bind(template_id.as_slice())
                .fetch_optional(&app_pool)
                .await
                .unwrap();
        assert!(
            row_tmpl.is_none(),
            "ffi_template query without tenant context must return zero rows"
        );

        let row_invoc: Option<(Uuid,)> = sqlx::query_as(
            "SELECT invocation_id FROM ffi_invocation_record WHERE caller_process_instance_id = $1",
        )
        .bind(iid)
        .fetch_optional(&app_pool)
        .await
        .unwrap();
        assert!(
            row_invoc.is_none(),
            "ffi_invocation_record query without tenant context must return zero rows"
        );

        let row_inc: Option<(Uuid,)> =
            sqlx::query_as("SELECT incident_id FROM incidents WHERE process_instance_id = $1")
                .bind(iid)
                .fetch_optional(&app_pool)
                .await
                .unwrap();
        assert!(
            row_inc.is_none(),
            "incidents query without tenant context must return zero rows"
        );

        // Verify UPDATE fails closed without context
        let update_res = sqlx::query(
            "UPDATE workflow_instances SET state = '\"Completed\"'::jsonb WHERE instance_id = $1",
        )
        .bind(iid)
        .execute(&app_pool)
        .await
        .unwrap();
        assert_eq!(
            update_res.rows_affected(),
            0,
            "Update without tenant context must affect zero rows"
        );

        // 4. Verify cross-tenant blocked / wrong tenant context (queries return zero rows)
        let mut tx = app_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL app.current_tenant = 'evil-tenant'")
            .execute(&mut *tx)
            .await
            .unwrap();

        let row_pi: Option<(Uuid,)> =
            sqlx::query_as("SELECT instance_id FROM workflow_instances WHERE instance_id = $1")
                .bind(iid)
                .fetch_optional(&mut *tx)
                .await
                .unwrap();
        assert!(
            row_pi.is_none(),
            "workflow_instances query with wrong tenant context must return zero rows"
        );

        let row_jq: Option<(String,)> =
            sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = 'job-t1-1'")
                .fetch_optional(&mut *tx)
                .await
                .unwrap();
        assert!(
            row_jq.is_none(),
            "job_queue query with wrong tenant context must return zero rows"
        );

        let row_tmpl: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT template_id FROM ffi_template WHERE template_id = $1")
                .bind(template_id.as_slice())
                .fetch_optional(&mut *tx)
                .await
                .unwrap();
        assert!(
            row_tmpl.is_none(),
            "ffi_template query with wrong tenant context must return zero rows"
        );

        let row_invoc: Option<(Uuid,)> = sqlx::query_as(
            "SELECT invocation_id FROM ffi_invocation_record WHERE caller_process_instance_id = $1",
        )
        .bind(iid)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
        assert!(
            row_invoc.is_none(),
            "ffi_invocation_record query with wrong tenant context must return zero rows"
        );

        let row_inc: Option<(Uuid,)> =
            sqlx::query_as("SELECT incident_id FROM incidents WHERE process_instance_id = $1")
                .bind(iid)
                .fetch_optional(&mut *tx)
                .await
                .unwrap();
        assert!(
            row_inc.is_none(),
            "incidents query with wrong tenant context must return zero rows"
        );

        tx.rollback().await.unwrap();

        // 5. Test Site B: quarantine_instance with WRONG tenant context
        // Should return Err because workflow_instances UPDATE affects 0 rows due to RLS.
        let quar_res = app_store
            .quarantine_instance(iid, "evil-tenant", "default", "test_t1_1")
            .await;
        assert!(
            quar_res.is_err(),
            "quarantine_instance must fail with incorrect tenant context"
        );

        // 5. Test Site A: atomic_consume_buffered_message with WRONG tenant context
        // We first need a claimed message in the buffer.
        let msg_id = format!("msg-t1-1-{}", Uuid::now_v7());
        let message_name = "test-message";
        let correlation_key = "corr-t1-1";

        let claim_token = Uuid::now_v7();
        let claim_until_ms =
            (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp_millis();
        let claim_until_dt = epoch_ms_to_datetime(claim_until_ms);
        sqlx::query(
            r#"
            INSERT INTO message_buffer (
                tenant_id, message_name, correlation_key, msg_id, payload,
                payload_hash, expires_at, process_instance_id, claim_token, claim_until, status
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'claimed')
            "#,
        )
        .bind(tenant_id)
        .bind(message_name)
        .bind(correlation_key)
        .bind(&msg_id)
        .bind(b"payload".to_vec())
        .bind(None::<Vec<u8>>)
        .bind(chrono::Utc::now() + chrono::Duration::seconds(60))
        .bind(iid)
        .bind(claim_token)
        .bind(claim_until_dt)
        .execute(&admin_pool)
        .await
        .unwrap();

        let claimed_msg = ClaimedBufferedMessage {
            message: BufferedMessage {
                tenant_id: tenant_id.to_string(), // correct tenant_id so message_buffer update succeeds
                message_name: message_name.to_string(),
                correlation_key: correlation_key.to_string(),
                msg_id: msg_id.to_string(),
                payload: b"payload".to_vec(),
                payload_hash: None,
                received_at: 0,
                expires_at: 0,
                process_instance_id: Some(iid),
            },
            claim_token: claim_token.to_string(),
            claim_until: claim_until_ms,
        };

        let fiber = Fiber::new(Uuid::now_v7(), 0);
        let mut evil_inst = inst.clone();
        evil_inst.tenant_id = "evil-tenant".to_string(); // mismatched

        let consume_res = app_store
            .atomic_consume_buffered_message(&evil_inst, &fiber, &claimed_msg, None, &[])
            .await;

        assert!(
            consume_res.is_err(),
            "atomic_consume_buffered_message must fail (return Err) with incorrect tenant context"
        );
    }

    /// E-invariant I2: Verify distinct cross-tenant read/write isolation under bpmn_lite_app role.
    #[tokio::test]
    async fn test_t1_1_rls_cross_tenant_isolation() {
        let (admin_pool, _admin_store, _lock) = setup().await;
        let tenant_a = "tenant-A";
        let tenant_b = "tenant-B";

        let iid_a = Uuid::now_v7();
        let iid_b = Uuid::now_v7();

        // 1. Admin setup: Insert process instances for A and B
        let mut inst_a = make_instance(iid_a);
        inst_a.tenant_id = tenant_a.to_string();
        inst_a.process_key = "process-A".to_string();
        let admin_store = PostgresWorkflowStore::new(admin_pool.clone());
        admin_store.save_instance("unused", &inst_a).await.unwrap();

        let mut inst_b = make_instance(iid_b);
        inst_b.tenant_id = tenant_b.to_string();
        inst_b.process_key = "process-B".to_string();
        admin_store.save_instance("unused", &inst_b).await.unwrap();

        // Admin inserts child rows for tenant-B
        let fib_id_b = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
            VALUES ($1, $2, 0, '[]'::jsonb, '{}'::jsonb, 'null'::jsonb, 0, $3)
            "#
        )
        .bind(iid_b)
        .bind(fib_id_b)
        .bind(tenant_b)
        .execute(&admin_pool)
        .await
        .unwrap();

        let timer_id_b = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO workflow_timers (
                tenant_id, timer_id, instance_id, fiber_id, due_at, kind, state
            ) VALUES ($1, $2, $3, $4, now(), '"Wait"'::jsonb, 'armed')
            "#,
        )
        .bind(tenant_b)
        .bind(timer_id_b)
        .bind(iid_b)
        .bind(fib_id_b)
        .execute(&admin_pool)
        .await
        .unwrap();

        let call_id_b = Uuid::now_v7();
        let idem_key_b = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO bpmn_pending_invocation (
                callout_id, process_instance_id, node_id, target_domain, verb_id,
                idempotency_key, execution_id, tenant_id
            )
            VALUES ($1, $2, 'node-1', 'domain', 'verb', $3, NULL, $4)
            "#,
        )
        .bind(call_id_b)
        .bind(iid_b)
        .bind(idem_key_b)
        .bind(tenant_b)
        .execute(&admin_pool)
        .await
        .unwrap();

        let msg_id_b = format!("msg-b-{}", Uuid::now_v7());
        sqlx::query(
            r#"
            INSERT INTO message_buffer (
                tenant_id, message_name, correlation_key, msg_id, payload,
                expires_at, status
            ) VALUES ($1, 'msg-b', 'corr-b', $2, 'payload'::bytea, now() + interval '1 hour', 'buffered')
            "#
        )
        .bind(tenant_b)
        .bind(&msg_id_b)
        .execute(&admin_pool)
        .await
        .unwrap();

        // 2. Connect as bpmn_lite_app non-superuser
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
        let app_url = if url.contains("@") {
            let parts: Vec<&str> = url.split('@').collect();
            let host_part = parts[1];
            format!(
                "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@{}",
                host_part
            )
        } else {
            "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@localhost/bpmn_lite_test"
                .to_string()
        };

        let app_pool = PgPool::connect(&app_url)
            .await
            .expect("Failed to connect as bpmn_lite_app");

        // 3. Non-vacuity check: admin/no-context view sees B's rows
        let admin_count: (i64,) =
            sqlx::query_as("SELECT count(*) FROM workflow_instances WHERE instance_id IN ($1, $2)")
                .bind(iid_a)
                .bind(iid_b)
                .fetch_one(&admin_pool)
                .await
                .unwrap();
        assert_eq!(
            admin_count.0, 2,
            "Admin connection must see both tenant rows (non-vacuous)"
        );

        // 4. Read isolation: with app.current_tenant = tenant_a, query A only sees A
        let mut tx = app_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL app.current_tenant = 'tenant-A'")
            .execute(&mut *tx)
            .await
            .unwrap();

        let visible_rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT instance_id, tenant_id FROM workflow_instances WHERE instance_id IN ($1, $2)",
        )
        .bind(iid_a)
        .bind(iid_b)
        .fetch_all(&mut *tx)
        .await
        .unwrap();

        assert_eq!(
            visible_rows.len(),
            1,
            "Tenant A context must only see 1 row"
        );
        assert_eq!(
            visible_rows[0].0, iid_a,
            "Visible row must belong to tenant A"
        );
        assert_eq!(
            visible_rows[0].1, "tenant-A",
            "Visible row must have tenant-A ID"
        );

        // Assert Tenant A cannot read Tenant B's child rows
        let visible_fibers: Vec<(Uuid,)> =
            sqlx::query_as("SELECT fiber_id FROM fibers WHERE instance_id = $1")
                .bind(iid_b)
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert!(
            visible_fibers.is_empty(),
            "Tenant A context must not see Tenant B fibers"
        );

        let visible_calls: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT callout_id FROM bpmn_pending_invocation WHERE process_instance_id = $1",
        )
        .bind(iid_b)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        assert!(
            visible_calls.is_empty(),
            "Tenant A context must not see Tenant B pending invocations"
        );

        let visible_msgs: Vec<(String,)> =
            sqlx::query_as("SELECT msg_id FROM message_buffer WHERE msg_id = $1")
                .bind(&msg_id_b)
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert!(
            visible_msgs.is_empty(),
            "Tenant A context must not see Tenant B message buffer rows"
        );

        let visible_timers: Vec<(Uuid,)> =
            sqlx::query_as("SELECT timer_id FROM workflow_timers WHERE timer_id = $1")
                .bind(timer_id_b)
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert!(
            visible_timers.is_empty(),
            "Tenant A context must not see Tenant B timers"
        );

        // 5. Write isolation: with app.current_tenant = tenant_a, update/delete B affects 0 rows
        let update_res = sqlx::query(
            "UPDATE workflow_instances SET state = '\"Completed\"'::jsonb WHERE instance_id = $1",
        )
        .bind(iid_b)
        .execute(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            update_res.rows_affected(),
            0,
            "Update on Tenant B row under Tenant A context must affect 0 rows"
        );

        let delete_res = sqlx::query("DELETE FROM workflow_instances WHERE instance_id = $1")
            .bind(iid_b)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            delete_res.rows_affected(),
            0,
            "Delete on Tenant B row under Tenant A context must affect 0 rows"
        );

        let update_msg_res =
            sqlx::query("UPDATE message_buffer SET status = 'consumed' WHERE msg_id = $1")
                .bind(&msg_id_b)
                .execute(&mut *tx)
                .await
                .unwrap();
        assert_eq!(
            update_msg_res.rows_affected(),
            0,
            "Update on Tenant B message_buffer row under Tenant A context must affect 0 rows"
        );

        let update_timer_res =
            sqlx::query("UPDATE workflow_timers SET state = 'cancelled' WHERE timer_id = $1")
                .bind(timer_id_b)
                .execute(&mut *tx)
                .await
                .unwrap();
        assert_eq!(
            update_timer_res.rows_affected(),
            0,
            "Update on Tenant B timer under Tenant A context must affect 0 rows"
        );

        // Assert writes with Tenant B tenant_id under Tenant A context are rejected by WITH CHECK
        let write_fib_res = sqlx::query(
            r#"
            INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
            VALUES ($1, $2, 0, '[]'::jsonb, '{}'::jsonb, 'null'::jsonb, 0, $3)
            "#
        )
        .bind(iid_b)
        .bind(Uuid::now_v7())
        .bind(tenant_b)
        .execute(&mut *tx)
        .await;
        assert!(
            write_fib_res.is_err(),
            "Write to fibers with tenant-B ID under tenant-A context must fail WITH CHECK"
        );

        let write_call_res = sqlx::query(
            r#"
            INSERT INTO bpmn_pending_invocation (
                callout_id, process_instance_id, node_id, target_domain, verb_id,
                idempotency_key, execution_id, tenant_id
            )
            VALUES ($1, $2, 'node-1', 'domain', 'verb', $3, NULL, $4)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(iid_b)
        .bind(Uuid::now_v7())
        .bind(tenant_b)
        .execute(&mut *tx)
        .await;
        assert!(
            write_call_res.is_err(),
            "Write to bpmn_pending_invocation with tenant-B ID under tenant-A context must fail WITH CHECK"
        );

        let write_msg_res = sqlx::query(
            r#"
            INSERT INTO message_buffer (
                tenant_id, message_name, correlation_key, msg_id, payload,
                expires_at, status
            ) VALUES ($1, 'msg-b-new', 'corr-b', $2, 'payload'::bytea, now() + interval '1 hour', 'buffered')
            "#
        )
        .bind(tenant_b)
        .bind(format!("msg-b-new-{}", Uuid::now_v7()))
        .execute(&mut *tx)
        .await;
        assert!(
            write_msg_res.is_err(),
            "Write to message_buffer with tenant-B ID under tenant-A context must fail WITH CHECK"
        );

        tx.rollback().await.unwrap();
    }

    /// RISK-009: Lease fencing re-enabled. A worker with the wrong lease owner is rejected.
    #[tokio::test]
    async fn test_risk_009_lease_fence_rejection() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // 1. Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("owner-a", &inst).await.unwrap();

        // 2. Claim it under owner-a
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-a",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        // 3. Force lease expiry in DB
        sqlx::query("UPDATE workflow_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // 4. Owner-b claims the expired lease
        let claimed_b = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-b",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed_b.is_some());

        // 5. Stale owner-a tries to write -> must be rejected (returns error because rows_affected == 0 in update_instance_state)
        let res_stale = store
            .update_instance_state(
                tenant_id,
                "owner-a",
                iid,
                ProcessState::Completed { at: 222 },
            )
            .await;
        assert!(
            res_stale.is_err(),
            "Stale owner-a write must fail the fence"
        );

        // 6. Current owner-b tries to write -> succeeds
        let res_current = store
            .update_instance_state(
                tenant_id,
                "owner-b",
                iid,
                ProcessState::Completed { at: 333 },
            )
            .await;
        assert!(res_current.is_ok(), "Current owner-b write must succeed");
    }

    /// Regression healed correctly: bus_runtime::advance and detect_interrupted_ffi_calls write successfully by claiming the lease.
    #[tokio::test]
    async fn test_regression_healed_by_claim() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // 1. Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("scheduler", &inst).await.unwrap();

        // Simulating park-release:
        sqlx::query("UPDATE workflow_instances SET lease_owner = NULL, lease_until = NULL WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // 2. A resumer (e.g. bus callback or recovery) claims the lease and writes
        let resumer_owner = "bus-resumer-x";
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                resumer_owner,
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some(), "Resumer must claim the released lease");

        // 3. Write state-advancing change under the claimed owner
        let res_write = store
            .update_instance_state(
                tenant_id,
                resumer_owner,
                iid,
                ProcessState::Completed { at: 999 },
            )
            .await;
        assert!(
            res_write.is_ok(),
            "Resumer write under held lease must succeed"
        );

        // 4. Assert that the write occurred under the claimed owner in the DB
        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(loaded.state, ProcessState::Completed { at: 999 }));
    }

    /// E-invariant: After a tick parks an instance, a different worker can claim the lease successfully (lease was released).
    #[tokio::test]
    async fn test_park_releases_lease() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // 1. Seed instance with state = Running
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        inst.state = ProcessState::Running;
        store.save_instance("owner-a", &inst).await.unwrap();

        // 2. Claim it under owner-a
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-a",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        // 3. Commit a tick that has no running fibers (parks the instance)
        // Since no fibers exist in the fibers table for this instance, has_running_fiber is false.
        commit_ops(&store, iid, tenant_id, "owner-a", &[])
            .await
            .unwrap();

        // 4. Verify lease is released (lease_owner is NULL)
        let row: (Option<String>,) =
            sqlx::query_as("SELECT lease_owner FROM workflow_instances WHERE instance_id = $1")
                .bind(iid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(row.0.is_none(), "Lease owner must be cleared after park");

        // 5. A different worker (owner-b) can claim it successfully
        let claimed_b = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-b",
                30000,
            )
            .await
            .unwrap();
        assert!(
            claimed_b.is_some(),
            "Different worker must be able to claim the released lease"
        );
    }

    /// C2: Postgres single-transaction atomicity test (the decisive proof)
    #[tokio::test]
    async fn test_pg_commit_tick_atomicity() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // 1. Seed a process instance in a known persisted state
        // (one parent fiber, join count 0, known payload)
        let parent_fiber_id = Uuid::now_v7();
        let parent_fiber = Fiber::new(parent_fiber_id, 0);

        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        inst.domain_payload = r#""initial_payload""#.to_string().into();
        inst.domain_payload_hash = [1u8; 32];
        inst.state = ProcessState::Running;

        store.save_instance("default", &inst).await.unwrap();
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "default",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());
        store.save_fiber(iid, &parent_fiber).await.unwrap();

        // 2. Build a tick op-record with several ops:
        // - Spawn child fiber 1
        // - Spawn child fiber 2
        // - Arrive at join barrier
        // - Delete parent fiber
        // - Consume a message designed to FAIL (because it is not claimed, or invalid token)
        let child1 = Fiber::new(Uuid::now_v7(), 1);
        let child2 = Fiber::new(Uuid::now_v7(), 1);

        let msg = ClaimedBufferedMessage {
            message: BufferedMessage {
                tenant_id: tenant_id.to_string(),
                message_name: "nonexistent".to_string(),
                correlation_key: "nonexistent".to_string(),
                msg_id: "nonexistent".to_string(),
                payload: vec![],
                payload_hash: None,
                process_instance_id: Some(iid),
                received_at: 0,
                expires_at: 300000,
            },
            claim_token: "wrong-token".to_string(),
            claim_until: 9999999999,
        };

        let ops = vec![
            TickOperation::SaveFiber {
                fiber: child1.clone(),
            },
            TickOperation::SaveFiber {
                fiber: child2.clone(),
            },
            TickOperation::JoinArrive { join_id: 100 },
            TickOperation::DeleteFiber {
                fiber_id: parent_fiber_id,
            },
            TickOperation::ConsumeBufferedMessage { message: msg }, // FAIL!
        ];

        // 3. Run the engine's atomic apply; assert it returns Err
        let res = commit_ops(&store, iid, tenant_id, "default", &ops).await;
        assert!(res.is_err(), "Expected transaction to fail and roll back");

        // 4. Query the Postgres database directly and assert NONE of the ops persisted:
        // - parent fiber intact
        // - zero child fibers
        // - join count still 0
        // - no new event rows
        // - instance state/payload unchanged
        let fibers = store
            .load_fibers(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap();
        assert_eq!(
            fibers.len(),
            1,
            "Rollback failed: expected exactly 1 fiber (parent)"
        );
        assert_eq!(
            fibers[0].fiber_id, parent_fiber_id,
            "Rollback failed: parent fiber not intact"
        );

        let join_count = store
            .join_get(&TenantId::new("default").unwrap(), iid, 100)
            .await
            .unwrap();
        assert_eq!(join_count, 0, "Rollback failed: join count updated");

        let events = store
            .read_events(&TenantId::new("default").unwrap(), iid, 0)
            .await
            .unwrap();
        assert_eq!(events.len(), 0, "Rollback failed: events appended");

        let loaded_inst = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_inst.domain_payload.as_ref(), r#""initial_payload""#);

        // 5. Re-run the tick without the failing op; assert it commits and all ops persist correctly
        let successful_ops = vec![
            TickOperation::SaveFiber {
                fiber: child1.clone(),
            },
            TickOperation::SaveFiber {
                fiber: child2.clone(),
            },
            TickOperation::JoinArrive { join_id: 100 },
            TickOperation::DeleteFiber {
                fiber_id: parent_fiber_id,
            },
            TickOperation::UpdateInstanceState {
                state: ProcessState::Completed { at: 123456 },
            },
        ];
        let res2 = commit_ops(&store, iid, tenant_id, "default", &successful_ops).await;
        assert!(res2.is_ok(), "Expected transaction to succeed: {:?}", res2);

        let fibers = store
            .load_fibers(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap();
        assert_eq!(fibers.len(), 2, "Expected exactly 2 child fibers");
        assert!(fibers.iter().any(|f| f.fiber_id == child1.fiber_id));
        assert!(fibers.iter().any(|f| f.fiber_id == child2.fiber_id));
        assert!(!fibers.iter().any(|f| f.fiber_id == parent_fiber_id));

        let join_count = store
            .join_get(&TenantId::new("default").unwrap(), iid, 100)
            .await
            .unwrap();
        assert_eq!(join_count, 1);

        let loaded_inst = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            loaded_inst.state,
            ProcessState::Completed { at: 123456 }
        ));
    }

    /// E-invariant: Emit atomicity (RISK-003). Atomically inserts outbox, pending, and saves instance, rolling back completely on failure.
    #[tokio::test]
    async fn test_risk_003_emit_atomicity() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("default", &inst).await.unwrap();
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "default",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        // Build a pending record
        let callout_id = Uuid::now_v7();
        let pending = PendingInvocation::new(
            TenantId::new(tenant_id).unwrap(),
            callout_id,
            iid,
            "node-1",
            "domain-x",
            "verb-y",
            Uuid::now_v7(),
        );

        // Build operations: insert pending, insert outbox, save instance, AND a failing op (e.g. consume message designed to fail)
        let fail_msg = ClaimedBufferedMessage {
            message: BufferedMessage {
                tenant_id: tenant_id.to_string(),
                message_name: "nonexistent".to_string(),
                correlation_key: "nonexistent".to_string(),
                msg_id: "nonexistent".to_string(),
                payload: vec![],
                payload_hash: None,
                process_instance_id: Some(iid),
                received_at: 0,
                expires_at: 300000,
            },
            claim_token: "wrong-token".to_string(),
            claim_until: 9999999999,
        };

        let ops = vec![
            TickOperation::InsertPendingInvocation {
                pending: pending.clone(),
            },
            TickOperation::InsertOutbox {
                id: Uuid::now_v7(),
                target_domain: "domain-x".to_string(),
                target_endpoint: "invocation".to_string(),
                payload: vec![1, 2, 3],
                idempotency_key: pending.idempotency_key,
                callout_id,
            },
            TickOperation::SaveInstance {
                instance: inst.clone(),
            },
            TickOperation::ConsumeBufferedMessage { message: fail_msg }, // FAIL!
        ];

        let commit_res = commit_ops(&store, iid, tenant_id, "default", &ops).await;
        assert!(
            commit_res.is_err(),
            "Expected emit commit to fail and roll back"
        );

        // Verify no pending row, no outbox row, and instance not advanced
        let pending_row: (i64,) =
            sqlx::query_as("SELECT count(*) FROM bpmn_pending_invocation WHERE callout_id = $1")
                .bind(callout_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending_row.0, 0, "Rollback failed: pending row found");

        let outbox_row: (i64,) =
            sqlx::query_as("SELECT count(*) FROM dsl_bus.outbox WHERE callout_id = $1")
                .bind(callout_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(outbox_row.0, 0, "Rollback failed: outbox row found");

        // Now run the successful commit
        let successful_ops = vec![
            TickOperation::InsertPendingInvocation {
                pending: pending.clone(),
            },
            TickOperation::InsertOutbox {
                id: Uuid::now_v7(),
                target_domain: "domain-x".to_string(),
                target_endpoint: "invocation".to_string(),
                payload: vec![1, 2, 3],
                idempotency_key: pending.idempotency_key,
                callout_id,
            },
        ];
        commit_ops(&store, iid, tenant_id, "default", &successful_ops)
            .await
            .unwrap();

        // Verify both rows now exist
        let pending_row2: (i64,) =
            sqlx::query_as("SELECT count(*) FROM bpmn_pending_invocation WHERE callout_id = $1")
                .bind(callout_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            pending_row2.0, 1,
            "Pending row not found after successful commit"
        );

        let outbox_row2: (i64,) =
            sqlx::query_as("SELECT count(*) FROM dsl_bus.outbox WHERE callout_id = $1")
                .bind(callout_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            outbox_row2.0, 1,
            "Outbox row not found after successful commit"
        );
    }

    /// E-invariant: Duplicate-result idempotency (RISK-004). Delivering the same result twice results in a no-op on the second run.
    #[tokio::test]
    async fn test_risk_004_duplicate_result_idempotency() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("default", &inst).await.unwrap();

        // Write a pending invocation row with execution_id
        let callout_id = Uuid::now_v7();
        let execution_id = Uuid::now_v7();
        let mut pending = PendingInvocation::new(
            TenantId::new(tenant_id).unwrap(),
            callout_id,
            iid,
            "node-1",
            "domain-x",
            "verb-y",
            Uuid::now_v7(),
        );
        pending.execution_id = Some(execution_id);

        let p_store = PostgresPendingInvocationStore::new(pool.clone());
        p_store.insert(pending.clone()).await.unwrap();

        // Claim transition lease for the first delivery
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-first",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        // First delivery: commit take pending + state change
        inst.state = ProcessState::Running; // advance state
        let ops = vec![
            TickOperation::TakePendingInvocation { execution_id },
            TickOperation::SaveInstance {
                instance: inst.clone(),
            },
        ];
        let first_res = commit_ops(&store, iid, tenant_id, "owner-first", &ops).await;
        assert!(first_res.is_ok(), "First delivery must succeed");

        // Release first lease manually (as we didn't park)
        store
            .release_instance_transition(&TenantId::new(tenant_id).unwrap(), iid, "owner-first")
            .await
            .unwrap();

        // Second delivery (re-delivery of same execution_id):
        // Claim transition lease for the second delivery
        let claimed_second = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-second",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed_second.is_some());

        // Try to commit the exact same ops (re-delivery)
        let second_ops = vec![
            TickOperation::TakePendingInvocation { execution_id },
            TickOperation::SaveInstance {
                instance: inst.clone(),
            },
        ];
        let second_res = commit_ops(&store, iid, tenant_id, "owner-second", &second_ops).await;
        assert!(
            second_res.is_err(),
            "Second delivery must fail because row is already gone"
        );
    }

    #[tokio::test]
    async fn test_pg_atomic_complete_idempotency() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";
        let lease_owner = "owner-a";

        // Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        inst.state = ProcessState::Running;
        store.save_instance("default", &inst).await.unwrap();

        // Claim lease
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                lease_owner,
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        let job_key = format!("{}-job", iid);
        let completion = JobCompletion {
            job_key: job_key.clone(),
            domain_payload: r#"{"done":true}"#.to_string(),
            expected_instance_payload_hash: test_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };

        // First completion call: advance state to Completed { at: 11111 }, add one event
        inst.state = ProcessState::Completed { at: 11111 };
        let events1 = vec![RuntimeEvent::JobCompleted {
            job_key: job_key.clone(),
            payload_hash_before: [0; 32],
            payload_hash_after: [1; 32],
            orch_flags_out: BTreeMap::new(),
            pc_next: 10.into(),
        }];
        complete_via_transition(&store, tenant_id, lease_owner, &inst, &completion, &events1)
            .await
            .unwrap();

        // Load and verify first completion applied
        let loaded1 = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded1.state, ProcessState::Completed { at: 11111 });

        let loaded_events1 = store
            .read_events(&TenantId::new("default").unwrap(), iid, 1)
            .await
            .unwrap();
        assert_eq!(loaded_events1.len(), 1);

        // Second completion call: try to advance state to Completed { at: 22222 }, add another event
        let mut inst2 = loaded1.clone();
        inst2.state = ProcessState::Completed { at: 22222 };
        let events2 = vec![RuntimeEvent::JobCompleted {
            job_key: job_key.clone(),
            payload_hash_before: [1; 32],
            payload_hash_after: [2; 32],
            orch_flags_out: BTreeMap::new(),
            pc_next: 11.into(),
        }];
        complete_via_transition(
            &store,
            tenant_id,
            lease_owner,
            &inst2,
            &completion,
            &events2,
        )
        .await
        .unwrap();

        // Load and verify second completion was a NO-OP: state remains Completed { at: 11111 } and event count remains 1
        let loaded2 = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded2.state, ProcessState::Completed { at: 11111 });

        let loaded_events2 = store
            .read_events(&TenantId::new("default").unwrap(), iid, 1)
            .await
            .unwrap();
        assert_eq!(loaded_events2.len(), 1);
    }

    /// E-invariant F2 negative test: a non-dedup commit_tick failure on the advance path (e.g. lease fence failure)
    /// must not be swallowed as AlreadyConsumedError. The error must propagate and the pending row must NOT be deleted.
    #[tokio::test]
    async fn test_risk_004_negative_other_failures_propagate() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("default", &inst).await.unwrap();

        // Write a pending invocation row
        let callout_id = Uuid::now_v7();
        let execution_id = Uuid::now_v7();
        let mut pending = PendingInvocation::new(
            TenantId::new(tenant_id).unwrap(),
            callout_id,
            iid,
            "node-1",
            "domain-x",
            "verb-y",
            Uuid::now_v7(),
        );
        pending.execution_id = Some(execution_id);

        let p_store = PostgresPendingInvocationStore::new(pool.clone());
        p_store.insert(pending.clone()).await.unwrap();

        // Claim the lease under "actual-owner"
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "actual-owner",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());

        // Attempt commit_tick under a different owner "wrong-owner" -> should fail due to lease fence rejection!
        let ops = vec![
            TickOperation::TakePendingInvocation { execution_id },
            TickOperation::SaveInstance {
                instance: inst.clone(),
            },
        ];
        let res = commit_ops(&store, iid, tenant_id, "wrong-owner", &ops).await;
        assert!(res.is_err(), "Expected lease fence error to propagate");

        let err = res.unwrap_err();
        // Assert it is NOT AlreadyConsumedError
        assert!(
            !err.to_string().contains("already consumed"),
            "Lease fence error must not be masked as AlreadyConsumedError"
        );

        // Assert that the pending row was NOT deleted (since the transaction rolled back)
        let row_count: (i64,) =
            sqlx::query_as("SELECT count(*) FROM bpmn_pending_invocation WHERE execution_id = $1")
                .bind(execution_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            row_count.0, 1,
            "Pending row must still exist since transaction rolled back"
        );
    }

    /// E-invariant: Concurrent Claim races and Concurrent Recovery (T3.3.1)
    #[tokio::test]
    async fn test_concurrent_claim_and_recovery() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // Seed instance with state = Running, lease expired
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        inst.state = ProcessState::Running;
        store.save_instance("default", &inst).await.unwrap();

        // Claim and expire it
        let claimed = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                iid,
                "owner-temp",
                30000,
            )
            .await
            .unwrap();
        assert!(claimed.is_some());
        sqlx::query("UPDATE workflow_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // Race two claimers concurrently
        let store_arc = Arc::new(store);
        let s1 = store_arc.clone();
        let s2 = store_arc.clone();

        let t1 = tokio::spawn(async move {
            s1.claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                iid,
                "claimer-1",
                30000,
            )
            .await
        });
        let t2 = tokio::spawn(async move {
            s2.claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                iid,
                "claimer-2",
                30000,
            )
            .await
        });

        let r1 = t1.await.unwrap().unwrap();
        let r2 = t2.await.unwrap().unwrap();

        // Assert exactly one won, and the other returned false (loser no-ops gracefully)
        assert_ne!(
            r1.is_some(),
            r2.is_some(),
            "Exactly one claimer must succeed"
        );
        assert!(
            r1.is_some() || r2.is_some(),
            "At least one claimer must succeed"
        );

        // Now test concurrent recovery:
        // Set the instance to Failed (simulating crash)
        let mut inst = store_arc
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        inst.state = ProcessState::Failed {
            incident_id: Uuid::now_v7(),
        };
        // Save using whichever won the lease
        let active_owner = if r1.is_some() {
            "claimer-1"
        } else {
            "claimer-2"
        };
        store_arc.save_instance(active_owner, &inst).await.unwrap();

        // Expire lease again so it's reclaimable by recovery
        sqlx::query("UPDATE workflow_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // Simulating two recovery processes racing
        let s1_rec = store_arc.clone();
        let s2_rec = store_arc.clone();

        let rec1 = tokio::spawn(async move {
            let owner = "recovery-1";
            let claimed = s1_rec
                .claim_instance_for_transition(
                    &TenantId::new("default").unwrap(),
                    iid,
                    owner,
                    30000,
                )
                .await
                .unwrap();
            if claimed.is_some() {
                // Recover the instance
                let mut inst = s1_rec
                    .load_instance(&TenantId::new("default").unwrap(), iid)
                    .await
                    .unwrap()
                    .unwrap();
                inst.state = ProcessState::Running;
                s1_rec.save_instance(owner, &inst).await.unwrap();
                s1_rec
                    .release_instance_transition(&TenantId::new("default").unwrap(), iid, owner)
                    .await
                    .unwrap();
                true
            } else {
                false
            }
        });

        let rec2 = tokio::spawn(async move {
            let owner = "recovery-2";
            let claimed = s2_rec
                .claim_instance_for_transition(
                    &TenantId::new("default").unwrap(),
                    iid,
                    owner,
                    30000,
                )
                .await
                .unwrap();
            if claimed.is_some() {
                // Recover the instance
                let mut inst = s2_rec
                    .load_instance(&TenantId::new("default").unwrap(), iid)
                    .await
                    .unwrap()
                    .unwrap();
                inst.state = ProcessState::Running;
                s2_rec.save_instance(owner, &inst).await.unwrap();
                s2_rec
                    .release_instance_transition(&TenantId::new("default").unwrap(), iid, owner)
                    .await
                    .unwrap();
                true
            } else {
                false
            }
        });

        let res_rec1 = rec1.await.unwrap();
        let res_rec2 = rec2.await.unwrap();

        assert!(
            res_rec1 != res_rec2,
            "Exactly one recovery runner must claim the instance"
        );
    }

    /// T4/E2: a lease holder whose fence has been superseded cannot commit,
    /// even when its owner string and snapshot revision are otherwise valid.
    #[tokio::test]
    async fn test_fenced_transition_rejects_expired_owner() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let tenant_id = "default";
        let instance = make_instance(instance_id);
        store.save_instance("seed", &instance).await.unwrap();

        let first = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "owner-a",
                30_000,
            )
            .await
            .unwrap()
            .expect("owner-a must acquire the initial claim");
        let renewal = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "owner-a",
                30_000,
            )
            .await
            .unwrap()
            .expect("owner-a must renew its claim");
        assert_eq!(
            renewal.fence(),
            first.fence(),
            "renewal must not increment fence"
        );

        sqlx::query(
            "UPDATE workflow_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1",
        )
        .bind(instance_id)
        .execute(&pool)
        .await
        .unwrap();

        let second = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "owner-b",
                30_000,
            )
            .await
            .unwrap()
            .expect("owner-b must acquire the expired lease");
        assert_eq!(second.fence(), first.fence() + 1);
        assert_eq!(second.expected_revision(), first.expected_revision());

        let mut completed = instance.clone();
        completed.state = ProcessState::Completed { at: 42 };
        let stale_transition = TransitionBuilder::new(completed.clone()).build();
        let stale = store.commit_transition(&first, &stale_transition).await;
        assert!(matches!(stale, Err(CommitError::StaleFence)));

        let unchanged: (i64, serde_json::Value) =
            sqlx::query_as("SELECT revision, state FROM workflow_instances WHERE instance_id = $1")
                .bind(instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unchanged.0, 0);
        assert_eq!(unchanged.1, serde_json::json!("Running"));

        let current_transition = TransitionBuilder::new(completed).build();
        store
            .commit_transition(&second, &current_transition)
            .await
            .unwrap();

        let committed: (i64, serde_json::Value) =
            sqlx::query_as("SELECT revision, state FROM workflow_instances WHERE instance_id = $1")
                .bind(instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(committed.0, 1);
        assert_eq!(
            committed.1,
            serde_json::json!({ "Completed": { "at": 42 } })
        );
    }

    #[tokio::test]
    async fn test_artifact_insert_verifies_bytes_and_detects_collision() {
        let (pool, store, _lock) = setup().await;
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="artifact" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="flow" sourceRef="start" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;
        let graph = bpmn_lite_compiler::parse_bpmn(xml).unwrap();
        let artifact = bpmn_lite_compiler::Compiler::lower(&graph).unwrap();

        store.store_artifact(&artifact).await.unwrap();
        store.store_artifact(&artifact).await.unwrap();
        let loaded = store.load_artifact(artifact.hash()).await.unwrap().unwrap();
        assert_eq!(loaded.hash(), artifact.hash());
        assert_eq!(
            loaded.canonical_bytes().unwrap(),
            artifact.canonical_bytes().unwrap()
        );

        sqlx::query("UPDATE compiled_programs SET canonical_bytes = $2 WHERE artifact_hash = $1")
            .bind(&artifact.hash().into_bytes()[..])
            .bind(b"corrupt".as_slice())
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            store.store_artifact(&artifact).await,
            Err(ArtifactStoreError::ArtifactCollision { .. })
        ));
    }

    #[tokio::test]
    async fn test_legacy_artifact_load_recanonicalizes_and_records_lineage() {
        let (pool, store, _lock) = setup().await;
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="legacy" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="flow" sourceRef="start" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;
        let graph = bpmn_lite_compiler::parse_bpmn(xml).unwrap();
        let legacy = bpmn_lite_compiler::lower(&graph).unwrap();
        let old_hash = legacy.bytecode_version();
        store.store_program(old_hash, &legacy).await.unwrap();

        let migrated = store
            .load_artifact(ArtifactHash::from_bytes(old_hash))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(migrated.hash().into_bytes(), old_hash);

        let recorded: Vec<u8> =
            sqlx::query_scalar("SELECT new_hash FROM artifact_lineage WHERE old_hash = $1")
                .bind(&old_hash[..])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(recorded, migrated.hash().as_bytes());
    }

    #[tokio::test]
    async fn test_durable_timer_survives_transition_cut_points() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let fiber_id = Uuid::now_v7();
        let tenant_id = "default";
        let instance = make_instance(instance_id);
        store.save_instance("seed", &instance).await.unwrap();

        let stale_claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "park-a",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE workflow_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1",
        )
        .bind(instance_id)
        .execute(&pool)
        .await
        .unwrap();
        let park_claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "park-b",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();

        let timer_id = EffectId::for_instruction(instance_id, fiber_id, 0);
        let mut parked_fiber = Fiber::new(fiber_id, 1);
        parked_fiber.wait = WaitState::Timer { deadline_ms: 10 };
        let park_transition = TransitionBuilder::new(instance.clone())
            .upsert_fiber(parked_fiber.clone())
            .effect(DurableEffect::schedule_timer(
                timer_id,
                fiber_id,
                10,
                TimerKind::Wait,
                None,
            ))
            .build();

        assert!(matches!(
            store
                .commit_transition(&stale_claim, &park_transition)
                .await,
            Err(CommitError::StaleFence)
        ));
        let before_park: i64 =
            sqlx::query_scalar("SELECT count(*) FROM workflow_timers WHERE timer_id = $1")
                .bind(timer_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before_park, 0, "failed park commit must not leak a timer");

        assert!(matches!(
            store
                .commit_transition(&park_claim, &park_transition)
                .await
                .unwrap(),
            CommitOutcome::Committed { .. }
        ));
        let first_delivery = store
            .claim_due_timers(&TenantId::new(tenant_id).unwrap(), "timer-a", 10, 1, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();

        sqlx::query(
            "UPDATE workflow_timers SET claim_until = now() - interval '1 second' WHERE timer_id = $1",
        )
        .bind(timer_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
        let recovery_time = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
        let recovered_delivery = store
            .claim_due_timers(
                &TenantId::new(tenant_id).unwrap(),
                "timer-b",
                recovery_time,
                1,
                30_000,
            )
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(first_delivery.timer_id(), recovered_delivery.timer_id());
        assert_ne!(
            first_delivery.claim_token(),
            recovered_delivery.claim_token()
        );

        let consume_claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "timer-b",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        let mut running_fiber = parked_fiber;
        running_fiber.wait = WaitState::Running;
        let consume_transition = TransitionBuilder::new(instance.clone())
            .upsert_fiber(running_fiber)
            .timer_mutation(TimerMutation::Consume {
                timer_id,
                claim_token: recovered_delivery.claim_token(),
            })
            .event(RuntimeEvent::TimerFired {
                timer_id,
                fiber_id,
                fired_at: 10,
            })
            .build();
        assert!(matches!(
            store
                .commit_transition(&consume_claim, &consume_transition)
                .await
                .unwrap(),
            CommitOutcome::Committed { .. }
        ));

        store
            .release_instance_transition(&TenantId::new(tenant_id).unwrap(), instance_id, "timer-b")
            .await
            .unwrap();
        let duplicate_claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "timer-c",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        let revision_before_duplicate = duplicate_claim.expected_revision();
        assert_eq!(
            store
                .commit_transition(&duplicate_claim, &consume_transition)
                .await
                .unwrap(),
            CommitOutcome::IdempotentNoOp
        );
        let revision_after_duplicate: i64 =
            sqlx::query_scalar("SELECT revision FROM workflow_instances WHERE instance_id = $1")
                .bind(instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(revision_after_duplicate as u64, revision_before_duplicate);
        let timer_state: String =
            sqlx::query_scalar("SELECT state FROM workflow_timers WHERE timer_id = $1")
                .bind(timer_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(timer_state, "consumed");
    }

    struct ViolatingTestStore {
        inner: Arc<PostgresWorkflowStore>,
        violate_instance_id: Uuid,
        should_fail_load_integrity: std::sync::atomic::AtomicBool,
        should_fail_commit_integrity: std::sync::atomic::AtomicBool,
        should_fail_generic: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl RuntimeStore for ViolatingTestStore {
        async fn load_instance(
            &self,
            tenant_id: &TenantId,
            id: Uuid,
        ) -> StoreResult<Option<ProcessInstance>> {
            if id == self.violate_instance_id {
                if self
                    .should_fail_load_integrity
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    return Err(StoreError::Integrity(
                        "injected instance integrity violation".into(),
                    ));
                }
                if self
                    .should_fail_generic
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    return Err(StoreError::Unavailable(
                        "injected generic load failure".into(),
                    ));
                }
            }
            <PostgresWorkflowStore as RuntimeStore>::load_instance(&*self.inner, tenant_id, id)
                .await
        }

        async fn load_fiber(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            fiber_id: Uuid,
        ) -> StoreResult<Option<Fiber>> {
            <PostgresWorkflowStore as RuntimeStore>::load_fiber(
                &*self.inner,
                tenant_id,
                instance_id,
                fiber_id,
            )
            .await
        }

        async fn load_fibers(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
        ) -> StoreResult<Vec<Fiber>> {
            <PostgresWorkflowStore as RuntimeStore>::load_fibers(
                &*self.inner,
                tenant_id,
                instance_id,
            )
            .await
        }

        async fn dedupe_get(
            &self,
            tenant_id: &TenantId,
            key: &str,
        ) -> StoreResult<Option<JobCompletion>> {
            <PostgresWorkflowStore as RuntimeStore>::dedupe_get(&*self.inner, tenant_id, key).await
        }

        async fn dequeue_jobs(
            &self,
            task_types: &[String],
            max: usize,
            tenant_id: &TenantId,
            worker_id: &str,
            lease_ms: u64,
        ) -> StoreResult<Vec<JobActivation>> {
            <PostgresWorkflowStore as RuntimeStore>::dequeue_jobs(
                &*self.inner,
                task_types,
                max,
                tenant_id,
                worker_id,
                lease_ms,
            )
            .await
        }

        async fn validate_job_claim(
            &self,
            tenant_id: &TenantId,
            job_key: &str,
            worker_id: &str,
            claim_token: &str,
        ) -> StoreResult<bool> {
            <PostgresWorkflowStore as RuntimeStore>::validate_job_claim(
                &*self.inner,
                tenant_id,
                job_key,
                worker_id,
                claim_token,
            )
            .await
        }

        async fn dead_letter_put(
            &self,
            name: u32,
            corr_key: &Value,
            payload: &[u8],
            ttl_ms: u64,
        ) -> StoreResult<()> {
            <PostgresWorkflowStore as RuntimeStore>::dead_letter_put(
                &*self.inner,
                name,
                corr_key,
                payload,
                ttl_ms,
            )
            .await
        }

        async fn dead_letter_take(
            &self,
            name: u32,
            corr_key: &Value,
        ) -> StoreResult<Option<Vec<u8>>> {
            <PostgresWorkflowStore as RuntimeStore>::dead_letter_take(&*self.inner, name, corr_key)
                .await
        }

        async fn claim_buffered_message(
            &self,
            tenant_id: &TenantId,
            message_name: &str,
            correlation_key: &str,
            claim_ms: u64,
        ) -> StoreResult<Option<ClaimedBufferedMessage>> {
            <PostgresWorkflowStore as RuntimeStore>::claim_buffered_message(
                &*self.inner,
                tenant_id,
                message_name,
                correlation_key,
                claim_ms,
            )
            .await
        }

        async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32> {
            <PostgresWorkflowStore as RuntimeStore>::reclaim_stale_buffered_message_claims(
                &*self.inner,
            )
            .await
        }

        async fn prune_expired_messages(&self) -> StoreResult<u32> {
            <PostgresWorkflowStore as RuntimeStore>::prune_expired_messages(&*self.inner).await
        }

        async fn load_incidents(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
        ) -> StoreResult<Vec<Incident>> {
            <PostgresWorkflowStore as RuntimeStore>::load_incidents(
                &*self.inner,
                tenant_id,
                instance_id,
            )
            .await
        }

        async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> StoreResult<u32> {
            <PostgresWorkflowStore as RuntimeStore>::reclaim_stale_jobs(&*self.inner, timeout_ms)
                .await
        }

        async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32> {
            <PostgresWorkflowStore as RuntimeStore>::prune_dedupe_cache(&*self.inner, older_than_ms)
                .await
        }

        async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>> {
            <PostgresWorkflowStore as RuntimeStore>::list_running_instances(&*self.inner, tenant_id)
                .await
        }

        async fn claim_running_instances(
            &self,
            tenant_id: &TenantId,
            owner: &str,
            limit: usize,
            lease_ms: u64,
        ) -> StoreResult<Vec<Uuid>> {
            <PostgresWorkflowStore as RuntimeStore>::claim_running_instances(
                &*self.inner,
                tenant_id,
                owner,
                limit,
                lease_ms,
            )
            .await
        }

        async fn claim_instance_for_transition(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            owner: &str,
            lease_ms: u64,
        ) -> std::result::Result<Option<Claim>, ClaimError> {
            <PostgresWorkflowStore as RuntimeStore>::claim_instance_for_transition(
                &*self.inner,
                tenant_id,
                instance_id,
                owner,
                lease_ms,
            )
            .await
        }

        async fn commit_transition(
            &self,
            claim: &Claim,
            transition: &Transition,
        ) -> std::result::Result<CommitOutcome, CommitError> {
            if claim.instance_id() == self.violate_instance_id
                && self
                    .should_fail_commit_integrity
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(CommitError::Integrity(
                    "injected transition integrity violation".into(),
                ));
            }
            <PostgresWorkflowStore as RuntimeStore>::commit_transition(
                &*self.inner,
                claim,
                transition,
            )
            .await
        }

        async fn lookup_start_instance(
            &self,
            tenant_id: &TenantId,
            idempotency_key: Uuid,
        ) -> StoreResult<Option<Uuid>> {
            <PostgresWorkflowStore as RuntimeStore>::lookup_start_instance(
                &*self.inner,
                tenant_id,
                idempotency_key,
            )
            .await
        }

        async fn claim_due_timers(
            &self,
            tenant_id: &TenantId,
            owner: &str,
            now_ms: u64,
            limit: usize,
            lease_ms: u64,
        ) -> StoreResult<Vec<ClaimedTimer>> {
            <PostgresWorkflowStore as RuntimeStore>::claim_due_timers(
                &*self.inner,
                tenant_id,
                owner,
                now_ms,
                limit,
                lease_ms,
            )
            .await
        }

        async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()> {
            <PostgresWorkflowStore as RuntimeStore>::release_timer_claim(&*self.inner, timer).await
        }

        async fn claim_pending_effects(
            &self,
            tenant_id: &TenantId,
            owner: &str,
            now_ms: u64,
            limit: usize,
            lease_ms: u64,
        ) -> StoreResult<Vec<ClaimedEffect>> {
            <PostgresWorkflowStore as RuntimeStore>::claim_pending_effects(
                &*self.inner,
                tenant_id,
                owner,
                now_ms,
                limit,
                lease_ms,
            )
            .await
        }

        async fn record_effect_response(
            &self,
            effect: &ClaimedEffect,
            response: &EffectResponse,
        ) -> StoreResult<bool> {
            <PostgresWorkflowStore as RuntimeStore>::record_effect_response(
                &*self.inner,
                effect,
                response,
            )
            .await
        }

        async fn load_effect_responses(
            &self,
            tenant_id: &TenantId,
            limit: usize,
        ) -> StoreResult<Vec<PendingEffectResponse>> {
            <PostgresWorkflowStore as RuntimeStore>::load_effect_responses(
                &*self.inner,
                tenant_id,
                limit,
            )
            .await
        }

        async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()> {
            <PostgresWorkflowStore as RuntimeStore>::release_effect_claim(&*self.inner, effect)
                .await
        }

        async fn schedule_effect_retry(
            &self,
            effect: &ClaimedEffect,
            decision: RetryDecision,
            error: &str,
        ) -> StoreResult<()> {
            <PostgresWorkflowStore as RuntimeStore>::schedule_effect_retry(
                &*self.inner,
                effect,
                decision,
                error,
            )
            .await
        }

        async fn release_instance_transition(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            owner: &str,
        ) -> StoreResult<()> {
            <PostgresWorkflowStore as RuntimeStore>::release_instance_transition(
                &*self.inner,
                tenant_id,
                instance_id,
                owner,
            )
            .await
        }

        async fn join_get(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            join_id: JoinId,
        ) -> StoreResult<u16> {
            <PostgresWorkflowStore as RuntimeStore>::join_get(
                &*self.inner,
                tenant_id,
                instance_id,
                join_id,
            )
            .await
        }
    }
    #[async_trait]
    impl ArtifactRepository for ViolatingTestStore {
        async fn store_program(
            &self,
            version: [u8; 32],
            program: &CompiledProgram,
        ) -> StoreResult<()> {
            <PostgresWorkflowStore as ArtifactRepository>::store_program(
                &*self.inner,
                version,
                program,
            )
            .await
        }

        async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>> {
            <PostgresWorkflowStore as ArtifactRepository>::load_program(&*self.inner, version).await
        }

        async fn store_artifact(
            &self,
            artifact: &ExecutableWorkflow,
        ) -> std::result::Result<(), ArtifactStoreError> {
            <PostgresWorkflowStore as ArtifactRepository>::store_artifact(&*self.inner, artifact)
                .await
        }

        async fn load_artifact(
            &self,
            hash: ArtifactHash,
        ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError> {
            <PostgresWorkflowStore as ArtifactRepository>::load_artifact(&*self.inner, hash).await
        }

        async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()> {
            <PostgresWorkflowStore as ArtifactRepository>::store_plan(
                &*self.inner,
                plan_hash,
                plan_json,
            )
            .await
        }

        async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>> {
            <PostgresWorkflowStore as ArtifactRepository>::load_plan(&*self.inner, plan_hash).await
        }
    }
    #[async_trait]
    impl JournalReader for ViolatingTestStore {
        async fn read_events(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            from_seq: u64,
        ) -> StoreResult<Vec<(u64, RuntimeEvent)>> {
            <PostgresWorkflowStore as JournalReader>::read_events(
                &*self.inner,
                tenant_id,
                instance_id,
                from_seq,
            )
            .await
        }

        async fn load_snapshot_envelope(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
        ) -> StoreResult<Option<SnapshotEnvelope>> {
            <PostgresWorkflowStore as JournalReader>::load_snapshot_envelope(
                &*self.inner,
                tenant_id,
                instance_id,
            )
            .await
        }

        async fn read_journal(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            after_revision: Option<u64>,
        ) -> StoreResult<Vec<JournalRecord>> {
            <PostgresWorkflowStore as JournalReader>::read_journal(
                &*self.inner,
                tenant_id,
                instance_id,
                after_revision,
            )
            .await
        }

        async fn load_payload_version(
            &self,
            tenant_id: &TenantId,
            instance_id: Uuid,
            hash: &[u8; 32],
        ) -> StoreResult<Option<String>> {
            <PostgresWorkflowStore as JournalReader>::load_payload_version(
                &*self.inner,
                tenant_id,
                instance_id,
                hash,
            )
            .await
        }
    }
    #[async_trait]
    impl AdminProjectionStore for ViolatingTestStore {
        async fn store_template(
            &self,
            name: &str,
            version: u32,
            plan_hash: [u8; 32],
            dsl_body: &str,
        ) -> StoreResult<()> {
            <PostgresWorkflowStore as AdminProjectionStore>::store_template(
                &*self.inner,
                name,
                version,
                plan_hash,
                dsl_body,
            )
            .await
        }

        async fn load_template_version(
            &self,
            name: &str,
            version: u32,
        ) -> StoreResult<Option<(String, [u8; 32])>> {
            <PostgresWorkflowStore as AdminProjectionStore>::load_template_version(
                &*self.inner,
                name,
                version,
            )
            .await
        }

        async fn load_latest_template_version(
            &self,
            name: &str,
        ) -> StoreResult<Option<(u32, String, [u8; 32])>> {
            <PostgresWorkflowStore as AdminProjectionStore>::load_latest_template_version(
                &*self.inner,
                name,
            )
            .await
        }

        async fn list_templates(&self) -> StoreResult<Vec<TemplateSummary>> {
            <PostgresWorkflowStore as AdminProjectionStore>::list_templates(&*self.inner).await
        }

        async fn health_check(&self) -> StoreResult<()> {
            <PostgresWorkflowStore as AdminProjectionStore>::health_check(&*self.inner).await
        }

        async fn ensure_tenant(&self, tenant_id: &TenantId) -> StoreResult<()> {
            <PostgresWorkflowStore as AdminProjectionStore>::ensure_tenant(&*self.inner, tenant_id)
                .await
        }

        async fn list_tenants(&self) -> StoreResult<Vec<String>> {
            <PostgresWorkflowStore as AdminProjectionStore>::list_tenants(&*self.inner).await
        }

        async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>> {
            <PostgresWorkflowStore as AdminProjectionStore>::list_tenants_in_pool(
                &*self.inner,
                pool_id,
            )
            .await
        }
    }

    /// E-invariant #1 & #2: Violation -> quarantine, not crash, not churn. Quarantine survives rollback.
    /// Drives a tick through the production path, rolls it back, and verifies state changes do not persist while quarantine does.
    #[tokio::test]
    async fn test_pg_integrity_violation_propagates_and_rolls_back() {
        let (_pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let engine = BpmnLiteEngine::new(store.clone());

        // 1. Compile and start a process instance.
        let compiled = engine.compile(SMOKE_BPMN).await.unwrap();
        let version = compiled.bytecode_version;

        let payload = r#"{"case_id":"test-123"}"#;
        let hash = bpmn_lite_vm::compute_hash(payload);
        let iid = engine
            .start("smoke_proc", version, payload, hash, "test-corr-1")
            .await
            .unwrap();

        // Check pre-tick state: quarantine_state is None, fiber pc is 0.
        let loaded_before = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_before.quarantine_state, None);
        let fibers_before = store
            .load_fibers(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap();
        assert_eq!(fibers_before[0].pc, 0.into());

        // 2. Wrap the store in ViolatingTestStore, violating on commit_tick.
        let violating_store = Arc::new(ViolatingTestStore {
            inner: store.clone(),
            violate_instance_id: iid,
            should_fail_load_integrity: std::sync::atomic::AtomicBool::new(false),
            should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(true),
            should_fail_generic: std::sync::atomic::AtomicBool::new(false),
        });
        let engine_with_violating_store = BpmnLiteEngine::new(violating_store.clone());

        // 3. T7 propagates integrity failures. T10 owns atomic quarantine during
        // mandatory integrity verification on load.
        let tick_res = engine_with_violating_store.tick_instance(iid).await;
        assert!(
            tick_res.is_err(),
            "Tick must propagate IntegrityViolation, got {:?}",
            tick_res
        );

        // 4. Assert both halves post-rollback:
        // A. The state change (instance's current_node_id and job queue enqueueing) did NOT persist (rolled back).
        let loaded_after = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded_after.current_node_id.as_deref(),
            None,
            "Instance current_node_id must be rolled back (still None)"
        );

        // Job queue must not contain any jobs for our instance
        let jobs = store
            .dequeue_jobs(
                &["do_work".to_string()],
                100,
                &TenantId::default(),
                "test-worker",
                5000,
            )
            .await
            .unwrap();
        let has_job_for_our_instance = jobs.iter().any(|j| j.process_instance_id == iid);
        assert!(
            !has_job_for_our_instance,
            "Job for our instance must not have been enqueued (rolled back)"
        );

        // B. T7 does not perform an unfenced, separate quarantine write.
        assert_eq!(
            loaded_after.quarantine_state.as_deref(),
            None,
            "quarantine is deferred to the T10 mandatory load gate"
        );
    }

    /// E-invariant #3: Discrimination / no over-quarantine.
    #[tokio::test]
    async fn test_pg_non_integrity_failure_does_not_quarantine() {
        let (_pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let iid = Uuid::now_v7();

        store
            .ensure_tenant(&TenantId::new("default").unwrap())
            .await
            .unwrap();
        let inst = make_instance(iid);
        store.save_instance("test-owner", &inst).await.unwrap();

        let violating_store = Arc::new(ViolatingTestStore {
            inner: store.clone(),
            violate_instance_id: iid,
            should_fail_load_integrity: std::sync::atomic::AtomicBool::new(false),
            should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(false),
            should_fail_generic: std::sync::atomic::AtomicBool::new(true),
        });

        let engine = BpmnLiteEngine::new(violating_store.clone());

        let tick_res = engine.tick_instance(iid).await;
        assert!(
            tick_res.is_err(),
            "Tick must propagate non-integrity errors, got {:?}",
            tick_res
        );

        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.quarantine_state, None,
            "Ordinary failure must not quarantine instance"
        );

        let claimed = store
            .claim_running_instances(
                &TenantId::new("default").unwrap(),
                "test-scheduler",
                10,
                5000,
            )
            .await
            .unwrap();
        assert!(
            claimed.contains(&iid),
            "Ordinary failed instance must remain claimable/retryable"
        );
    }

    /// T7 propagates integrity failures from startup recovery. T10 owns the mandatory
    /// load-integrity gate, atomic quarantine, and readiness behavior.
    #[tokio::test]
    async fn test_pg_integrity_violation_in_startup_recovery() {
        let (pool, store, _lock) = setup().await;
        let store = Arc::new(store);

        let iid_corrupt = Uuid::now_v7();
        let iid_healthy = Uuid::now_v7();

        store
            .ensure_tenant(&TenantId::new("default").unwrap())
            .await
            .unwrap();

        // 1. Publish FFI template as NonIdempotent
        let template_id = [1u8; 32];
        let template_id_hex = hex(&template_id);
        use ffi_catalogue::FfiTemplateStore;
        let ffi_store = Arc::new(crate::ffi_template_store::PostgresFfiTemplateStore::new(
            pool.clone(),
        ));
        let template = ffi_types::FfiTemplate {
            template_id,
            owner_type: "dmn-decision".to_string(),
            owner_metadata: "CheckEligibility".as_bytes().to_vec(),
            input_schema: vec![],
            output_schema: vec![],
            idempotency: ffi_types::Idempotency::NonIdempotent,
            tenant_id: "default".to_string(),
            published_at: 1700000000000,
            publisher: "test".to_string(),
        };
        ffi_store.publish(&template).await.unwrap();

        // Build FfiDispatcher and cache
        let catalogue = Arc::new(ffi_catalogue::FfiCatalogue::new(ffi_store));
        catalogue
            .load_into_cache(&TenantId::new("default").unwrap())
            .await
            .unwrap();
        let dispatcher = Arc::new(ffi_dispatcher::FfiDispatcher::new(catalogue));

        // 2. Save both instances
        let inst_corrupt = make_instance(iid_corrupt);
        let inst_healthy = make_instance(iid_healthy);
        store
            .save_instance("test-owner", &inst_corrupt)
            .await
            .unwrap();
        store
            .save_instance("test-owner", &inst_healthy)
            .await
            .unwrap();

        // Save a fiber for both, so the incident creator can locate the fiber at pc = 0
        let fiber_corrupt = Fiber::new(Uuid::now_v7(), 0);
        let fiber_healthy = Fiber::new(Uuid::now_v7(), 0);
        store.save_fiber(iid_corrupt, &fiber_corrupt).await.unwrap();
        store.save_fiber(iid_healthy, &fiber_healthy).await.unwrap();

        // Append pending Ffi invocation event to both
        let ev_corrupt = RuntimeEvent::FfiInvocationPending {
            invocation_id: Uuid::now_v7(),
            template_id_hex: template_id_hex.clone(),
            caller_task_id: "task1".to_string(),
            caller_pc: 0.into(),
            owner_type: "engine".to_string(),
        };
        let ev_healthy = RuntimeEvent::FfiInvocationPending {
            invocation_id: Uuid::now_v7(),
            template_id_hex: template_id_hex.clone(),
            caller_task_id: "task1".to_string(),
            caller_pc: 0.into(),
            owner_type: "engine".to_string(),
        };
        store.append_event(iid_corrupt, &ev_corrupt).await.unwrap();
        store.append_event(iid_healthy, &ev_healthy).await.unwrap();

        // 3. Wrap in ViolatingTestStore. We return IntegrityViolation on load_instance for iid_corrupt.
        let violating_store = Arc::new(ViolatingTestStore {
            inner: store.clone(),
            violate_instance_id: iid_corrupt,
            should_fail_load_integrity: std::sync::atomic::AtomicBool::new(true),
            should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(false),
            should_fail_generic: std::sync::atomic::AtomicBool::new(false),
        });

        let engine = BpmnLiteEngine::new(violating_store.clone()).with_ffi_dispatcher(dispatcher);

        // 4. Recovery must fail closed rather than silently skipping corruption.
        let recovery_result = engine
            .detect_interrupted_ffi_calls(&TenantId::default())
            .await;
        assert!(
            recovery_result.is_err(),
            "startup recovery must propagate integrity failure"
        );

        // 5. Assert:
        // A. T7 does not perform the unfenced quarantine write that T10 replaces.
        let corrupt_loaded = store
            .load_instance(&TenantId::new("default").unwrap(), iid_corrupt)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(corrupt_loaded.quarantine_state, None);

        // B. Recovery stops at the integrity error; it does not partially advance another
        // instance after a failed readiness scan.
        let healthy_loaded = store
            .load_instance(&TenantId::new("default").unwrap(), iid_healthy)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(healthy_loaded.state, ProcessState::Running),
            "healthy instance must remain Running, got {:?}",
            healthy_loaded.state
        );
        assert_eq!(
            healthy_loaded.quarantine_state, None,
            "Healthy instance must not be quarantined"
        );
    }

    #[tokio::test]
    async fn test_durable_effect_commit_dispatch_inbox_and_consume() {
        let (pool, store, _lock) = setup().await;
        let tenant_id = "default";
        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        store.save_instance("fixture", &instance).await.unwrap();

        let effect_id = EffectId::for_transition(instance_id, 1, 0);
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        fiber.wait = WaitState::Effect { effect_id };
        store.save_fiber(instance_id, &fiber).await.unwrap();
        let effect = DurableEffect::Invoke {
            effect_id,
            fiber_id: fiber.fiber_id,
            pc: 0,
            operation: "fixture.operation".to_string(),
            template_id: [7; 32],
            input: br#"{"input":1}"#.to_vec(),
            idempotency_key: effect_id.as_uuid().to_string(),
        };
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(
                &claim,
                &TransitionBuilder::new(instance.clone())
                    .upsert_fiber(fiber)
                    .effect(effect.clone())
                    .build(),
            )
            .await
            .unwrap();

        let dispatch_now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
        let claimed = store
            .claim_pending_effects(
                &TenantId::new(tenant_id).unwrap(),
                "dispatcher",
                dispatch_now,
                10,
                30_000,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        let response = EffectResponse::FfiNoMatch;
        assert!(store
            .record_effect_response(&claimed[0], &response)
            .await
            .unwrap());
        assert!(!store
            .record_effect_response(&claimed[0], &response)
            .await
            .unwrap());
        let pending = store
            .load_effect_responses(&TenantId::new(tenant_id).unwrap(), 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].effect_id(), effect_id);

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "resume",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        instance.state = ProcessState::Completed { at: 1 };
        store
            .commit_transition(
                &claim,
                &TransitionBuilder::new(instance)
                    .effect_mutation(EffectMutation::terminal(
                        effect_id,
                        EffectTerminalState::Completed,
                    ))
                    .terminal_cleanup(TerminalCleanup::new(true, true, true))
                    .build(),
            )
            .await
            .unwrap();

        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_effects WHERE effect_id = $1")
                .bind(effect_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "completed");
        let inbox_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM workflow_inbox WHERE effect_id = $1")
                .bind(effect_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(inbox_count, 0);
    }

    #[tokio::test]
    async fn test_concurrent_start_command_commits_one_instance_with_full_lineage() {
        let (pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let tenant = TenantId::new("default").unwrap();
        let idempotency_key = Uuid::now_v7();
        let entry_id = Uuid::now_v7();
        let runbook_id = Uuid::now_v7();
        let first_id = Uuid::now_v7();
        let second_id = Uuid::now_v7();

        let make_start = |instance_id| {
            let mut instance = make_instance(instance_id);
            instance.entry_id = entry_id;
            instance.runbook_id = runbook_id;
            instance.correlation_id = "start-correlation".to_string();
            let command =
                StartCommand::builder(tenant.clone(), instance_id, instance.bytecode_version)
                    .process_key(instance.process_key.clone())
                    .lineage(entry_id, runbook_id)
                    .correlation_id(instance.correlation_id.clone())
                    .idempotency_key(idempotency_key)
                    .initial_payload(instance.domain_payload.to_string())
                    .session_stack(instance.session_stack.clone())
                    .logical_time(instance.created_at)
                    .build()
                    .unwrap();
            let transition = TransitionBuilder::new(instance)
                .upsert_fiber(Fiber::new(
                    EffectId::for_command(idempotency_key, 0, 0).as_uuid(),
                    0,
                ))
                .start_dedupe(StartDedupe::new(command))
                .build();
            (Claim::new(tenant.clone(), instance_id, 0, 0), transition)
        };
        let (first_claim, first_transition) = make_start(first_id);
        let (second_claim, second_transition) = make_start(second_id);
        let first_store = store.clone();
        let second_store = store.clone();
        let (first, second) = tokio::join!(
            first_store.commit_transition(&first_claim, &first_transition),
            second_store.commit_transition(&second_claim, &second_transition),
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CommitOutcome::Committed { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CommitOutcome::IdempotentNoOp))
                .count(),
            1
        );
        let winner = store
            .lookup_start_instance(&TenantId::new("default").unwrap(), idempotency_key)
            .await
            .unwrap()
            .unwrap();
        assert!(winner == first_id || winner == second_id);
        let aggregate_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_instances WHERE instance_id = ANY($1)",
        )
        .bind(vec![first_id, second_id])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(aggregate_count, 1);
        let lineage: (Uuid, Uuid, Vec<u8>, i16) = sqlx::query_as(
            "SELECT entry_id, runbook_id, initial_payload_hash, schema_version FROM bpmn_spawn_idempotency WHERE tenant_id = 'default' AND idempotency_key = $1",
        )
        .bind(idempotency_key)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(lineage.0, entry_id);
        assert_eq!(lineage.1, runbook_id);
        assert_eq!(
            lineage.2,
            EffectId::content_hash(br#"{"case_id":"abc"}"#).to_vec()
        );
        assert_eq!(
            lineage.3,
            i16::try_from(START_COMMAND_SCHEMA_VERSION).unwrap()
        );
        let snapshot = store
            .load_snapshot_envelope(&TenantId::new("default").unwrap(), winner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.schema_version(), SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.revision(), 0);
        assert_eq!(snapshot.state().instance().instance_id, winner);
        let journal = store
            .read_journal(&TenantId::new("default").unwrap(), winner, None)
            .await
            .unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].prior_revision(), -1);
        assert_eq!(journal[0].new_revision(), 0);
        assert_eq!(journal[0].command().command_type(), "start");
        assert_eq!(journal[0].state_hash(), snapshot.state_hash().unwrap());
    }

    #[tokio::test]
    async fn test_workflow_instances_is_sole_authority_and_plan_runtime_is_disabled() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        store
            .save_instance("authority-test", &make_instance(instance_id))
            .await
            .unwrap();
        let legacy_table: Option<String> =
            sqlx::query_scalar("SELECT to_regclass('public.bpmn_process_instance')::text")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(legacy_table.is_none());
        let rejected =
            sqlx::query("UPDATE workflow_instances SET plan_hash = $1 WHERE instance_id = $2")
                .bind(vec![1u8; 32])
                .bind(instance_id)
                .execute(&pool)
                .await;
        assert!(rejected.is_err());
    }

    #[tokio::test]
    async fn test_claim_load_quarantines_corrupt_snapshot_atomically() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        store
            .save_instance("integrity-fixture", &make_instance(instance_id))
            .await
            .unwrap();
        sqlx::query("UPDATE workflow_instances SET snapshot_envelope = $1 WHERE instance_id = $2")
            .bind(b"not-a-versioned-envelope".as_slice())
            .bind(instance_id)
            .execute(&pool)
            .await
            .unwrap();

        let result = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        assert!(matches!(result, Err(ClaimError::Integrity(_))));
        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(quarantine.as_deref(), Some("replay_integrity_violation"));
        let lease_owner: Option<String> =
            sqlx::query_scalar("SELECT lease_owner FROM workflow_instances WHERE instance_id = $1")
                .bind(instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(lease_owner.is_none());
    }

    /// V2.5 Ring 1: unlike `test_claim_load_quarantines_corrupt_snapshot_atomically`
    /// (whose fixture is garbage bytes that never even parse as JSON), this
    /// flips one byte in an otherwise well-formed, previously-committed
    /// `snapshot_envelope` while leaving `frame_hash` at its original
    /// correct value. Proves the pre-decode BLAKE3 comparison — not merely
    /// "decode failed" — is what fires, and fires atomically (quarantine
    /// state set, lease released) before `SnapshotEnvelope::decode` runs.
    #[tokio::test]
    async fn test_claim_load_quarantines_ring1_flipped_byte_under_stale_hash() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("integrity-fixture", &instance).await.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(&claim, &TransitionBuilder::new(instance).build())
            .await
            .unwrap();

        let mut envelope_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT snapshot_envelope FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        // Flip a byte inside the JSON payload; frame_hash column is left
        // untouched, so it now describes bytes that no longer exist.
        let flip_at = envelope_bytes.len() / 2;
        envelope_bytes[flip_at] ^= 0xFF;
        sqlx::query("UPDATE workflow_instances SET snapshot_envelope = $1 WHERE instance_id = $2")
            .bind(&envelope_bytes)
            .bind(instance_id)
            .execute(&pool)
            .await
            .unwrap();

        let result = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        match result {
            Err(ClaimError::Integrity(message)) => {
                assert!(
                    message.contains("Ring 1"),
                    "expected Ring 1 physical integrity violation, got: {message}"
                );
            }
            other => panic!("expected ClaimError::Integrity naming Ring 1, got {other:?}"),
        }
        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(quarantine.as_deref(), Some("replay_integrity_violation"));
        let lease_owner: Option<String> =
            sqlx::query_scalar("SELECT lease_owner FROM workflow_instances WHERE instance_id = $1")
                .bind(instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(lease_owner.is_none());
    }

    /// V6.1 isolate: once an instance is quarantined on a load-time integrity
    /// violation, a subsequent claim must not re-select it. Without the
    /// `quarantine_state IS NULL` claim predicate the poisoned instance is
    /// re-claimed → re-checked → re-quarantined every tick (fail-closed but
    /// never isolated). The second claim must return `Ok(None)`, not another
    /// `Err(Integrity)`.
    #[tokio::test]
    async fn test_quarantined_instance_is_not_reclaimed() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("integrity-fixture", &instance).await.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(&claim, &TransitionBuilder::new(instance).build())
            .await
            .unwrap();

        let mut envelope_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT snapshot_envelope FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let flip = envelope_bytes.len() / 2;
        envelope_bytes[flip] ^= 0xFF;
        sqlx::query("UPDATE workflow_instances SET snapshot_envelope = $1 WHERE instance_id = $2")
            .bind(&envelope_bytes)
            .bind(instance_id)
            .execute(&pool)
            .await
            .unwrap();

        // First claim detects, quarantines, and fails closed.
        let first = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        assert!(matches!(first, Err(ClaimError::Integrity(_))));

        // Second claim must find nothing to claim — the row is isolated.
        let second = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        assert!(
            matches!(second, Ok(None)),
            "a quarantined instance must not be re-claimed, got {second:?}"
        );
    }

    /// V6.1 surface: the claim-path quarantine must emit an
    /// `InstanceQuarantined` audit event, not merely set the column. Recovery
    /// is point-in-time restore, which an operator only initiates on seeing
    /// the event — a silent column set is invisible and never triggers it.
    #[tokio::test]
    async fn test_claim_path_quarantine_emits_audit_event() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("integrity-fixture", &instance).await.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(&claim, &TransitionBuilder::new(instance).build())
            .await
            .unwrap();

        let mut envelope_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT snapshot_envelope FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let flip = envelope_bytes.len() / 2;
        envelope_bytes[flip] ^= 0xFF;
        sqlx::query("UPDATE workflow_instances SET snapshot_envelope = $1 WHERE instance_id = $2")
            .bind(&envelope_bytes)
            .bind(instance_id)
            .execute(&pool)
            .await
            .unwrap();

        let result = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        assert!(matches!(result, Err(ClaimError::Integrity(_))));

        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event_log WHERE instance_id = $1 AND event::text LIKE '%InstanceQuarantined%'",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            event_count, 1,
            "claim-path quarantine must emit exactly one InstanceQuarantined audit event"
        );
    }

    /// V2.5 Ring 2: directly corrupts the journal head's `prior_state_hash`
    /// column so it no longer equals the previous record's `state_hash`,
    /// proving the chain-walk added in V2.3 (not merely the three-way
    /// snapshot/journal agreement) is what detects a broken hash chain.
    #[tokio::test]
    async fn test_claim_load_quarantines_ring2_chain_break() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("integrity-fixture", &instance).await.unwrap();

        // Two commits: genesis, then one more, so the journal head has a
        // real (non-zero, non-genesis) prior_state_hash to corrupt.
        let claim1 = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(
                &claim1,
                &TransitionBuilder::new(instance.clone()).build(),
            )
            .await
            .unwrap();
        let claim2 = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(&claim2, &TransitionBuilder::new(instance).build())
            .await
            .unwrap();

        let head_revision: i64 = sqlx::query_scalar(
            "SELECT new_revision FROM workflow_journal WHERE instance_id = $1 ORDER BY new_revision DESC LIMIT 1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(head_revision > 0, "need a non-genesis journal head to break its chain link");
        sqlx::query(
            "UPDATE workflow_journal SET prior_state_hash = $1 WHERE instance_id = $2 AND new_revision = $3",
        )
        .bind(vec![0xAB_u8; 32])
        .bind(instance_id)
        .bind(head_revision)
        .execute(&pool)
        .await
        .unwrap();

        let result = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        match result {
            Err(ClaimError::Integrity(message)) => {
                assert!(
                    message.contains("Ring 2"),
                    "expected Ring 2 frame integrity violation, got: {message}"
                );
            }
            other => panic!("expected ClaimError::Integrity naming Ring 2, got {other:?}"),
        }
        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(quarantine.as_deref(), Some("replay_integrity_violation"));
    }

    /// V2.5: DB-level tripwire test, distinct from `persistence.rs`'s
    /// unit-level `unknown_snapshot_version_is_refused` (which calls
    /// `SnapshotEnvelope::decode` directly, in-process). Corrupts the
    /// persisted `schema_version` field in a real committed row and
    /// recomputes `frame_hash` to match, proving that even a Ring
    /// 1-consistent (hash-correct) but Ring-1-tripwire-violating frame is
    /// rejected — the tripwire fires on the versioned value itself, not on
    /// whether the bytes hash-match (V&S §6: "a dispatch key, not a
    /// version").
    #[tokio::test]
    async fn test_claim_load_quarantines_wrong_schema_version_tripwire() {
        let (pool, store, _lock) = setup().await;
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("integrity-fixture", &instance).await.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(&claim, &TransitionBuilder::new(instance).build())
            .await
            .unwrap();

        let envelope_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT snapshot_envelope FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&envelope_bytes).unwrap();
        value["schema_version"] = serde_json::json!(99);
        let corrupted_bytes = serde_json::to_vec(&value).unwrap();
        let corrupted_frame_hash = *blake3::hash(&corrupted_bytes).as_bytes();
        sqlx::query(
            "UPDATE workflow_instances SET snapshot_envelope = $1, frame_hash = $2 WHERE instance_id = $3",
        )
        .bind(&corrupted_bytes)
        .bind(&corrupted_frame_hash[..])
        .bind(instance_id)
        .execute(&pool)
        .await
        .unwrap();

        let result = store
            .claim_instance_for_transition(
                &TenantId::new("default").unwrap(),
                instance_id,
                "recovery",
                30_000,
            )
            .await;
        match result {
            Err(ClaimError::Integrity(message)) => {
                assert!(
                    message.contains("unsupported") && message.contains("99"),
                    "expected unsupported-version tripwire naming version 99, got: {message}"
                );
            }
            other => panic!("expected ClaimError::Integrity naming the version tripwire, got {other:?}"),
        }
        let quarantine: Option<String> = sqlx::query_scalar(
            "SELECT quarantine_state FROM workflow_instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(quarantine.as_deref(), Some("replay_integrity_violation"));
    }

    // V2.5 remaining named corruption fixtures — dangling handle, membership
    // asymmetry, over-arity barrier, orphaned pending effect — are all D1
    // concurrency-table-shaped corruptions. Ring 1 cannot distinguish *which*
    // of these occurred: it is a single BLAKE3 hash over the whole frame, so
    // any single-bit flip inside the encoded `ConcurrencyTable`/pending-effects
    // region manifests identically to `test_claim_load_quarantines_ring1_flipped_byte_under_stale_hash`
    // above — "hash mismatch on raw bytes (pre-decode)", full stop. That
    // generic detection is what the byte-flip test above proves at the
    // Postgres layer, and what `flipping_any_byte_never_reproduces_the_original_value`
    // (bpmn-lite-types/src/canonical.rs) proves exhaustively at the encoding
    // layer: no corruption of the canonical-binary region survives re-encoding
    // undetected. Semantic distinction between these four cases — naming
    // *which* invariant broke, not just that the frame no longer hashes
    // correctly — is Ring 3's job (runtime shadow asserts, V4), per the plan's
    // explicit ring split (V&S §6). Writing four more byte-flip tests here
    // would assert the same mechanism four more times without adding
    // detection coverage; the plan permits deferring the semantic split to V4.

    #[tokio::test]
    async fn test_transient_effect_failure_retains_effect_without_advancing_instance() {
        let (pool, store, _lock) = setup().await;
        let tenant_id = "default";
        let instance_id = Uuid::now_v7();
        let instance = make_instance(instance_id);
        store.save_instance("fixture", &instance).await.unwrap();

        let effect_id = EffectId::for_transition(instance_id, 1, 0);
        let fiber_id = Uuid::now_v7();
        let effect = DurableEffect::Invoke {
            effect_id,
            fiber_id,
            pc: 0,
            operation: "fixture.transient".to_string(),
            template_id: [9; 32],
            input: br#"{"input":2}"#.to_vec(),
            idempotency_key: effect_id.as_uuid().to_string(),
        };
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "apply",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(
                &claim,
                &TransitionBuilder::new(instance).effect(effect).build(),
            )
            .await
            .unwrap();

        let revision_before: i64 = sqlx::query_scalar(
            "SELECT revision FROM workflow_instances WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(tenant_id)
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
        let mut claimed = store
            .claim_pending_effects(
                &TenantId::new(tenant_id).unwrap(),
                "dispatcher",
                now,
                1,
                30_000,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        let claimed = claimed.pop().unwrap();
        let due_at = now + 5_000;
        store
            .schedule_effect_retry(
                &claimed,
                RetryDecision::At { attempt: 1, due_at },
                "lock timeout",
            )
            .await
            .unwrap();

        let (state, terminal, attempt, stored_due_at, last_error): (
            String,
            bool,
            i32,
            chrono::DateTime<chrono::Utc>,
            String,
        ) = sqlx::query_as(
            "SELECT state, terminal, attempt, next_due_at, last_error FROM workflow_effects WHERE effect_id = $1",
        )
        .bind(effect_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "pending");
        assert!(!terminal);
        assert_eq!(attempt, 1);
        assert_eq!(
            stored_due_at.timestamp_millis(),
            i64::try_from(due_at).unwrap()
        );
        assert_eq!(last_error, "lock timeout");

        let revision_after: i64 = sqlx::query_scalar(
            "SELECT revision FROM workflow_instances WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(tenant_id)
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(revision_after, revision_before);
        assert!(store
            .load_effect_responses(&TenantId::new(tenant_id).unwrap(), 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_submission_ack_commits_outbox_effect_pending_and_instance_atomically() {
        let (pool, store, _lock) = setup().await;
        let tenant_id = "default";
        let instance_id = Uuid::now_v7();
        let callout_id = Uuid::now_v7();
        let outbox_id = Uuid::now_v7();
        let idempotency_key = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        instance.state = ProcessState::WaitingOnSubmission {
            callout_id,
            node_id: "service-a".to_string(),
        };
        store.save_instance("fixture", &instance).await.unwrap();
        let submitted_at = chrono::Utc::now().timestamp_millis();
        let pending = PendingInvocationWrite::new(
            PendingInvocationIdentity {
                callout_id,
                process_instance_id: instance_id,
                node_id: "service-a".to_string(),
                target_domain: "peer".to_string(),
                verb_id: "peer.do".to_string(),
                idempotency_key,
            },
            PendingInvocationTiming {
                execution_id: None,
                submitted_at,
                ack_received_at: None,
                timeout_at: None,
            },
        );
        let outbox = OutboxWrite::new(
            outbox_id,
            "peer".to_string(),
            "invocation".to_string(),
            b"request".to_vec(),
            idempotency_key,
            callout_id,
        );
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "submit",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        store
            .commit_transition(
                &claim,
                &TransitionBuilder::new(instance.clone())
                    .pending_invocation(pending)
                    .outbox(outbox)
                    .build(),
            )
            .await
            .unwrap();

        let mut dispatch_tx = pool.begin().await.unwrap();
        PostgresWorkflowStore::set_tenant_context(&mut dispatch_tx, tenant_id)
            .await
            .unwrap();
        let claimed = dsl_bus_storage::claim_pending_outbox(
            &mut dispatch_tx,
            1,
            "dispatcher",
            Uuid::now_v7(),
            chrono::Utc::now() + chrono::Duration::seconds(30),
        )
        .await
        .unwrap();
        dispatch_tx.commit().await.unwrap();
        assert_eq!(claimed.len(), 1);
        let execution_id = Uuid::now_v7();
        let dispatch_claim_token = claimed[0].claim_token.unwrap();

        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "ack",
                30_000,
            )
            .await
            .unwrap()
            .unwrap();
        instance.state = ProcessState::WaitingOnInvocation {
            execution_id,
            node_id: "service-a".to_string(),
        };
        store
            .commit_transition(
                &claim,
                &TransitionBuilder::new(instance)
                    .bus_submission_ack(BusSubmissionAckMutation::new(
                        outbox_id,
                        callout_id,
                        execution_id,
                        dispatch_claim_token,
                    ))
                    .build(),
            )
            .await
            .unwrap();

        let outbox_state: (String, Option<Uuid>) =
            sqlx::query_as("SELECT status, execution_id FROM dsl_bus.outbox WHERE id = $1")
                .bind(outbox_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(outbox_state, ("submitted".to_string(), Some(execution_id)));
        let pending_execution: Option<Uuid> = sqlx::query_scalar(
            "SELECT execution_id FROM bpmn_pending_invocation WHERE callout_id = $1",
        )
        .bind(callout_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending_execution, Some(execution_id));
        let effect_state: (String, Option<Uuid>) =
            sqlx::query_as("SELECT state, execution_id FROM workflow_effects WHERE effect_id = $1")
                .bind(callout_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(effect_state, ("accepted".to_string(), Some(execution_id)));
        let loaded = store
            .load_instance(&TenantId::new("default").unwrap(), instance_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            loaded.state,
            ProcessState::WaitingOnInvocation { execution_id: current, .. }
                if current == execution_id
        ));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    impl PostgresWorkflowStore {
        async fn save_instance(&self, lease_owner: &str, instance: &ProcessInstance) -> Result<()> {
            self.ensure_tenant(
                &TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?,
            )
            .await
            .persistence()?;
            if self
                .load_instance(&TenantId::new("default").unwrap(), instance.instance_id)
                .await
                .persistence()?
                .is_none()
            {
                let tenant =
                    TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?;
                self.commit_transition(
                    &Claim::new(tenant, instance.instance_id, 0, 0),
                    &TransitionBuilder::new(instance.clone()).build(),
                )
                .await
                .map(|_| ())
                .map_err(StoreError::integrity)
            } else {
                let claim = self
                    .claim_instance_for_transition(
                        &TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?,
                        instance.instance_id,
                        lease_owner,
                        30_000,
                    )
                    .await
                    .persistence()?
                    .ok_or_else(|| {
                        StoreError::Integrity("fixture instance is leased".to_string())
                    })?;
                let result = self
                    .commit_transition(&claim, &TransitionBuilder::new(instance.clone()).build())
                    .await
                    .map(|_| ())
                    .map_err(StoreError::integrity);
                self.release_instance_transition(
                    &TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?,
                    instance.instance_id,
                    lease_owner,
                )
                .await
                .persistence()?;
                result
            }
        }
        async fn update_instance_state(
            &self,
            tenant_id: &str,
            lease_owner: &str,
            id: Uuid,
            state: ProcessState,
        ) -> Result<()> {
            let tenant_id = tenant_id.to_string();
            let lease_owner = lease_owner.to_string();
            self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            let state_json = serde_json::to_value(&state).persistence()?;
            let result = sqlx::query(
                "UPDATE workflow_instances SET state = $1 WHERE instance_id = $2 AND lease_owner = $3",
            )
            .bind(&state_json)
            .bind(id)
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await.persistence()?;

            tx.assert_rows_affected(&result, 1, "update_instance_state")
        })).await
        }
        async fn update_instance_flags(
            &self,
            tenant_id: &str,
            lease_owner: &str,
            id: Uuid,
            flags: &BTreeMap<FlagKey, Value>,
        ) -> Result<()> {
            let tenant_id = tenant_id.to_string();
            let lease_owner = lease_owner.to_string();
            let flags = flags.clone();
            self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            let flags_json = serde_json::to_value(&flags).persistence()?;
            let result = sqlx::query(
                "UPDATE workflow_instances SET flags = $1 WHERE instance_id = $2 AND lease_owner = $3",
            )
            .bind(&flags_json)
            .bind(id)
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await.persistence()?;

            tx.assert_rows_affected(&result, 1, "update_instance_flags")
        })).await
        }
        async fn update_instance_payload(
            &self,
            tenant_id: &str,
            lease_owner: &str,
            id: Uuid,
            payload: &str,
            hash: &[u8; 32],
        ) -> Result<()> {
            let tenant_id = tenant_id.to_string();
            let lease_owner = lease_owner.to_string();
            let payload = payload.to_string();
            let hash = *hash;
            self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            let result = sqlx::query(
                "UPDATE workflow_instances SET domain_payload = $1, domain_payload_hash = $2 WHERE instance_id = $3 AND lease_owner = $4",
            )
            .bind(&payload)
            .bind(&hash[..])
            .bind(id)
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await.persistence()?;

            tx.assert_rows_affected(&result, 1, "update_instance_payload")
        })).await
        }
        async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            let stack = serde_json::to_value(&fiber.stack).persistence()?;
            let regs = serde_json::to_value(Vec::<bpmn_lite_types::Value>::new()).persistence()?;
            let wait_state = serde_json::to_value(&fiber.wait).persistence()?;

            let result = sqlx::query(
            r#"
            INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (instance_id, fiber_id) DO UPDATE SET
                pc = EXCLUDED.pc,
                stack = EXCLUDED.stack,
                regs = EXCLUDED.regs,
                wait_state = EXCLUDED.wait_state,
                loop_epoch = EXCLUDED.loop_epoch
            "#,
        )
        .bind(instance_id)
        .bind(fiber.fiber_id)
        .bind(fiber.pc.get() as i32)
        .bind(&stack)
        .bind(&regs)
        .bind(&wait_state)
        .bind(fiber.loop_epoch as i32)
        .bind(&tenant_id)
        .execute(&mut *tx)
        .await.persistence()?;

            // A18 — rows_affected validation. INSERT ... ON CONFLICT DO UPDATE
            // must touch exactly one row. Zero means RLS rejection on the
            // parent instance, or the parent instance was deleted concurrently.
            if result.rows_affected() == 0 {
                return Err(StoreError::Integrity(format!(
                    "save_fiber affected 0 rows for instance {} fiber {}; \
                 parent instance may be missing or RLS rejected",
                    instance_id, fiber.fiber_id
                )));
            }

            tx.commit().await.persistence()?;
            Ok(())
        }
        async fn delete_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<()> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            sqlx::query("DELETE FROM fibers WHERE instance_id = $1 AND fiber_id = $2")
                .bind(instance_id)
                .bind(fiber_id)
                .execute(&mut *tx)
                .await
                .persistence()?;

            tx.commit().await.persistence()?;
            Ok(())
        }
        async fn delete_all_fibers(&self, instance_id: Uuid) -> Result<()> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            sqlx::query("DELETE FROM fibers WHERE instance_id = $1")
                .bind(instance_id)
                .execute(&mut *tx)
                .await
                .persistence()?;

            tx.commit().await.persistence()?;
            Ok(())
        }
        async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            let row = sqlx::query(
                r#"
            INSERT INTO join_barriers (instance_id, join_id, arrive_count, tenant_id)
            VALUES ($1, $2, 1, $3)
            ON CONFLICT (instance_id, join_id) DO UPDATE
                SET arrive_count = join_barriers.arrive_count + 1
            RETURNING arrive_count
            "#,
            )
            .bind(instance_id)
            .bind(join_id as i32)
            .bind(&tenant_id)
            .fetch_one(&mut *tx)
            .await
            .persistence()?;

            use sqlx::Row;
            let count: i16 = row.get("arrive_count");
            tx.commit().await.persistence()?;
            Ok(count as u16)
        }
        async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            sqlx::query(
                r#"
            INSERT INTO join_barriers (instance_id, join_id, arrive_count, tenant_id)
            VALUES ($1, $2, 0, $3)
            ON CONFLICT (instance_id, join_id) DO UPDATE
                SET arrive_count = 0
            "#,
            )
            .bind(instance_id)
            .bind(join_id as i32)
            .bind(&tenant_id)
            .execute(&mut *tx)
            .await
            .persistence()?;

            tx.commit().await.persistence()?;
            Ok(())
        }
        async fn join_delete_all(&self, instance_id: Uuid) -> Result<()> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            sqlx::query("DELETE FROM join_barriers WHERE instance_id = $1")
                .bind(instance_id)
                .execute(&mut *tx)
                .await
                .persistence()?;

            tx.commit().await.persistence()?;
            Ok(())
        }
        async fn dedupe_put(
            &self,
            tenant_id: &str,
            key: &str,
            completion: &JobCompletion,
        ) -> Result<()> {
            let json = serde_json::to_value(completion).persistence()?;
            let tenant_id_str = tenant_id.to_string();
            let key = key.to_string();
            self.with_tenant(tenant_id, |tx| Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO dedupe_cache (job_key, completion, tenant_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (job_key) DO UPDATE SET completion = EXCLUDED.completion, tenant_id = EXCLUDED.tenant_id
                "#,
            )
            .bind(&key)
            .bind(&json)
            .bind(&tenant_id_str)
            .execute(&mut **tx)
            .await.persistence()?;
            Ok(())
        })).await
        }
        async fn enqueue_job(&self, activation: &JobActivation) -> Result<()> {
            let lease_owner = "unused";
            let tenant_id = activation.tenant_id.clone();
            let activation = activation.clone();
            self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
                Box::pin(async move { Self::enqueue_job_inner(tx, &activation).await })
            })
            .await
        }
        async fn ack_job(&self, tenant_id: &str, job_key: &str) -> Result<()> {
            let lease_owner = "unused";
            let job_key = job_key.to_string();
            let tenant_id = tenant_id.to_string();
            self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
                Box::pin(async move { Self::ack_job_inner(tx, &job_key).await })
            })
            .await
        }
        async fn cancel_jobs_for_instance(&self, instance_id: Uuid) -> Result<Vec<String>> {
            let tenant_id = "default".to_string();
            let lease_owner = "unused";
            self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
                Box::pin(async move { Self::cancel_jobs_for_instance_inner(tx, instance_id).await })
            })
            .await
        }
        async fn atomic_consume_buffered_message(
            &self,
            instance: &ProcessInstance,
            fiber: &Fiber,
            message: &ClaimedBufferedMessage,
            payload_update: Option<&PayloadUpdate>,
            events: &[RuntimeEvent],
        ) -> Result<bool> {
            let lease_owner = "unused";
            let tenant_id = instance.tenant_id.clone();
            if tenant_id != message.message.tenant_id {
                return Err(StoreError::Integrity(
                    "tenant_id mismatch: cross-tenant message consumption blocked".to_string(),
                ));
            }
            let instance = instance.clone();
            let fiber = fiber.clone();
            let message = message.clone();
            let payload_update = payload_update.cloned();
            let events = events.to_vec();
            self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| Box::pin(async move {
            let result = sqlx::query(
                r#"
                UPDATE message_buffer
                SET consumed_at = now(),
                    consumed_by_instance_id = $5,
                    consumed_by_fiber_id = $6,
                    status = 'consumed'
                WHERE tenant_id = $1
                  AND message_name = $2
                  AND correlation_key = $3
                  AND msg_id = $4
                  AND claim_token = $7
                  AND claim_until = $8
                  AND consumed_at IS NULL
                "#,
            )
            .bind(&message.message.tenant_id)
            .bind(&message.message.message_name)
            .bind(&message.message.correlation_key)
            .bind(&message.message.msg_id)
            .bind(instance.instance_id)
            .bind(fiber.fiber_id)
            .bind(&message.claim_token)
            .bind(epoch_ms_to_datetime(message.claim_until))
            .execute(&mut *tx.tx)
            .await.persistence()?;

            if result.rows_affected() != 1 {
                return Ok(false);
            }

            let payload = payload_update
                .as_ref()
                .map(|payload_update| payload_update.payload.as_str())
                .unwrap_or(instance.domain_payload.as_ref());
            let payload_hash = payload_update
                .as_ref()
                .map(|payload_update| payload_update.payload_hash)
                .unwrap_or(instance.domain_payload_hash);

            let flags = serde_json::to_value(&instance.flags).persistence()?;
            let counters = serde_json::to_value(&instance.counters).persistence()?;
            let join_expected = serde_json::to_value(&instance.join_expected).persistence()?;
            let state = serde_json::to_value(&instance.state).persistence()?;

            let workflow_instances_result = sqlx::query(
                r#"
                UPDATE workflow_instances
                SET domain_payload = $2,
                    domain_payload_hash = $3,
                    flags = $4,
                    counters = $5,
                    join_expected = $6,
                    state = $7
                WHERE instance_id = $1
                "#,
            )
            .bind(instance.instance_id)
            .bind(payload)
            .bind(&payload_hash[..])
            .bind(&flags)
            .bind(&counters)
            .bind(&join_expected)
            .bind(&state)
            .execute(&mut *tx.tx)
            .await.persistence()?;

            tx.assert_rows_affected(&workflow_instances_result, 1, "atomic_consume_buffered_message: workflow_instances update")?;

            if let Some(payload_update) = &payload_update {
                sqlx::query(
                    r#"
                    INSERT INTO payload_history (instance_id, payload_hash, domain_payload, tenant_id)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (instance_id, payload_hash) DO NOTHING
                    "#,
                )
                .bind(instance.instance_id)
                .bind(&payload_update.payload_hash[..])
                .bind(&payload_update.payload)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await.persistence()?;
            }

            let stack = serde_json::to_value(&fiber.stack).persistence()?;
            let regs = serde_json::to_value(Vec::<bpmn_lite_types::Value>::new()).persistence()?;
            let wait_state = serde_json::to_value(&fiber.wait).persistence()?;

            sqlx::query(
                r#"
                INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (instance_id, fiber_id) DO UPDATE SET
                    pc = EXCLUDED.pc,
                    stack = EXCLUDED.stack,
                    regs = EXCLUDED.regs,
                    wait_state = EXCLUDED.wait_state,
                    loop_epoch = EXCLUDED.loop_epoch
                "#,
            )
            .bind(instance.instance_id)
            .bind(fiber.fiber_id)
            .bind(fiber.pc.get() as i32)
            .bind(&stack)
            .bind(&regs)
            .bind(&wait_state)
            .bind(fiber.loop_epoch as i32)
            .bind(&tx.tenant_id)
            .execute(&mut *tx.tx)
            .await.persistence()?;

            for event in &events {
                let event_json = serde_json::to_value(event).persistence()?;
                sqlx::query(
                    r#"
                    WITH seq AS (
                        INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                        VALUES ($1, 1, $3)
                        ON CONFLICT (instance_id) DO UPDATE
                            SET next_seq = event_sequences.next_seq + 1
                        RETURNING next_seq, tenant_id
                    )
                    INSERT INTO event_log (instance_id, seq, event, tenant_id)
                    SELECT $1, seq.next_seq, $2, seq.tenant_id
                    FROM seq
                    "#,
                )
                .bind(instance.instance_id)
                .bind(&event_json)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await.persistence()?;
            }

            if !events.is_empty() {
                notify_event_tx(&mut tx.tx, instance.instance_id).await.persistence()?;
            }
            Ok(true)
        })).await
        }
        async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;
            let event_json = serde_json::to_value(event).persistence()?;

            let row = sqlx::query(
                r#"
            WITH seq AS (
                INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                VALUES ($1, 1, $3)
                ON CONFLICT (instance_id) DO UPDATE
                    SET next_seq = event_sequences.next_seq + 1
                RETURNING next_seq
            )
            INSERT INTO event_log (instance_id, seq, event, tenant_id)
            SELECT $1, seq.next_seq, $2, $3
            FROM seq
            RETURNING seq
            "#,
            )
            .bind(instance_id)
            .bind(&event_json)
            .bind(&tenant_id)
            .fetch_one(&mut *tx)
            .await
            .persistence()?;

            use sqlx::Row;
            let seq: i64 = row.get("seq");
            notify_event_tx(&mut tx, instance_id).await.persistence()?;
            tx.commit().await.persistence()?;
            Ok(seq as u64)
        }
        async fn save_payload_version(
            &self,
            instance_id: Uuid,
            hash: &[u8; 32],
            payload: &str,
        ) -> Result<()> {
            let tenant_id = "default".to_string();
            let mut tx = self.pool.begin().await.persistence()?;
            Self::set_tenant_context(&mut tx, &tenant_id)
                .await
                .persistence()?;

            sqlx::query(
                r#"
            INSERT INTO payload_history (instance_id, payload_hash, domain_payload, tenant_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (instance_id, payload_hash) DO NOTHING
            "#,
            )
            .bind(instance_id)
            .bind(&hash[..])
            .bind(payload)
            .bind(&tenant_id)
            .execute(&mut *tx)
            .await
            .persistence()?;

            tx.commit().await.persistence()?;
            Ok(())
        }
        async fn save_incident(&self, incident: &Incident) -> Result<()> {
            let tenant_id = "default".to_string();
            let lease_owner = "unused";
            let incident = incident.clone();
            self.execute_tenant_scoped(&tenant_id, lease_owner, |tx| {
                Box::pin(async move { Self::save_incident_inner(tx, &incident).await })
            })
            .await
        }
        async fn quarantine_instance(
            &self,
            instance_id: Uuid,
            tenant_id: &str,
            lease_owner: &str,
            detection_point: &str,
        ) -> Result<()> {
            let lease_owner = lease_owner.to_string();
            let tenant_id_str = tenant_id.to_string();
            let detection_point_str = detection_point.to_string();
            self.execute_tenant_scoped(&tenant_id_str, &lease_owner, |tx| {
                Box::pin(async move {
                    Self::quarantine_instance_inner(tx, instance_id).await.persistence()?;

                    // 2. Append InstanceQuarantined event to the audit log.
                    let now = chrono::Utc::now();
                    let now_ms = now.timestamp_millis();
                    let event = RuntimeEvent::InstanceQuarantined {
                        instance_id,
                        tenant_id: tx.tenant_id.clone(),
                        detection_point: detection_point_str,
                        failure_reason: "integrity_hash_mismatch".to_string(),
                        detected_at: now_ms,
                    };
                    let event_json = serde_json::to_value(&event).persistence()?;

                    sqlx::query(
                        r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
                    VALUES ($1, 1, $3)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq, tenant_id
                )
                INSERT INTO event_log (instance_id, seq, event, tenant_id)
                SELECT $1, seq.next_seq, $2, seq.tenant_id
                FROM seq
                "#,
                    )
                    .bind(instance_id)
                    .bind(&event_json)
                    .bind(&tx.tenant_id)
                    .execute(&mut *tx.tx)
                    .await
                    .map_err(|error| StoreError::Unavailable(format!("quarantine_instance: failed to append InstanceQuarantined event: {error}")))?;

                    notify_event_tx(&mut tx.tx, instance_id).await.persistence()?;
                    Ok(())
                })
            })
            .await.persistence()?;

            tracing::warn!(
                instance_id = %instance_id,
                tenant_id = %tenant_id,
                detection_point = %detection_point,
                "A19: instance quarantined due to integrity hash mismatch"
            );

            Ok(())
        }
        async fn enqueue_job_inner(
            tx: &mut TenantTx<'_>,
            activation: &JobActivation,
        ) -> Result<()> {
            let orch_flags = serde_json::to_value(&activation.orch_flags).persistence()?;
            let session_stack = serde_json::to_value(&activation.session_stack).persistence()?;

            let result = sqlx::query(
            r#"
            INSERT INTO job_queue (
                job_key, tenant_id, process_instance_id, task_type, service_task_id,
                domain_payload, domain_payload_hash, session_stack, orch_flags, retries_remaining,
                entry_id, runbook_id
            ) VALUES ($1, (SELECT tenant_id FROM workflow_instances WHERE instance_id = $2), $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (job_key) DO NOTHING
            "#,
        )
        .bind(&activation.job_key)
        .bind(activation.process_instance_id)
        .bind(&activation.task_type)
        .bind(&activation.service_task_id)
        .bind(&activation.domain_payload)
        .bind(&activation.domain_payload_hash[..])
        .bind(&session_stack)
        .bind(&orch_flags)
        .bind(activation.retries_remaining as i32)
        .bind(activation.entry_id)
        .bind(activation.runbook_id)
        .execute(&mut *tx.tx)
        .await.persistence()?;

            if result.rows_affected() == 0 {
                let existing: Option<(String,)> =
                    sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = $1")
                        .bind(&activation.job_key)
                        .fetch_optional(&mut *tx.tx)
                        .await
                        .persistence()?;

                if existing.is_none() {
                    return Err(StoreError::Integrity(format!(
                        "enqueue_job affected 0 rows for job {} (instance {}); \
                     parent instance missing, RLS rejected, or NOT NULL \
                     constraint violation on tenant_id",
                        activation.job_key, activation.process_instance_id
                    )));
                }
                tracing::debug!(
                    job_key = %activation.job_key,
                    "enqueue_job: duplicate job_key, idempotent no-op"
                );
            }

            Ok(())
        }
        async fn ack_job_inner(tx: &mut TenantTx<'_>, job_key: &str) -> Result<()> {
            let result = sqlx::query("DELETE FROM job_queue WHERE job_key = $1")
                .bind(job_key)
                .execute(&mut *tx.tx)
                .await
                .persistence()?;

            if result.rows_affected() == 0 {
                tracing::debug!(
                    job_key = %job_key,
                    "ack_job: 0 rows deleted (already acked, expired, or cancelled)"
                );
            }

            Ok(())
        }
        async fn cancel_jobs_for_instance_inner(
            tx: &mut TenantTx<'_>,
            instance_id: Uuid,
        ) -> Result<Vec<String>> {
            let rows = sqlx::query(
            "DELETE FROM job_queue WHERE process_instance_id = $1 AND status IN ('pending', 'claimed') RETURNING job_key",
        )
        .bind(instance_id)
        .fetch_all(&mut *tx.tx)
        .await.persistence()?;

            use sqlx::Row;
            Ok(rows.iter().map(|r| r.get("job_key")).collect())
        }
        async fn quarantine_instance_inner(tx: &mut TenantTx<'_>, instance_id: Uuid) -> Result<()> {
            let result = sqlx::query(
                "UPDATE workflow_instances \
             SET quarantine_state = 'integrity_violation' \
             WHERE instance_id = $1",
            )
            .bind(instance_id)
            .execute(&mut *tx.tx)
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "quarantine_instance: failed to set quarantine_state: {error}"
                ))
            })?;

            tx.assert_rows_affected(&result, 1, "quarantine_instance")
        }
        async fn save_incident_inner(tx: &mut TenantTx<'_>, incident: &Incident) -> Result<()> {
            let error_class = serde_json::to_value(&incident.error_class).persistence()?;
            let created_at = epoch_ms_to_datetime(incident.created_at);
            let resolved_at = incident.resolved_at.map(epoch_ms_to_datetime);

            let result = sqlx::query(
                r#"
            INSERT INTO incidents (
                incident_id, process_instance_id, fiber_id, service_task_id,
                bytecode_addr, error_class, message, retry_count,
                created_at, resolved_at, resolution, tenant_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
            )
            .bind(incident.incident_id)
            .bind(incident.process_instance_id)
            .bind(incident.fiber_id)
            .bind(&incident.service_task_id)
            .bind(incident.bytecode_addr.get() as i32)
            .bind(&error_class)
            .bind(&incident.message)
            .bind(incident.retry_count as i32)
            .bind(created_at)
            .bind(resolved_at)
            .bind(&incident.resolution)
            .bind(&tx.tenant_id)
            .execute(&mut *tx.tx)
            .await
            .persistence()?;

            tx.assert_rows_affected(&result, 1, "save_incident")
        }
    }
}
