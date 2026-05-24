use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::integrity::compute_instance_integrity_hash;
use bpmn_lite_types::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const EVENT_NOTIFY_CHANNEL: &str = "bpmn_lite_events";

/// Serialize a `Value` into a deterministic string key for dead-letter lookup.
/// Must match MemoryStore's `value_key()` exactly.
fn value_key(v: &Value) -> String {
    match v {
        Value::Bool(b) => format!("b:{b}"),
        Value::I64(n) => format!("i:{n}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Ref(r) => format!("r:{r}"),
    }
}

/// Deserialize a JSONB `Vec<Value>` into `[Value; 8]`, padding with `Value::Bool(false)` if short.
fn regs_from_json(json: serde_json::Value) -> Result<[Value; 8]> {
    let vec: Vec<Value> =
        serde_json::from_value(json).context("failed to deserialize fiber regs")?;
    if vec.len() > 8 {
        return Err(anyhow!(
            "fiber regs has {} elements, expected <= 8",
            vec.len()
        ));
    }
    let mut regs: [Value; 8] = std::array::from_fn(|_| Value::Bool(false));
    for (i, v) in vec.into_iter().enumerate() {
        regs[i] = v;
    }
    Ok(regs)
}

/// Convert a `[u8; 32]` BYTEA column loaded as `Vec<u8>` back to `[u8; 32]`.
fn bytes_to_hash(bytes: Vec<u8>) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("expected 32 bytes, got {}", v.len()))
}

/// Convert an epoch-ms i64 to a `chrono::DateTime<chrono::Utc>` for TIMESTAMPTZ binding.
fn epoch_ms_to_datetime(epoch_ms: i64) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    let secs = epoch_ms / 1000;
    let nanos = ((epoch_ms % 1000) * 1_000_000) as u32;
    chrono::Utc
        .timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(chrono::Utc::now)
}

fn datetime_to_epoch_ms(dt: chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp_millis()
}

/// PostgreSQL-backed implementation of `ProcessStore`.
pub struct PostgresProcessStore {
    pool: sqlx::PgPool,
}

impl PostgresProcessStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// A18 — Execute `f` inside a transaction with `app.tenant_id` set via
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
        let mut tx = self
            .pool
            .begin()
            .await
            .context("with_tenant: begin transaction")?;
        Self::set_tenant_context(&mut tx, tenant_id).await?;
        let result = f(&mut tx).await?;
        tx.commit()
            .await
            .context("with_tenant: commit transaction")?;
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
            .context("failed to run bpmn-lite migrations")?;
        Ok(())
    }

    /// A16 — Set the tenant context for the current transaction.
    ///
    /// Call `SET LOCAL app.tenant_id = <tenant>` at the start of each
    /// transaction so that Row-Level Security policies can filter rows.
    /// `SET LOCAL` scopes the setting to the current transaction only;
    /// it is reset automatically when the transaction commits or rolls back.
    ///
    /// Usage: call this immediately after beginning a transaction, before
    /// any data query. Without this, RLS policies using
    /// `current_setting('app.tenant_id', true)` will return NULL and
    /// no rows will be visible.
    pub async fn set_tenant_context(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant_id: &str,
    ) -> Result<()> {
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant_id)
            .execute(tx.as_mut())
            .await
            .context("failed to set tenant context for RLS")?;
        Ok(())
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
        .await?;
    Ok(())
}

#[async_trait]
impl ProcessStore for PostgresProcessStore {
    // ── Instance ──

    async fn save_instance(&self, instance: &ProcessInstance) -> Result<()> {
        let flags = serde_json::to_value(&instance.flags)?;
        let counters = serde_json::to_value(&instance.counters)?;
        let join_expected = serde_json::to_value(&instance.join_expected)?;
        let state = serde_json::to_value(&instance.state)?;
        let session_stack = serde_json::to_value(&instance.session_stack)?;
        let created_at = epoch_ms_to_datetime(instance.created_at);
        // A19 — compute integrity hash over immutable fields at creation.
        // integrity_hash is excluded from the ON CONFLICT DO UPDATE clause so
        // it is written once and never overwritten by subsequent updates.
        let integrity_hash = compute_instance_integrity_hash(instance);

        let result = sqlx::query(
            r#"
            INSERT INTO process_instances (
                instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                domain_payload_hash, session_stack, flags, counters, join_expected, state,
                correlation_id, entry_id, runbook_id, created_at, integrity_hash,
                plan_hash, current_node_id, placeholder_values
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                      $17, $18, $19)
            ON CONFLICT (instance_id) DO UPDATE SET
                domain_payload = EXCLUDED.domain_payload,
                domain_payload_hash = EXCLUDED.domain_payload_hash,
                session_stack = EXCLUDED.session_stack,
                flags = EXCLUDED.flags,
                counters = EXCLUDED.counters,
                join_expected = EXCLUDED.join_expected,
                state = EXCLUDED.state,
                correlation_id = EXCLUDED.correlation_id,
                plan_hash = EXCLUDED.plan_hash,
                current_node_id = EXCLUDED.current_node_id,
                placeholder_values = EXCLUDED.placeholder_values
            -- Immutable fields (tenant_id, process_key, bytecode_version, entry_id,
            -- runbook_id, created_at, integrity_hash) are omitted: migration 029
            -- trigger rejects any UPDATE that changes them. quarantine_state is
            -- owned exclusively by quarantine_instance().
            "#,
        )
        .bind(instance.instance_id)
        .bind(&instance.tenant_id)
        .bind(&instance.process_key)
        .bind(&instance.bytecode_version[..])
        .bind(instance.domain_payload.as_ref())
        .bind(&instance.domain_payload_hash[..])
        .bind(&session_stack)
        .bind(&flags)
        .bind(&counters)
        .bind(&join_expected)
        .bind(&state)
        .bind(&instance.correlation_id)
        .bind(instance.entry_id)
        .bind(instance.runbook_id)
        .bind(created_at)
        .bind(&integrity_hash[..])
        .bind(instance.plan_hash.as_ref().map(|h| h.as_slice()))
        .bind(instance.current_node_id.as_deref())
        .bind(instance.placeholder_values.as_ref())
        .execute(&self.pool)
        .await?;

        // A18 — rows_affected validation. INSERT ... ON CONFLICT DO UPDATE
        // must touch exactly one row. Zero means RLS rejection, missing
        // parent FK, or other silent failure.
        if result.rows_affected() == 0 {
            return Err(anyhow!(
                "save_instance affected 0 rows for instance {} (tenant={}); \
                 possible RLS rejection or constraint violation",
                instance.instance_id,
                instance.tenant_id
            ));
        }

        Ok(())
    }

    async fn load_instance(&self, id: Uuid) -> Result<Option<ProcessInstance>> {
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
            FROM process_instances
            WHERE instance_id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

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
                    domain_payload: Arc::<str>::from(row.get::<String, _>("domain_payload")),
                    domain_payload_hash: bytes_to_hash(domain_payload_hash)?,
                    session_stack: serde_json::from_value(session_stack_json)?,
                    flags: serde_json::from_value(flags_json)?,
                    counters: serde_json::from_value(counters_json)?,
                    join_expected: serde_json::from_value(join_expected_json)?,
                    state: serde_json::from_value(state_json)?,
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
    }

    async fn update_instance_state(&self, id: Uuid, state: ProcessState) -> Result<()> {
        let state_json = serde_json::to_value(&state)?;
        let result = sqlx::query("UPDATE process_instances SET state = $1 WHERE instance_id = $2")
            .bind(&state_json)
            .bind(id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("instance not found: {id}"));
        }
        Ok(())
    }

    async fn update_instance_flags(
        &self,
        id: Uuid,
        flags: &BTreeMap<FlagKey, Value>,
    ) -> Result<()> {
        let flags_json = serde_json::to_value(flags)?;
        let result = sqlx::query("UPDATE process_instances SET flags = $1 WHERE instance_id = $2")
            .bind(&flags_json)
            .bind(id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("instance not found: {id}"));
        }
        Ok(())
    }

    async fn update_instance_payload(
        &self,
        id: Uuid,
        payload: &str,
        hash: &[u8; 32],
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE process_instances SET domain_payload = $1, domain_payload_hash = $2 WHERE instance_id = $3",
        )
        .bind(payload)
        .bind(&hash[..])
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("instance not found: {id}"));
        }
        Ok(())
    }

    // ── Fibers ──

    async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()> {
        let stack = serde_json::to_value(&fiber.stack)?;
        let regs = serde_json::to_value(&fiber.regs)?;
        let wait_state = serde_json::to_value(&fiber.wait)?;

        let result = sqlx::query(
            r#"
            INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
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
        .bind(fiber.pc as i32)
        .bind(&stack)
        .bind(&regs)
        .bind(&wait_state)
        .bind(fiber.loop_epoch as i32)
        .execute(&self.pool)
        .await?;

        // A18 — rows_affected validation. INSERT ... ON CONFLICT DO UPDATE
        // must touch exactly one row. Zero means RLS rejection on the
        // parent instance, or the parent instance was deleted concurrently.
        if result.rows_affected() == 0 {
            return Err(anyhow!(
                "save_fiber affected 0 rows for instance {} fiber {}; \
                 parent instance may be missing or RLS rejected",
                instance_id,
                fiber.fiber_id
            ));
        }

        Ok(())
    }

    async fn load_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<Option<Fiber>> {
        let row = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE instance_id = $1 AND fiber_id = $2",
        )
        .bind(instance_id)
        .bind(fiber_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let pc: i32 = row.get("pc");
                let stack_json: serde_json::Value = row.get("stack");
                let regs_json: serde_json::Value = row.get("regs");
                let wait_json: serde_json::Value = row.get("wait_state");
                let loop_epoch: i32 = row.get("loop_epoch");

                Ok(Some(Fiber {
                    fiber_id: row.get("fiber_id"),
                    pc: pc as u32,
                    stack: serde_json::from_value(stack_json)?,
                    regs: regs_from_json(regs_json)?,
                    wait: serde_json::from_value(wait_json)?,
                    loop_epoch: loop_epoch as u32,
                }))
            }
        }
    }

    async fn load_fibers(&self, instance_id: Uuid) -> Result<Vec<Fiber>> {
        let rows = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_all(&self.pool)
        .await?;

        let mut fibers = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row;
            let pc: i32 = row.get("pc");
            let stack_json: serde_json::Value = row.get("stack");
            let regs_json: serde_json::Value = row.get("regs");
            let wait_json: serde_json::Value = row.get("wait_state");
            let loop_epoch: i32 = row.get("loop_epoch");

            fibers.push(Fiber {
                fiber_id: row.get("fiber_id"),
                pc: pc as u32,
                stack: serde_json::from_value(stack_json)?,
                regs: regs_from_json(regs_json)?,
                wait: serde_json::from_value(wait_json)?,
                loop_epoch: loop_epoch as u32,
            });
        }
        Ok(fibers)
    }

    async fn delete_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM fibers WHERE instance_id = $1 AND fiber_id = $2")
            .bind(instance_id)
            .bind(fiber_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_all_fibers(&self, instance_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM fibers WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Join barriers ──

    async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
        let row = sqlx::query(
            r#"
            INSERT INTO join_barriers (instance_id, join_id, arrive_count)
            VALUES ($1, $2, 1)
            ON CONFLICT (instance_id, join_id) DO UPDATE
                SET arrive_count = join_barriers.arrive_count + 1
            RETURNING arrive_count
            "#,
        )
        .bind(instance_id)
        .bind(join_id as i32)
        .fetch_one(&self.pool)
        .await?;

        use sqlx::Row;
        let count: i16 = row.get("arrive_count");
        Ok(count as u16)
    }

    async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO join_barriers (instance_id, join_id, arrive_count)
            VALUES ($1, $2, 0)
            ON CONFLICT (instance_id, join_id) DO UPDATE
                SET arrive_count = 0
            "#,
        )
        .bind(instance_id)
        .bind(join_id as i32)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn join_delete_all(&self, instance_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM join_barriers WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Dedupe cache ──

    async fn dedupe_get(&self, key: &str) -> Result<Option<JobCompletion>> {
        let row = sqlx::query("SELECT completion FROM dedupe_cache WHERE job_key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let json: serde_json::Value = row.get("completion");
                Ok(Some(serde_json::from_value(json)?))
            }
        }
    }

    async fn dedupe_put(&self, key: &str, completion: &JobCompletion) -> Result<()> {
        let json = serde_json::to_value(completion)?;
        sqlx::query(
            r#"
            INSERT INTO dedupe_cache (job_key, completion)
            VALUES ($1, $2)
            ON CONFLICT (job_key) DO UPDATE SET completion = EXCLUDED.completion
            "#,
        )
        .bind(key)
        .bind(&json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_message_delivery(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        msg_id: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            INSERT INTO message_dedupe (tenant_id, instance_id, msg_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (tenant_id, instance_id, msg_id) DO NOTHING
            "#,
        )
        .bind(tenant_id)
        .bind(instance_id)
        .bind(msg_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    // ── Job queue ──

    async fn enqueue_job(&self, activation: &JobActivation) -> Result<()> {
        let orch_flags = serde_json::to_value(&activation.orch_flags)?;
        let session_stack = serde_json::to_value(&activation.session_stack)?;

        let result = sqlx::query(
            r#"
            INSERT INTO job_queue (
                job_key, tenant_id, process_instance_id, task_type, service_task_id,
                domain_payload, domain_payload_hash, session_stack, orch_flags, retries_remaining,
                entry_id, runbook_id
            ) VALUES ($1, (SELECT tenant_id FROM process_instances WHERE instance_id = $2), $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
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
        .execute(&self.pool)
        .await?;

        // A18 — rows_affected validation. INSERT with `ON CONFLICT DO NOTHING`
        // legitimately produces 0 rows on duplicate job_key (idempotent
        // re-enqueue). But it also produces 0 rows if the parent instance
        // subquery resolves to NULL (parent missing or RLS rejection) —
        // in that case the row insert would have a NULL tenant_id, which
        // the NOT NULL constraint rejects. So 0 here is ambiguous: either
        // benign duplicate, or unsignalled failure. We check the parent
        // existence explicitly to disambiguate.
        if result.rows_affected() == 0 {
            // Distinguish duplicate vs missing parent. If the job_key
            // already exists, we accept it (idempotent). Otherwise, the
            // parent instance is missing or RLS rejected.
            let existing: Option<(String,)> =
                sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = $1")
                    .bind(&activation.job_key)
                    .fetch_optional(&self.pool)
                    .await?;

            if existing.is_none() {
                return Err(anyhow!(
                    "enqueue_job affected 0 rows for job {} (instance {}); \
                     parent instance missing, RLS rejected, or NOT NULL \
                     constraint violation on tenant_id",
                    activation.job_key,
                    activation.process_instance_id
                ));
            }
            // Duplicate job_key — benign idempotent re-enqueue, fall through.
            tracing::debug!(
                job_key = %activation.job_key,
                "enqueue_job: duplicate job_key, idempotent no-op"
            );
        }

        Ok(())
    }

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &str,
        worker_id: &str,
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
            )
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
            "#,
        )
        .bind(task_types)
        .bind(max as i64)
        .bind(tenant_id)
        .bind(worker_id)
        .bind(lease_ms as f64)
        .fetch_all(&self.pool)
        .await?;

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
                domain_payload: row.get("domain_payload"),
                domain_payload_hash: bytes_to_hash(hash)?,
                session_stack: serde_json::from_value(session_stack_json)?,
                orch_flags: serde_json::from_value(orch_flags_json)?,
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

    async fn ack_job(&self, job_key: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM job_queue WHERE job_key = $1")
            .bind(job_key)
            .execute(&self.pool)
            .await?;

        // A18 — rows_affected validation. A 0-row DELETE on ack_job is
        // legitimate: a concurrent worker may have already acked, the
        // claim may have expired and been reclaimed elsewhere, or the
        // job may have been cancelled. Treat as a soft signal (debug
        // log) rather than an error to preserve current orchestrator
        // behavior. A18-Session-2 may revisit this to return a typed
        // AckOutcome { Acked, AlreadyAcked } once the caller side is
        // ready to discriminate.
        if result.rows_affected() == 0 {
            tracing::debug!(
                job_key = %job_key,
                "ack_job: 0 rows deleted (already acked, expired, or cancelled)"
            );
        }

        Ok(())
    }

    async fn validate_job_claim(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> Result<bool> {
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
        .bind(job_key)
        .bind(worker_id)
        .bind(claim_token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn retry_claimed_job(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        not_before_ms: i64,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE job_queue
            SET status = 'pending',
                claimed_at = NULL,
                worker_id = NULL,
                claim_token = NULL,
                claim_expires_at = NULL,
                not_before = $4,
                retries_remaining = GREATEST(retries_remaining - 1, 0),
                failure_count = failure_count + 1,
                last_failed_at = now(),
                last_error_class = $5,
                last_error_message = $6,
                last_error = $6
            WHERE job_key = $1
              AND status = 'claimed'
              AND worker_id = $2
              AND claim_token = $3
              AND claim_expires_at > now()
            "#,
        )
        .bind(job_key)
        .bind(worker_id)
        .bind(claim_token)
        .bind(epoch_ms_to_datetime(not_before_ms))
        .bind(error_class)
        .bind(error_message)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn dead_letter_claimed_job(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        incident_id: Uuid,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE job_queue
            SET status = 'dead_lettered',
                claimed_at = NULL,
                worker_id = NULL,
                claim_token = NULL,
                claim_expires_at = NULL,
                failure_count = failure_count + 1,
                last_failed_at = now(),
                dead_lettered_at = now(),
                last_error_class = $4,
                last_error_message = $5,
                last_error = $5,
                incident_id = $6
            WHERE job_key = $1
              AND status = 'claimed'
              AND worker_id = $2
              AND claim_token = $3
              AND claim_expires_at > now()
            "#,
        )
        .bind(job_key)
        .bind(worker_id)
        .bind(claim_token)
        .bind(error_class)
        .bind(error_message)
        .bind(incident_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn cancel_jobs_for_instance(&self, instance_id: Uuid) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "DELETE FROM job_queue WHERE process_instance_id = $1 AND status IN ('pending', 'claimed') RETURNING job_key",
        )
        .bind(instance_id)
        .fetch_all(&self.pool)
        .await?;

        use sqlx::Row;
        Ok(rows.iter().map(|r| r.get("job_key")).collect())
    }

    // ── Program store ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> Result<()> {
        let json = serde_json::to_value(program)?;
        sqlx::query(
            r#"
            INSERT INTO compiled_programs (bytecode_version, program)
            VALUES ($1, $2)
            ON CONFLICT (bytecode_version) DO NOTHING
            "#,
        )
        .bind(&version[..])
        .bind(&json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load_program(&self, version: [u8; 32]) -> Result<Option<CompiledProgram>> {
        let row = sqlx::query("SELECT program FROM compiled_programs WHERE bytecode_version = $1")
            .bind(&version[..])
            .fetch_optional(&self.pool)
            .await?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                let json: serde_json::Value = row.get("program");
                Ok(Some(serde_json::from_value(json)?))
            }
        }
    }

    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> Result<()> {
        let plan_json_value: serde_json::Value = serde_json::from_str(plan_json)
            .context("store_plan: invalid JSON")?;
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
        .context("store_plan: insert failed")?;
        Ok(())
    }

    async fn load_plan(&self, plan_hash: [u8; 32]) -> Result<Option<String>> {
        let row: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT plan_body FROM workflow_plans WHERE plan_hash = $1",
        )
        .bind(&plan_hash[..])
        .fetch_optional(&self.pool)
        .await
        .context("load_plan: query failed")?;
        Ok(row.map(|v| v.to_string()))
    }

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> Result<()> {
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
        .await?;
        Ok(())
    }

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> Result<Option<Vec<u8>>> {
        let key = value_key(corr_key);

        let row = sqlx::query(
            "DELETE FROM dead_letter_queue WHERE name = $1 AND corr_key = $2 AND expires_at > now() RETURNING payload",
        )
        .bind(name as i32)
        .bind(&key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                Ok(Some(row.get("payload")))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn buffer_message(
        &self,
        tenant_id: &str,
        message_name: &str,
        correlation_key: &str,
        msg_id: &str,
        payload: &[u8],
        payload_hash: Option<[u8; 32]>,
        ttl_ms: u64,
        process_instance_id: Option<Uuid>,
    ) -> Result<BufferMessageResult> {
        let expires_at = chrono::Utc::now() + chrono::Duration::milliseconds(ttl_ms as i64);
        let result = sqlx::query(
            r#"
            INSERT INTO message_buffer (
                tenant_id, message_name, correlation_key, msg_id, payload,
                payload_hash, expires_at, process_instance_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (tenant_id, message_name, correlation_key, msg_id) DO NOTHING
            "#,
        )
        .bind(tenant_id)
        .bind(message_name)
        .bind(correlation_key)
        .bind(msg_id)
        .bind(payload)
        .bind(payload_hash.map(|hash| hash.to_vec()))
        .bind(expires_at)
        .bind(process_instance_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            Ok(BufferMessageResult::Inserted)
        } else {
            Ok(BufferMessageResult::Duplicate)
        }
    }

    async fn claim_buffered_message(
        &self,
        tenant_id: &str,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> Result<Option<ClaimedBufferedMessage>> {
        let claim_until = chrono::Utc::now() + chrono::Duration::milliseconds(claim_ms as i64);
        let claim_token = Uuid::now_v7().to_string();
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
        .bind(tenant_id)
        .bind(message_name)
        .bind(correlation_key)
        .bind(&claim_token)
        .bind(claim_until)
        .fetch_optional(&self.pool)
        .await?;

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

    async fn atomic_consume_buffered_message(
        &self,
        instance: &ProcessInstance,
        fiber: &Fiber,
        message: &ClaimedBufferedMessage,
        payload_update: Option<&PayloadUpdate>,
        events: &[RuntimeEvent],
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;

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
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }

        let payload = payload_update
            .map(|payload_update| payload_update.payload.as_str())
            .unwrap_or(instance.domain_payload.as_ref());
        let payload_hash = payload_update
            .map(|payload_update| payload_update.payload_hash)
            .unwrap_or(instance.domain_payload_hash);

        let flags = serde_json::to_value(&instance.flags)?;
        let counters = serde_json::to_value(&instance.counters)?;
        let join_expected = serde_json::to_value(&instance.join_expected)?;
        let state = serde_json::to_value(&instance.state)?;

        sqlx::query(
            r#"
            UPDATE process_instances
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
        .execute(&mut *tx)
        .await?;

        if let Some(payload_update) = payload_update {
            sqlx::query(
                r#"
                INSERT INTO payload_history (instance_id, payload_hash, domain_payload)
                VALUES ($1, $2, $3)
                ON CONFLICT (instance_id, payload_hash) DO NOTHING
                "#,
            )
            .bind(instance.instance_id)
            .bind(&payload_update.payload_hash[..])
            .bind(&payload_update.payload)
            .execute(&mut *tx)
            .await?;
        }

        let stack = serde_json::to_value(&fiber.stack)?;
        let regs = serde_json::to_value(&fiber.regs)?;
        let wait_state = serde_json::to_value(&fiber.wait)?;
        sqlx::query(
            r#"
            INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
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
        .bind(fiber.pc as i32)
        .bind(&stack)
        .bind(&regs)
        .bind(&wait_state)
        .bind(fiber.loop_epoch as i32)
        .execute(&mut *tx)
        .await?;

        for event in events {
            let event_json = serde_json::to_value(event)?;
            sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq)
                    VALUES ($1, 1)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq
                )
                INSERT INTO event_log (instance_id, seq, event)
                SELECT $1, seq.next_seq, $2
                FROM seq
                "#,
            )
            .bind(instance.instance_id)
            .bind(&event_json)
            .execute(&mut *tx)
            .await?;
        }

        if !events.is_empty() {
            notify_event_tx(&mut tx, instance.instance_id).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn release_buffered_message_claim(
        &self,
        message: &ClaimedBufferedMessage,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE message_buffer
            SET claim_token = NULL,
                claimed_at = NULL,
                claim_until = NULL,
                status = 'buffered'
            WHERE tenant_id = $1
              AND message_name = $2
              AND correlation_key = $3
              AND msg_id = $4
              AND claim_token = $5
              AND consumed_at IS NULL
            "#,
        )
        .bind(&message.message.tenant_id)
        .bind(&message.message.message_name)
        .bind(&message.message.correlation_key)
        .bind(&message.message.msg_id)
        .bind(&message.claim_token)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> Result<u32> {
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
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() as u32)
    }

    async fn prune_expired_messages(&self) -> Result<u32> {
        let rows = sqlx::query(
            r#"
            DELETE FROM message_buffer
            WHERE consumed_at IS NULL
              AND expires_at <= now()
            RETURNING process_instance_id, message_name, correlation_key, msg_id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        use sqlx::Row;
        for row in &rows {
            let instance_id: Option<Uuid> = row.get("process_instance_id");
            if let Some(instance_id) = instance_id {
                self.append_event(
                    instance_id,
                    &RuntimeEvent::BufferedMessageExpired {
                        message_name: row.get("message_name"),
                        correlation_key: row.get("correlation_key"),
                        msg_id: row.get("msg_id"),
                    },
                )
                .await?;
            }
        }
        Ok(rows.len() as u32)
    }

    // ── Event log ──

    async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let event_json = serde_json::to_value(event)?;

        let row = sqlx::query(
            r#"
            WITH seq AS (
                INSERT INTO event_sequences (instance_id, next_seq)
                VALUES ($1, 1)
                ON CONFLICT (instance_id) DO UPDATE
                    SET next_seq = event_sequences.next_seq + 1
                RETURNING next_seq
            )
            INSERT INTO event_log (instance_id, seq, event)
            SELECT $1, seq.next_seq, $2
            FROM seq
            RETURNING seq
            "#,
        )
        .bind(instance_id)
        .bind(&event_json)
        .fetch_one(&mut *tx)
        .await?;

        use sqlx::Row;
        let seq: i64 = row.get("seq");
        notify_event_tx(&mut tx, instance_id).await?;
        tx.commit().await?;
        Ok(seq as u64)
    }

    async fn batch_append_events(&self, instance_id: Uuid, events: &[RuntimeEvent]) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let mut last_seq = 0;
        for event in events {
            let event_json = serde_json::to_value(event)?;
            let row = sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq)
                    VALUES ($1, 1)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq
                )
                INSERT INTO event_log (instance_id, seq, event)
                SELECT $1, seq.next_seq, $2
                FROM seq
                RETURNING seq
                "#,
            )
            .bind(instance_id)
            .bind(&event_json)
            .fetch_one(&mut *tx)
            .await?;

            use sqlx::Row;
            let seq: i64 = row.get("seq");
            last_seq = seq as u64;
        }
        if !events.is_empty() {
            notify_event_tx(&mut tx, instance_id).await?;
        }
        tx.commit().await?;
        Ok(last_seq)
    }

    async fn read_events(
        &self,
        instance_id: Uuid,
        from_seq: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>> {
        let rows = sqlx::query(
            "SELECT seq, event FROM event_log WHERE instance_id = $1 AND seq >= $2 ORDER BY seq",
        )
        .bind(instance_id)
        .bind(from_seq as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row;
            let seq: i64 = row.get("seq");
            let event_json: serde_json::Value = row.get("event");
            let event: RuntimeEvent = serde_json::from_value(event_json)?;
            events.push((seq as u64, event));
        }
        Ok(events)
    }

    // ── Payload history ──

    async fn save_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
        payload: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO payload_history (instance_id, payload_hash, domain_payload)
            VALUES ($1, $2, $3)
            ON CONFLICT (instance_id, payload_hash) DO NOTHING
            "#,
        )
        .bind(instance_id)
        .bind(&hash[..])
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT domain_payload FROM payload_history WHERE instance_id = $1 AND payload_hash = $2",
        )
        .bind(instance_id)
        .bind(&hash[..])
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(row) => {
                use sqlx::Row;
                Ok(Some(row.get("domain_payload")))
            }
        }
    }

    // ── Incidents ──

    async fn save_incident(&self, incident: &Incident) -> Result<()> {
        let error_class = serde_json::to_value(&incident.error_class)?;
        let created_at = epoch_ms_to_datetime(incident.created_at);
        let resolved_at = incident.resolved_at.map(epoch_ms_to_datetime);

        let result = sqlx::query(
            r#"
            INSERT INTO incidents (
                incident_id, process_instance_id, fiber_id, service_task_id,
                bytecode_addr, error_class, message, retry_count,
                created_at, resolved_at, resolution
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(incident.incident_id)
        .bind(incident.process_instance_id)
        .bind(incident.fiber_id)
        .bind(&incident.service_task_id)
        .bind(incident.bytecode_addr as i32)
        .bind(&error_class)
        .bind(&incident.message)
        .bind(incident.retry_count as i32)
        .bind(created_at)
        .bind(resolved_at)
        .bind(&incident.resolution)
        .execute(&self.pool)
        .await?;

        // A18 — rows_affected validation. A straight INSERT with no
        // ON CONFLICT clause must produce exactly 1 row. Zero means
        // RLS rejected (when migration 025's policy applies) or the
        // parent process_instance was deleted concurrently.
        if result.rows_affected() == 0 {
            return Err(anyhow!(
                "save_incident affected 0 rows for incident {} (instance {}); \
                 parent missing, RLS rejected, or constraint violation",
                incident.incident_id,
                incident.process_instance_id
            ));
        }

        Ok(())
    }

    async fn load_incidents(&self, instance_id: Uuid) -> Result<Vec<Incident>> {
        let rows = sqlx::query(
            r#"
            SELECT incident_id, process_instance_id, fiber_id, service_task_id,
                   bytecode_addr, error_class, message, retry_count,
                   (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms,
                   (EXTRACT(EPOCH FROM resolved_at) * 1000)::BIGINT AS resolved_at_ms,
                   resolution
            FROM incidents
            WHERE process_instance_id = $1
            ORDER BY created_at
            "#,
        )
        .bind(instance_id)
        .fetch_all(&self.pool)
        .await?;

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
                bytecode_addr: bytecode_addr as u32,
                error_class: serde_json::from_value(error_class_json)?,
                message: row.get("message"),
                retry_count: retry_count as u32,
                created_at: created_at_ms,
                resolved_at: resolved_at_ms,
                resolution: row.get("resolution"),
            });
        }
        Ok(incidents)
    }

    // ── Atomic compound operations ──

    async fn atomic_start(
        &self,
        instance: &ProcessInstance,
        root_fiber: &Fiber,
        event: &RuntimeEvent,
    ) -> Result<u64> {
        // Register the tenant on first use. Idempotent — ON CONFLICT DO NOTHING.
        // Runs outside the main transaction so the tenants row is visible to
        // the scheduler even if the main transaction rolls back.
        self.ensure_tenant(&instance.tenant_id).await?;

        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &instance.tenant_id).await?;

        // 1. INSERT process_instances
        let flags = serde_json::to_value(&instance.flags)?;
        let counters = serde_json::to_value(&instance.counters)?;
        let join_expected = serde_json::to_value(&instance.join_expected)?;
        let state = serde_json::to_value(&instance.state)?;
        let session_stack = serde_json::to_value(&instance.session_stack)?;
        let created_at = epoch_ms_to_datetime(instance.created_at);

        sqlx::query(
            r#"
            INSERT INTO process_instances (
                instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                domain_payload_hash, session_stack, flags, counters, join_expected, state,
                correlation_id, entry_id, runbook_id, created_at,
                plan_hash, current_node_id, placeholder_values
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                      $16, $17, $18)
            "#,
        )
        .bind(instance.instance_id)
        .bind(&instance.tenant_id)
        .bind(&instance.process_key)
        .bind(&instance.bytecode_version[..])
        .bind(instance.domain_payload.as_ref())
        .bind(&instance.domain_payload_hash[..])
        .bind(&session_stack)
        .bind(&flags)
        .bind(&counters)
        .bind(&join_expected)
        .bind(&state)
        .bind(&instance.correlation_id)
        .bind(instance.entry_id)
        .bind(instance.runbook_id)
        .bind(created_at)
        .bind(instance.plan_hash.as_ref().map(|h| h.as_slice()))
        .bind(instance.current_node_id.as_deref())
        .bind(instance.placeholder_values.as_ref())
        .execute(&mut *tx)
        .await?;

        // 2. INSERT fiber
        let stack = serde_json::to_value(&root_fiber.stack)?;
        let regs = serde_json::to_value(&root_fiber.regs)?;
        let wait_state = serde_json::to_value(&root_fiber.wait)?;

        sqlx::query(
            r#"
            INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(instance.instance_id)
        .bind(root_fiber.fiber_id)
        .bind(root_fiber.pc as i32)
        .bind(&stack)
        .bind(&regs)
        .bind(&wait_state)
        .bind(root_fiber.loop_epoch as i32)
        .execute(&mut *tx)
        .await?;

        // 3. Append event (sequence + log)
        let event_json = serde_json::to_value(event)?;

        let row = sqlx::query(
            r#"
            WITH seq AS (
                INSERT INTO event_sequences (instance_id, next_seq)
                VALUES ($1, 1)
                ON CONFLICT (instance_id) DO UPDATE
                    SET next_seq = event_sequences.next_seq + 1
                RETURNING next_seq
            )
            INSERT INTO event_log (instance_id, seq, event)
            SELECT $1, seq.next_seq, $2
            FROM seq
            RETURNING seq
            "#,
        )
        .bind(instance.instance_id)
        .bind(&event_json)
        .fetch_one(&mut *tx)
        .await?;

        use sqlx::Row;
        let seq: i64 = row.get("seq");

        notify_event_tx(&mut tx, instance.instance_id).await?;
        tx.commit().await?;
        Ok(seq as u64)
    }

    async fn atomic_complete(
        &self,
        instance: &ProcessInstance,
        completion: &JobCompletion,
        events: &[RuntimeEvent],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &instance.tenant_id).await?;

        // 1. UPSERT process_instances
        let flags = serde_json::to_value(&instance.flags)?;
        let counters = serde_json::to_value(&instance.counters)?;
        let join_expected = serde_json::to_value(&instance.join_expected)?;
        let state = serde_json::to_value(&instance.state)?;
        let session_stack = serde_json::to_value(&instance.session_stack)?;
        let created_at = epoch_ms_to_datetime(instance.created_at);

        sqlx::query(
            r#"
            INSERT INTO process_instances (
                instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                domain_payload_hash, session_stack, flags, counters, join_expected, state,
                correlation_id, entry_id, runbook_id, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (instance_id) DO UPDATE SET
                domain_payload = EXCLUDED.domain_payload,
                domain_payload_hash = EXCLUDED.domain_payload_hash,
                session_stack = EXCLUDED.session_stack,
                flags = EXCLUDED.flags,
                counters = EXCLUDED.counters,
                join_expected = EXCLUDED.join_expected,
                state = EXCLUDED.state,
                correlation_id = EXCLUDED.correlation_id
            -- Immutable fields omitted: migration 029 trigger enforces them.
            "#,
        )
        .bind(instance.instance_id)
        .bind(&instance.tenant_id)
        .bind(&instance.process_key)
        .bind(&instance.bytecode_version[..])
        .bind(instance.domain_payload.as_ref())
        .bind(&instance.domain_payload_hash[..])
        .bind(&session_stack)
        .bind(&flags)
        .bind(&counters)
        .bind(&join_expected)
        .bind(&state)
        .bind(&instance.correlation_id)
        .bind(instance.entry_id)
        .bind(instance.runbook_id)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;

        // 2. INSERT dedupe_cache ON CONFLICT
        let completion_json = serde_json::to_value(completion)?;
        sqlx::query(
            r#"
            INSERT INTO dedupe_cache (job_key, completion)
            VALUES ($1, $2)
            ON CONFLICT (job_key) DO UPDATE SET completion = EXCLUDED.completion
            "#,
        )
        .bind(&completion.job_key)
        .bind(&completion_json)
        .execute(&mut *tx)
        .await?;

        // 3. INSERT payload_history ON CONFLICT
        sqlx::query(
            r#"
            INSERT INTO payload_history (instance_id, payload_hash, domain_payload)
            VALUES ($1, $2, $3)
            ON CONFLICT (instance_id, payload_hash) DO NOTHING
            "#,
        )
        .bind(instance.instance_id)
        .bind(&instance.domain_payload_hash[..])
        .bind(instance.domain_payload.as_ref())
        .execute(&mut *tx)
        .await?;

        // 4. Append completion events in the same transaction.
        for event in events {
            let event_json = serde_json::to_value(event)?;
            sqlx::query(
                r#"
                WITH seq AS (
                    INSERT INTO event_sequences (instance_id, next_seq)
                    VALUES ($1, 1)
                    ON CONFLICT (instance_id) DO UPDATE
                        SET next_seq = event_sequences.next_seq + 1
                    RETURNING next_seq
                )
                INSERT INTO event_log (instance_id, seq, event)
                SELECT $1, seq.next_seq, $2
                FROM seq
                "#,
            )
            .bind(instance.instance_id)
            .bind(&event_json)
            .execute(&mut *tx)
            .await?;
        }

        // 5. ACK job in the same transaction as completion state.
        sqlx::query("DELETE FROM job_queue WHERE job_key = $1")
            .bind(&completion.job_key)
            .execute(&mut *tx)
            .await?;

        if !events.is_empty() {
            notify_event_tx(&mut tx, instance.instance_id).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    // ── Durability maintenance ──

    async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> Result<u32> {
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
        .fetch_all(&self.pool)
        .await?;

        use sqlx::Row;
        for row in &rows {
            let instance_id: Uuid = row.get("process_instance_id");
            let previous_worker_id: Option<String> = row.get("previous_worker_id");
            self.append_event(
                instance_id,
                &RuntimeEvent::JobReclaimed {
                    job_key: row.get("job_key"),
                    previous_worker_id,
                },
            )
            .await?;
        }
        Ok(rows.len() as u32)
    }

    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> Result<u32> {
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
        .await?;

        use sqlx::Row;
        let cnt: i64 = row.get("cnt");
        Ok(cnt as u32)
    }

    async fn list_running_instances(&self, tenant_id: &str) -> Result<Vec<Uuid>> {
        let rows = sqlx::query(
            r#"SELECT instance_id FROM process_instances WHERE tenant_id = $1 AND state = '"Running"'::jsonb"#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        use sqlx::Row;
        Ok(rows.iter().map(|r| r.get("instance_id")).collect())
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &str,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> Result<Vec<Uuid>> {
        let rows = sqlx::query(
            r#"
            WITH candidates AS (
                SELECT instance_id
                FROM process_instances
                WHERE tenant_id = $1
                  AND state = '"Running"'::jsonb
                  AND quarantine_state IS NULL
                  AND (lease_until IS NULL OR lease_until < now() OR lease_owner = $2)
                ORDER BY updated_at
                LIMIT $3
                FOR UPDATE SKIP LOCKED
            )
            UPDATE process_instances
            SET lease_owner = $2,
                lease_until = now() + make_interval(secs => $4::float / 1000.0),
                last_tick_at = now()
            FROM candidates
            WHERE process_instances.instance_id = candidates.instance_id
            RETURNING process_instances.instance_id
            "#,
        )
        .bind(tenant_id)
        .bind(owner)
        .bind(limit as i64)
        .bind(lease_ms as f64)
        .fetch_all(&self.pool)
        .await?;

        use sqlx::Row;
        Ok(rows.iter().map(|r| r.get("instance_id")).collect())
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE process_instances
            SET lease_owner = $3,
                lease_until = now() + make_interval(secs => $4::float / 1000.0),
                last_tick_at = now()
            WHERE tenant_id = $1
              AND instance_id = $2
              AND (lease_until IS NULL OR lease_until < now() OR lease_owner = $3)
            "#,
        )
        .bind(tenant_id)
        .bind(instance_id)
        .bind(owner)
        .bind(lease_ms as f64)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE process_instances
            SET lease_owner = NULL,
                lease_until = NULL
            WHERE tenant_id = $1
              AND instance_id = $2
              AND lease_owner = $3
            "#,
        )
        .bind(tenant_id)
        .bind(instance_id)
        .bind(owner)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn ensure_tenant(&self, tenant_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO tenants (tenant_id, pool_id) VALUES ($1, 'default') ON CONFLICT DO NOTHING",
        )
        .bind(tenant_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_tenants(&self) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT tenant_id FROM tenants ORDER BY first_seen_at")
            .fetch_all(&self.pool)
            .await?;
        use sqlx::Row;
        Ok(rows
            .iter()
            .map(|r| r.get::<String, _>("tenant_id"))
            .collect())
    }

    async fn list_tenants_in_pool(&self, pool_id: &str) -> Result<Vec<String>> {
        let rows =
            sqlx::query("SELECT tenant_id FROM tenants WHERE pool_id = $1 ORDER BY first_seen_at")
                .bind(pool_id)
                .fetch_all(&self.pool)
                .await?;
        use sqlx::Row;
        Ok(rows
            .iter()
            .map(|r| r.get::<String, _>("tenant_id"))
            .collect())
    }

    async fn quarantine_instance(
        &self,
        instance_id: Uuid,
        tenant_id: &str,
        detection_point: &str,
    ) -> Result<()> {
        // 1. Mark the row as quarantined. Use a separate pool connection
        //    so the quarantine persists even if the caller's transaction rolls back.
        sqlx::query(
            "UPDATE process_instances \
             SET quarantine_state = 'integrity_violation' \
             WHERE instance_id = $1",
        )
        .bind(instance_id)
        .execute(&self.pool)
        .await
        .context("quarantine_instance: failed to set quarantine_state")?;

        // 2. Append InstanceQuarantined event to the audit log.
        let now = chrono::Utc::now();
        let now_ms = now.timestamp_millis();
        let event = RuntimeEvent::InstanceQuarantined {
            instance_id,
            tenant_id: tenant_id.to_string(),
            detection_point: detection_point.to_string(),
            failure_reason: "integrity_hash_mismatch".to_string(),
            detected_at: now_ms,
        };
        let event_json = serde_json::to_value(&event)?;

        sqlx::query(
            r#"
            WITH seq AS (
                INSERT INTO event_sequences (instance_id, next_seq)
                VALUES ($1, 1)
                ON CONFLICT (instance_id) DO UPDATE
                    SET next_seq = event_sequences.next_seq + 1
                RETURNING next_seq
            )
            INSERT INTO event_log (instance_id, seq, event)
            SELECT $1, seq.next_seq, $2
            FROM seq
            "#,
        )
        .bind(instance_id)
        .bind(&event_json)
        .execute(&self.pool)
        .await
        .context("quarantine_instance: failed to append InstanceQuarantined event")?;

        tracing::warn!(
            instance_id = %instance_id,
            tenant_id = %tenant_id,
            detection_point = %detection_point,
            "A19: instance quarantined due to integrity hash mismatch"
        );

        Ok(())
    }
}

// The whole crate is postgres-only — no need for the inner cfg-gate
// that store_postgres used when it lived inside bpmn-lite-core
// behind `cfg(feature = "postgres")`. Tests still need a real
// database (BPMN_LITE_TEST_DATABASE_URL); the `--ignored` runner
// guards them at the test level.
#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_engine::BpmnLiteEngine;
    use sqlx::PgPool;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

    async fn setup() -> (PgPool, PostgresProcessStore) {
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
        let pool = PgPool::connect(&url).await.expect("connect to db");

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        // Truncate all tables
        sqlx::query("TRUNCATE process_instances CASCADE")
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

        let store = PostgresProcessStore::new(pool.clone());
        (pool, store)
    }

    fn test_hash(data: &str) -> [u8; 32] {
        blake3::hash(data.as_bytes()).into()
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

    /// T-PG-1: Instance round-trip
    #[tokio::test]
    #[ignore]
    async fn test_pg_instance_round_trip() {
        let (_pool, store) = setup().await;
        let id = Uuid::now_v7();
        let inst = make_instance(id);

        store.save_instance(&inst).await.unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();

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
    #[ignore]
    async fn test_pg_instance_session_stack_copy_round_trip() {
        let (_pool, store) = setup().await;
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

        store.save_instance(&inst).await.unwrap();

        inst.session_stack.scope = Some(bpmn_lite_types::session_stack::SessionScopeState {
            client_group_id: mutated_scope_id,
            client_group_name: Some("Mutated".to_string()),
        });
        inst.session_stack.active_workspace =
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Deal);
        inst.session_stack.trace_sequence = 99;

        let loaded = store.load_instance(id).await.unwrap().unwrap();
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
    #[ignore]
    async fn test_pg_fiber_round_trip() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        let fid = Uuid::now_v7();

        // Need an instance first (FK constraint)
        store.save_instance(&make_instance(iid)).await.unwrap();

        let mut fiber = Fiber::new(fid, 10);
        fiber.wait = WaitState::Job {
            job_key: "job-123".to_string(),
        };
        fiber.stack.push(Value::I64(99));
        fiber.loop_epoch = 3;

        store.save_fiber(iid, &fiber).await.unwrap();
        let loaded = store.load_fiber(iid, fid).await.unwrap().unwrap();

        assert_eq!(loaded.fiber_id, fid);
        assert_eq!(loaded.pc, 10);
        assert_eq!(
            loaded.wait,
            WaitState::Job {
                job_key: "job-123".to_string()
            }
        );
        assert_eq!(loaded.stack, vec![Value::I64(99)]);
        assert_eq!(loaded.loop_epoch, 3);
        // Verify regs padded to 8
        assert_eq!(loaded.regs.len(), 8);

        // Delete
        store.delete_fiber(iid, fid).await.unwrap();
        assert!(store.load_fiber(iid, fid).await.unwrap().is_none());
    }

    /// T-PG-3: Join barrier
    #[tokio::test]
    #[ignore]
    async fn test_pg_join_barrier() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

        let join_id: JoinId = 0;
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 1);
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 2);
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 3);

        store.join_reset(iid, join_id).await.unwrap();
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 1);
    }

    /// T-PG-4: Dedupe
    #[tokio::test]
    #[ignore]
    async fn test_pg_dedupe() {
        let (_pool, store) = setup().await;
        let completion = JobCompletion {
            job_key: "job-abc".to_string(),
            domain_payload: r#"{"done":true}"#.to_string(),
            expected_instance_payload_hash: test_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };

        assert!(store.dedupe_get("job-abc").await.unwrap().is_none());
        store.dedupe_put("job-abc", &completion).await.unwrap();

        let cached = store.dedupe_get("job-abc").await.unwrap().unwrap();
        assert_eq!(cached.job_key, "job-abc");
        assert_eq!(cached.domain_payload, r#"{"done":true}"#);

        // Idempotent put
        store.dedupe_put("job-abc", &completion).await.unwrap();
    }

    /// T-PG-5: Job queue
    #[tokio::test]
    #[ignore]
    async fn test_pg_job_queue() {
        let (_pool, store) = setup().await;
        let task_type = "create_case".to_string();
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

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
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch1.len(), 2);

        // Ack one
        store.ack_job(&batch1[0].job_key).await.unwrap();

        // Dequeue remaining
        let batch2 = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                10,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch2.len(), 1);
    }

    /// T-PG-6: Event log
    #[tokio::test]
    #[ignore]
    async fn test_pg_event_log() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

        for i in 0..5 {
            let event = RuntimeEvent::FlagSet {
                key: i,
                value: Value::I64(i as i64),
            };
            let seq = store.append_event(iid, &event).await.unwrap();
            assert_eq!(seq, (i + 1) as u64);
        }

        let events = store.read_events(iid, 3).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].0, 3);
        assert_eq!(events[1].0, 4);
        assert_eq!(events[2].0, 5);
    }

    /// T-PG-7: Payload history
    #[tokio::test]
    #[ignore]
    async fn test_pg_payload_history() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

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
            .load_payload_version(iid, &hash_v1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v1, payload_v1);

        let loaded_v2 = store
            .load_payload_version(iid, &hash_v2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v2, payload_v2);

        let bad_hash = [0xFFu8; 32];
        assert!(store
            .load_payload_version(iid, &bad_hash)
            .await
            .unwrap()
            .is_none());
    }

    /// T-PG-8: Program store
    #[tokio::test]
    #[ignore]
    async fn test_pg_program_store() {
        let (_pool, store) = setup().await;

        let program = CompiledProgram {
            bytecode_version: test_hash("test-program"),
            program: vec![Instr::End],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let version = program.bytecode_version;
        store.store_program(version, &program).await.unwrap();

        let loaded = store.load_program(version).await.unwrap().unwrap();
        assert_eq!(loaded.bytecode_version, version);
        assert_eq!(loaded.program.len(), 1);

        // Idempotent store
        store.store_program(version, &program).await.unwrap();
    }

    /// T-PG-9: Dead letter
    #[tokio::test]
    #[ignore]
    async fn test_pg_dead_letter() {
        let (_pool, store) = setup().await;
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
    #[ignore]
    async fn test_pg_incidents() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

        for i in 0..2 {
            store
                .save_incident(&Incident {
                    incident_id: Uuid::now_v7(),
                    process_instance_id: iid,
                    fiber_id: Uuid::now_v7(),
                    service_task_id: format!("task-{i}"),
                    bytecode_addr: i * 10,
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

        let loaded = store.load_incidents(iid).await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    /// T-PG-11: Instance updates
    #[tokio::test]
    #[ignore]
    async fn test_pg_instance_updates() {
        let (_pool, store) = setup().await;
        let id = Uuid::now_v7();
        store.save_instance(&make_instance(id)).await.unwrap();

        // Update state
        let new_state = ProcessState::Completed { at: 1700001000000 };
        store
            .update_instance_state(id, new_state.clone())
            .await
            .unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();
        assert_eq!(loaded.state, new_state);

        // Update flags
        let new_flags = BTreeMap::from([(5, Value::Bool(false))]);
        store.update_instance_flags(id, &new_flags).await.unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();
        assert_eq!(loaded.flags.len(), 1);
        assert_eq!(loaded.flags[&5], Value::Bool(false));

        // Update payload
        let new_payload = r#"{"updated":true}"#;
        let new_hash = test_hash(new_payload);
        store
            .update_instance_payload(id, new_payload, &new_hash)
            .await
            .unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();
        assert_eq!(loaded.domain_payload.as_ref(), new_payload);
        assert_eq!(loaded.domain_payload_hash, new_hash);
    }

    /// T-PG-12: Teardown (delete_all_fibers + join_delete_all)
    #[tokio::test]
    #[ignore]
    async fn test_pg_teardown() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

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
        let fibers = store.load_fibers(iid).await.unwrap();
        assert!(fibers.is_empty());

        // join_delete_all
        store.join_delete_all(iid).await.unwrap();
        // Arrive again — should start at 1
        assert_eq!(store.join_arrive(iid, 0).await.unwrap(), 1);
    }

    /// T-PG-13: Concurrent dequeue (SKIP LOCKED)
    #[tokio::test]
    #[ignore]
    async fn test_pg_concurrent_dequeue() {
        let (_pool, store) = setup().await;
        let store = Arc::new(store);
        let task_type = "concurrent_task".to_string();
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

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
                s.dequeue_jobs(&[tt], 1, "default", "test-worker", 300_000)
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
    #[ignore]
    async fn test_pg_full_engine_smoke() {
        let (_pool, store) = setup().await;
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
            .dequeue_jobs(&task_types, 1, "default", "test-worker", 300_000)
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
        let inst = store.load_instance(instance_id).await.unwrap().unwrap();

        // Read events — should have at least InstanceStarted
        let events = store.read_events(instance_id, 1).await.unwrap();
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
    }

    /// T-PG-15: cancel_jobs_for_instance
    #[tokio::test]
    #[ignore]
    async fn test_pg_cancel_jobs_for_instance() {
        let (_pool, store) = setup().await;
        let task_type = "cancel_test".to_string();

        let iid_a = Uuid::now_v7();
        let iid_b = Uuid::now_v7();
        store.save_instance(&make_instance(iid_a)).await.unwrap();

        let mut inst_b = make_instance(iid_b);
        inst_b.instance_id = iid_b;
        store.save_instance(&inst_b).await.unwrap();

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
            .dequeue_jobs(&[task_type], 10, "default", "test-worker", 300_000)
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
    /// The job_queue tenant_id is derived via subquery on process_instances;
    /// a missing parent yields NULL tenant_id which violates NOT NULL.
    #[tokio::test]
    #[ignore]
    async fn test_a18_enqueue_job_missing_parent_errors() {
        let (_pool, store) = setup().await;
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
    #[ignore]
    async fn test_a18_enqueue_job_duplicate_is_idempotent() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

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
    #[ignore]
    async fn test_a18_save_incident_missing_parent_errors() {
        let (_pool, store) = setup().await;
        let fake_parent = Uuid::now_v7();

        let incident = Incident {
            incident_id: Uuid::now_v7(),
            process_instance_id: fake_parent,
            fiber_id: Uuid::now_v7(),
            service_task_id: "a18-task".to_string(),
            bytecode_addr: 0,
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
    #[ignore]
    async fn test_a18_ack_job_already_acked_is_ok() {
        let (_pool, store) = setup().await;
        // No setup needed — job_key simply doesn't exist.
        store
            .ack_job("a18-nonexistent-job-key")
            .await
            .expect("ack_job of nonexistent key must be Ok (soft signal)");
    }

    /// T-A18-5: save_instance + save_fiber happy path still works.
    /// Regression guard so rows_affected validation doesn't break the
    /// normal path.
    #[tokio::test]
    #[ignore]
    async fn test_a18_happy_path_writes_succeed() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();
        let fid = Uuid::now_v7();

        store
            .save_instance(&make_instance(iid))
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
    // These tests require a real Postgres database and are gated by #[ignore].
    // They verify: hash stored at creation; load returns it; tampering surfaces;
    // quarantined instances are skipped by claim_running_instances.

    /// T-A19-PG-1: save_instance stores an integrity hash; load_instance returns it.
    #[tokio::test]
    #[ignore]
    async fn test_a19_hash_stored_on_save_and_loaded() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance(&make_instance(iid)).await.unwrap();

        let loaded = store.load_instance(iid).await.unwrap().unwrap();
        assert!(
            loaded.integrity_hash.is_some(),
            "integrity_hash must be set after save_instance"
        );

        // Verify the hash is correct (matches recomputation).
        use bpmn_lite_types::integrity::verify_instance_integrity;
        assert!(
            verify_instance_integrity(&loaded).is_ok(),
            "loaded instance must pass integrity verification"
        );
    }

    /// T-A19-PG-2: hash is NOT updated when save_instance is called again (ON CONFLICT branch).
    #[tokio::test]
    #[ignore]
    async fn test_a19_hash_not_overwritten_on_update() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance(&make_instance(iid)).await.unwrap();
        let original_hash = store
            .load_instance(iid)
            .await
            .unwrap()
            .unwrap()
            .integrity_hash;

        // Re-save (simulates tick updating state/flags).
        let inst = store.load_instance(iid).await.unwrap().unwrap();
        store.save_instance(&inst).await.unwrap();

        let after_hash = store
            .load_instance(iid)
            .await
            .unwrap()
            .unwrap()
            .integrity_hash;

        assert_eq!(original_hash, after_hash, "hash must not change on update");
    }

    /// T-A19-PG-3: deliberate DB-level tamper of tenant_id is detected via verify_instance_integrity.
    #[tokio::test]
    #[ignore]
    async fn test_a19_tamper_tenant_id_detected() {
        let (pool, store) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance(&make_instance(iid)).await.unwrap();

        // Simulate DB-level tamper (bypass application layer).
        sqlx::query(
            "UPDATE process_instances SET tenant_id = 'evil-tenant' WHERE instance_id = $1",
        )
        .bind(iid)
        .execute(&pool)
        .await
        .unwrap();

        let loaded = store.load_instance(iid).await.unwrap().unwrap();
        use bpmn_lite_types::integrity::verify_instance_integrity;
        let result = verify_instance_integrity(&loaded);
        assert!(
            result.is_err(),
            "tampered instance must fail integrity verification"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("integrity hash mismatch"), "got: {msg}");
    }

    /// T-A19-PG-4: quarantine_instance marks the row and logs an event.
    #[tokio::test]
    #[ignore]
    async fn test_a19_quarantine_marks_row_and_logs_event() {
        use sqlx::Row;
        let (pool, store) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance(&make_instance(iid)).await.unwrap();
        store
            .quarantine_instance(iid, "default", "grpc_handler")
            .await
            .expect("quarantine_instance must succeed");

        // Check quarantine_state column.
        let row =
            sqlx::query("SELECT quarantine_state FROM process_instances WHERE instance_id = $1")
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
        let events = store.read_events(iid, 0).await.unwrap();
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
    #[ignore]
    async fn test_a19_quarantined_instance_skipped_by_scheduler() {
        let (_pool, store) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance(&make_instance(iid)).await.unwrap();

        // Quarantine the instance.
        store
            .quarantine_instance(iid, "default", "scheduler_claim")
            .await
            .unwrap();

        // Claim batch — quarantined instance should not be returned.
        let claimed = store
            .claim_running_instances("default", "test-scheduler", 10, 5_000)
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
    #[ignore]
    async fn test_l0_default_pool_exists() {
        let (pool, _store) = setup().await;
        let row: (String,) =
            sqlx::query_as("SELECT pool_id FROM tenant_pools WHERE pool_id = 'default'")
                .fetch_one(&pool)
                .await
                .expect("default pool row must exist after migration 032");
        assert_eq!(row.0, "default");
    }

    /// T-L0-PG-2: ensure_tenant assigns pool_id = 'default'.
    #[tokio::test]
    #[ignore]
    async fn test_l0_ensure_tenant_sets_pool_id() {
        let (pool, store) = setup().await;
        store.ensure_tenant("l0_test_tenant").await.unwrap();
        let row: (String,) =
            sqlx::query_as("SELECT pool_id FROM tenants WHERE tenant_id = 'l0_test_tenant'")
                .fetch_one(&pool)
                .await
                .expect("tenant row must exist");
        assert_eq!(row.0, "default");
    }

    /// T-L0-PG-3: list_tenants_in_pool returns only tenants in that pool.
    #[tokio::test]
    #[ignore]
    async fn test_l0_list_tenants_in_pool() {
        let (_pool, store) = setup().await;
        store.ensure_tenant("l0_pool_tenant_a").await.unwrap();
        store.ensure_tenant("l0_pool_tenant_b").await.unwrap();

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
    #[ignore]
    async fn test_l0_fk_rejects_unknown_pool() {
        let (pool, _store) = setup().await;
        let result = sqlx::query(
            "INSERT INTO tenants (tenant_id, pool_id) VALUES ('fk_test_tenant', 'nonexistent_pool')",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "FK constraint must reject unknown pool_id");
    }
}
