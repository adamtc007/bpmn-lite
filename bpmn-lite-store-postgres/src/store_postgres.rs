#![allow(clippy::redundant_pattern_matching, clippy::needless_borrow)]
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bpmn_lite_store::store::{ProcessStore, TickOperation};
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

    pub fn assert_rows_affected(&self, result: &sqlx::postgres::PgQueryResult, expected: u64, msg: &str) -> Result<()> {
        let rows = result.rows_affected();
        if rows != expected {
            return Err(anyhow!("{} (affected {} rows, expected {})", msg, rows, expected));
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
    F: for<'b, 'c> FnOnce(&'b mut TenantTx<'c>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'b>> + Send,
    T: Send,
{
    let mut tx = pool.begin().await.context("execute_tenant_scoped: begin transaction")?;

    PostgresProcessStore::set_tenant_context(&mut tx, tenant_id).await?;

    let mut tenant_tx = TenantTx {
        tx,
        tenant_id: tenant_id.to_string(),
        lease_owner: lease_owner.to_string(),
    };

    let result = f(&mut tenant_tx).await;

    if result.is_ok() {
        tenant_tx.tx.commit().await.context("execute_tenant_scoped: commit transaction")?;
    }

    result
}

/// PostgreSQL-backed implementation of `ProcessStore`.
pub struct PostgresProcessStore {
    pool: sqlx::PgPool,
}

impl PostgresProcessStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    pub async fn execute_tenant_scoped<F, T>(&self, tenant_id: &str, lease_owner: &str, f: F) -> Result<T>
    where
        F: for<'b, 'c> FnOnce(&'b mut TenantTx<'c>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'b>> + Send,
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
            .context("failed to set tenant context for RLS")?;
        Ok(())
    }

    async fn list_tenants(&self) -> Result<Vec<String>> {
        use sqlx::Row;
        let rows = sqlx::query("SELECT tenant_id FROM tenants ORDER BY first_seen_at")
            .fetch_all(&self.pool)
            .await;
        match rows {
            Ok(rows) => {
                let tenants: Vec<String> = rows.iter().map(|r| r.get::<String, _>("tenant_id")).collect();
                if tenants.is_empty() {
                    Ok(vec!["default".to_string()])
                } else {
                    Ok(tenants)
                }
            }
            Err(_) => Ok(vec!["default".to_string()]),
        }
    }

    pub async fn resolve_tenant_id(&self, instance_id: Uuid) -> Result<String> {
        let tenants = self.list_tenants().await?;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = Self::set_tenant_context(&mut tx, &tenant_id).await {
                continue;
            }
            let exists_res: Result<bool, sqlx::Error> = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM process_instances WHERE instance_id = $1)")
                .bind(instance_id)
                .fetch_one(&mut *tx)
                .await;
            let _ = tx.commit().await;
            if let Ok(true) = exists_res {
                return Ok(tenant_id);
            }
        }
        Err(anyhow!("resolve_tenant_id: instance not found"))
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

pub struct StaleReclaimInfo {
    pub job_key: String,
    pub process_instance_id: Uuid,
    pub previous_worker_id: Option<String>,
}

#[async_trait]
impl ProcessStore for PostgresProcessStore {
    // ── Instance ──

    async fn save_instance(&self, lease_owner: &str, instance: &ProcessInstance) -> Result<()> {
        let tenant_id = instance.tenant_id.clone();
        let lease_owner = lease_owner.to_string();
        let instance = instance.clone();
        self.ensure_tenant(&tenant_id).await?;
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            let flags = serde_json::to_value(&instance.flags)?;
            let counters = serde_json::to_value(&instance.counters)?;
            let join_expected = serde_json::to_value(&instance.join_expected)?;
            let state = serde_json::to_value(&instance.state)?;
            let session_stack = serde_json::to_value(&instance.session_stack)?;
            let created_at = epoch_ms_to_datetime(instance.created_at);
            let integrity_hash = compute_instance_integrity_hash(&instance);

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
                WHERE process_instances.lease_owner = $20
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
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await?;

            tx.assert_rows_affected(&result, 1, "save_instance")
        })).await
    }
    async fn load_instance(&self, id: Uuid) -> Result<Option<ProcessInstance>> {
        let tenant_id = match self.resolve_tenant_id(id).await {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
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
            .fetch_optional(&mut *tx.tx)
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
        })).await
    }

    async fn update_instance_state(&self, tenant_id: &str, lease_owner: &str, id: Uuid, state: ProcessState) -> Result<()> {
        let tenant_id = tenant_id.to_string();
        let lease_owner = lease_owner.to_string();
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            let state_json = serde_json::to_value(&state)?;
            let result = sqlx::query(
                "UPDATE process_instances SET state = $1 WHERE instance_id = $2 AND lease_owner = $3",
            )
            .bind(&state_json)
            .bind(id)
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await?;

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
            let flags_json = serde_json::to_value(&flags)?;
            let result = sqlx::query(
                "UPDATE process_instances SET flags = $1 WHERE instance_id = $2 AND lease_owner = $3",
            )
            .bind(&flags_json)
            .bind(id)
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await?;

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
                "UPDATE process_instances SET domain_payload = $1, domain_payload_hash = $2 WHERE instance_id = $3 AND lease_owner = $4",
            )
            .bind(&payload)
            .bind(&hash[..])
            .bind(id)
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await?;

            tx.assert_rows_affected(&result, 1, "update_instance_payload")
        })).await
    }

    // ── Fibers ──

    async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        let stack = serde_json::to_value(&fiber.stack)?;
        let regs = serde_json::to_value(&fiber.regs)?;
        let wait_state = serde_json::to_value(&fiber.wait)?;

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
        .bind(fiber.pc as i32)
        .bind(&stack)
        .bind(&regs)
        .bind(&wait_state)
        .bind(fiber.loop_epoch as i32)
        .bind(&tenant_id)
        .execute(&mut *tx)
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

        tx.commit().await?;
        Ok(())
    }

    async fn load_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<Option<Fiber>> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        let row = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE instance_id = $1 AND fiber_id = $2",
        )
        .bind(instance_id)
        .bind(fiber_id)
        .fetch_optional(&mut *tx)
        .await?;

        tx.commit().await?;

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
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        let rows = sqlx::query(
            "SELECT fiber_id, pc, stack, regs, wait_state, loop_epoch FROM fibers WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_all(&mut *tx)
        .await?;

        tx.commit().await?;

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
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        sqlx::query("DELETE FROM fibers WHERE instance_id = $1 AND fiber_id = $2")
            .bind(instance_id)
            .bind(fiber_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn delete_all_fibers(&self, instance_id: Uuid) -> Result<()> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        sqlx::query("DELETE FROM fibers WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    // ── Join barriers ──

    async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

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
        .await?;

        use sqlx::Row;
        let count: i16 = row.get("arrive_count");
        tx.commit().await?;
        Ok(count as u16)
    }

    async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

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
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn join_delete_all(&self, instance_id: Uuid) -> Result<()> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        sqlx::query("DELETE FROM join_barriers WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
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
        let tenant_id_str = tenant_id.to_string();
        let tenant_id_for_query = tenant_id_str.clone();
        let msg_id_str = msg_id.to_string();
        self.with_tenant(&tenant_id_str, |tx| Box::pin(async move {
            let result = sqlx::query(
                r#"
                INSERT INTO message_dedupe (tenant_id, instance_id, msg_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (tenant_id, instance_id, msg_id) DO NOTHING
                "#,
            )
            .bind(&tenant_id_for_query)
            .bind(instance_id)
            .bind(&msg_id_str)
            .execute(&mut **tx)
            .await?;
            Ok(result.rows_affected() == 1)
        })).await
    }

    // ── Job queue ──

    async fn enqueue_job(&self, activation: &JobActivation) -> Result<()> {
        let lease_owner = "unused";
        let tenant_id = activation.tenant_id.clone();
        let activation = activation.clone();
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            Self::enqueue_job_inner(tx, &activation).await
        })).await
    }

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &str,
        worker_id: &str,
        lease_ms: u64,
    ) -> Result<Vec<JobActivation>> {
        let task_types = task_types.to_vec();
        let tenant_id_owned = tenant_id.to_string();
        let worker_id_owned = worker_id.to_string();
        self.execute_tenant_scoped(&tenant_id_owned, &worker_id_owned, |tx| Box::pin(async move {
            Self::dequeue_jobs_inner(tx, &task_types, max, lease_ms).await
        })).await
    }

    async fn ack_job(&self, tenant_id: &str, job_key: &str) -> Result<()> {
        let lease_owner = "unused";
        let job_key = job_key.to_string();
        let tenant_id = tenant_id.to_string();
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            Self::ack_job_inner(tx, &job_key).await
        })).await
    }

    async fn validate_job_claim(
        &self,
        tenant_id: &str,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> Result<bool> {
        let lease_owner = "unused";
        let tenant_id = tenant_id.to_string();
        let job_key = job_key.to_string();
        let worker_id = worker_id.to_string();
        let claim_token = claim_token.to_string();
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
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
            .await?;
            Ok(row.is_some())
        })).await
    }

    async fn retry_claimed_job(
        &self,
        tenant_id: &str,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        not_before_ms: i64,
    ) -> Result<bool> {
        let tenant_id = tenant_id.to_string();
        let job_key = job_key.to_string();
        let claim_token = claim_token.to_string();
        let error_class = error_class.to_string();
        let error_message = error_message.to_string();
        self.execute_tenant_scoped(&tenant_id, worker_id, |tx| Box::pin(async move {
            Self::retry_claimed_job_inner(tx, &job_key, &claim_token, &error_class, &error_message, not_before_ms).await
        })).await
    }

    async fn dead_letter_claimed_job(
        &self,
        tenant_id: &str,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        incident_id: Uuid,
    ) -> Result<bool> {
        let tenant_id = tenant_id.to_string();
        let job_key = job_key.to_string();
        let claim_token = claim_token.to_string();
        let error_class = error_class.to_string();
        let error_message = error_message.to_string();
        self.execute_tenant_scoped(&tenant_id, worker_id, |tx| Box::pin(async move {
            Self::dead_letter_claimed_job_inner(tx, &job_key, &claim_token, &error_class, &error_message, incident_id).await
        })).await
    }

    async fn cancel_jobs_for_instance(&self, instance_id: Uuid) -> Result<Vec<String>> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            Self::cancel_jobs_for_instance_inner(tx, instance_id).await
        })).await
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
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, tenant_id).await?;

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
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

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
        let claim_until_ms = (chrono::Utc::now() + chrono::Duration::milliseconds(claim_ms as i64)).timestamp_millis();
        let claim_until = epoch_ms_to_datetime(claim_until_ms);
        let claim_token = Uuid::now_v7().to_string();
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, tenant_id).await?;

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
        .fetch_optional(&mut *tx)
        .await?;

        tx.commit().await?;

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
        let lease_owner = "unused";
        let tenant_id = instance.tenant_id.clone();
        if tenant_id != message.message.tenant_id {
            return Err(anyhow::anyhow!("tenant_id mismatch: cross-tenant message consumption blocked"));
        }
        let instance = instance.clone();
        let fiber = fiber.clone();
        let message = message.clone();
        let payload_update = payload_update.cloned();
        let events = events.to_vec();
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
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
            .await?;

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

            let flags = serde_json::to_value(&instance.flags)?;
            let counters = serde_json::to_value(&instance.counters)?;
            let join_expected = serde_json::to_value(&instance.join_expected)?;
            let state = serde_json::to_value(&instance.state)?;

            let process_instances_result = sqlx::query(
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
            .execute(&mut *tx.tx)
            .await?;

            tx.assert_rows_affected(&process_instances_result, 1, "atomic_consume_buffered_message: process_instances update")?;

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
                .await?;
            }

            let stack = serde_json::to_value(&fiber.stack)?;
            let regs = serde_json::to_value(&fiber.regs)?;
            let wait_state = serde_json::to_value(&fiber.wait)?;

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
            .bind(fiber.pc as i32)
            .bind(&stack)
            .bind(&regs)
            .bind(&wait_state)
            .bind(fiber.loop_epoch as i32)
            .bind(&tx.tenant_id)
            .execute(&mut *tx.tx)
            .await?;

            for event in &events {
                let event_json = serde_json::to_value(event)?;
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
                .await?;
            }

            if !events.is_empty() {
                notify_event_tx(&mut tx.tx, instance.instance_id).await?;
            }
            Ok(true)
        })).await
    }

    async fn release_buffered_message_claim(
        &self,
        message: &ClaimedBufferedMessage,
    ) -> Result<bool> {
        let tenant_id = &message.message.tenant_id;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, tenant_id).await?;

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
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> Result<u32> {
        let tenants = self.list_tenants().await?;
        let mut total_affected = 0;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = Self::set_tenant_context(&mut tx, &tenant_id).await {
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
            .await?;
            tx.commit().await?;
            total_affected += result.rows_affected() as u32;
        }
        Ok(total_affected)
    }

    async fn prune_expired_messages(&self) -> Result<u32> {
        let tenants = self.list_tenants().await?;
        let mut total_pruned = 0;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = Self::set_tenant_context(&mut tx, &tenant_id).await {
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
            .await?;

            use sqlx::Row;
            for row in &rows {
                let instance_id: Option<Uuid> = row.get("process_instance_id");
                if let Some(instance_id) = instance_id {
                    let event = RuntimeEvent::BufferedMessageExpired {
                        message_name: row.get("message_name"),
                        correlation_key: row.get("correlation_key"),
                        msg_id: row.get("msg_id"),
                    };
                    let event_json = serde_json::to_value(&event)?;

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
                    .context("prune_expired_messages: failed to append BufferedMessageExpired event")?;

                    notify_event_tx(&mut tx, instance_id).await?;
                }
            }

            tx.commit().await?;
            total_pruned += rows.len() as u32;
        }
        Ok(total_pruned)
    }

    // ── Event log ──

    async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;
        let event_json = serde_json::to_value(event)?;

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
        .await?;

        use sqlx::Row;
        let seq: i64 = row.get("seq");
        notify_event_tx(&mut tx, instance_id).await?;
        tx.commit().await?;
        Ok(seq as u64)
    }

    async fn batch_append_events(&self, instance_id: Uuid, events: &[RuntimeEvent]) -> Result<u64> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;
        let mut last_seq = 0;
        for event in events {
            let event_json = serde_json::to_value(event)?;
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
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        let rows = sqlx::query(
            "SELECT seq, event FROM event_log WHERE instance_id = $1 AND seq >= $2 ORDER BY seq",
        )
        .bind(instance_id)
        .bind(from_seq as i64)
        .fetch_all(&mut *tx)
        .await?;

        tx.commit().await?;

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
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

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
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn load_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> Result<Option<String>> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        let row = sqlx::query(
            "SELECT domain_payload FROM payload_history WHERE instance_id = $1 AND payload_hash = $2",
        )
        .bind(instance_id)
        .bind(&hash[..])
        .fetch_optional(&mut *tx)
        .await?;

        tx.commit().await?;

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
        let tenant_id = self.resolve_tenant_id(incident.process_instance_id)
            .await
            .map_err(|e| anyhow::anyhow!("save_incident for incident {}: {}", incident.incident_id, e))?;
        let lease_owner = "unused";
        let incident = incident.clone();
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            Self::save_incident_inner(tx, &incident).await
        })).await
    }

    async fn load_incidents(&self, instance_id: Uuid) -> Result<Vec<Incident>> {
        let tenant_id = match self.resolve_tenant_id(instance_id).await {
            Ok(t) => t,
            Err(_) => return Ok(vec![]),
        };
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
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
            .fetch_all(&mut *tx.tx)
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
        })).await
    }

    // ── Atomic compound operations ──

    async fn atomic_start(
        &self,
        tenant_id: &str,
        lease_owner: &str,
        instance: &ProcessInstance,
        root_fiber: &Fiber,
        event: &RuntimeEvent,
    ) -> Result<u64> {
        // Register the tenant on first use. Idempotent — ON CONFLICT DO NOTHING.
        // Runs outside the main transaction so the tenants row is visible to
        // the scheduler even if the main transaction rolls back.
        self.ensure_tenant(&instance.tenant_id).await?;

        let lease_owner = lease_owner.to_string();
        let tenant_id = tenant_id.to_string();
        let instance = instance.clone();
        let root_fiber = root_fiber.clone();
        let event = event.clone();

        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            // 1. INSERT process_instances
            let flags = serde_json::to_value(&instance.flags)?;
            let counters = serde_json::to_value(&instance.counters)?;
            let join_expected = serde_json::to_value(&instance.join_expected)?;
            let state = serde_json::to_value(&instance.state)?;
            let session_stack = serde_json::to_value(&instance.session_stack)?;
            let created_at = epoch_ms_to_datetime(instance.created_at);
            let integrity_hash = compute_instance_integrity_hash(&instance);

            sqlx::query(
                r#"
                INSERT INTO process_instances (
                    instance_id, tenant_id, process_key, bytecode_version, domain_payload,
                    domain_payload_hash, session_stack, flags, counters, join_expected, state,
                    correlation_id, entry_id, runbook_id, created_at,
                    plan_hash, current_node_id, placeholder_values, integrity_hash
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                          $16, $17, $18, $19)
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
            .bind(&integrity_hash[..])
            .execute(&mut *tx.tx)
            .await?;

            // 2. INSERT fiber
            let stack = serde_json::to_value(&root_fiber.stack)?;
            let regs = serde_json::to_value(&root_fiber.regs)?;
            let wait_state = serde_json::to_value(&root_fiber.wait)?;

            sqlx::query(
                r#"
                INSERT INTO fibers (instance_id, fiber_id, pc, stack, regs, wait_state, loop_epoch, tenant_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(instance.instance_id)
            .bind(root_fiber.fiber_id)
            .bind(root_fiber.pc as i32)
            .bind(&stack)
            .bind(&regs)
            .bind(&wait_state)
            .bind(root_fiber.loop_epoch as i32)
            .bind(&tx.tenant_id)
            .execute(&mut *tx.tx)
            .await?;

            // 3. Append event (sequence + log)
            let event_json = serde_json::to_value(event)?;

            let row = sqlx::query(
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
                RETURNING seq
                "#,
            )
            .bind(instance.instance_id)
            .bind(&event_json)
            .bind(&tx.tenant_id)
            .fetch_one(&mut *tx.tx)
            .await?;

            use sqlx::Row;
            let seq: i64 = row.get("seq");

            notify_event_tx(&mut tx.tx, instance.instance_id).await?;
            Ok(seq as u64)
        })).await
    }

    async fn atomic_complete(
        &self,
        tenant_id: &str,
        lease_owner: &str,
        instance: &ProcessInstance,
        completion: &JobCompletion,
        events: &[RuntimeEvent],
    ) -> Result<()> {
        let lease_owner = lease_owner.to_string();
        let tenant_id = tenant_id.to_string();
        let instance = instance.clone();
        let completion = completion.clone();
        let events = events.to_vec();

        self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
            // 1. UPSERT process_instances
            let flags = serde_json::to_value(&instance.flags)?;
            let counters = serde_json::to_value(&instance.counters)?;
            let join_expected = serde_json::to_value(&instance.join_expected)?;
            let state = serde_json::to_value(&instance.state)?;
            let session_stack = serde_json::to_value(&instance.session_stack)?;
            let created_at = epoch_ms_to_datetime(instance.created_at);

            let result = sqlx::query(
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
                WHERE process_instances.lease_owner = $16
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
            .bind(&tx.lease_owner)
            .execute(&mut *tx.tx)
            .await?;

            tx.assert_rows_affected(&result, 1, "atomic_complete: process_instances update")?;

            // 2. INSERT dedupe_cache ON CONFLICT
            let completion_json = serde_json::to_value(&completion)?;
            sqlx::query(
                r#"
                INSERT INTO dedupe_cache (job_key, completion)
                VALUES ($1, $2)
                ON CONFLICT (job_key) DO UPDATE SET completion = EXCLUDED.completion
                "#,
            )
            .bind(&completion.job_key)
            .bind(&completion_json)
            .execute(&mut *tx.tx)
            .await?;

            // 3. INSERT payload_history ON CONFLICT
            sqlx::query(
                r#"
                INSERT INTO payload_history (instance_id, payload_hash, domain_payload, tenant_id)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (instance_id, payload_hash) DO NOTHING
                "#,
            )
            .bind(instance.instance_id)
            .bind(&instance.domain_payload_hash[..])
            .bind(instance.domain_payload.as_ref())
            .bind(&tx.tenant_id)
            .execute(&mut *tx.tx)
            .await?;

            // 4. Append completion events in the same transaction.
            for event in &events {
                let event_json = serde_json::to_value(event)?;
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
                .await?;
            }

            // 5. ACK job in the same transaction as completion state.
            sqlx::query("DELETE FROM job_queue WHERE job_key = $1")
                .bind(&completion.job_key)
                .execute(&mut *tx.tx)
                .await?;

            if !events.is_empty() {
                notify_event_tx(&mut tx.tx, instance.instance_id).await?;
            }
            Ok(())
        })).await
    }

    // ── Durability maintenance ──

    async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> Result<u32> {
        let lease_owner = "unused";
        let tenants = self.list_tenants().await?;
        let mut total_count = 0;
        for tenant_id in tenants {
            let reclaims = self.execute_tenant_scoped(&tenant_id, &lease_owner, |tx| Box::pin(async move {
                Self::reclaim_stale_jobs_inner(tx, timeout_ms).await
            })).await?;

            total_count += reclaims.len() as u32;
            for item in reclaims {
                self.append_event(
                    item.process_instance_id,
                    &RuntimeEvent::JobReclaimed {
                        job_key: item.job_key,
                        previous_worker_id: item.previous_worker_id,
                    },
                )
                .await?;
            }
        }
        Ok(total_count)
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
        let tenant_id_owned = tenant_id.to_string();
        let tenant_id_query = tenant_id.to_string();
        let lease_owner = "unused";
        self.execute_tenant_scoped(&tenant_id_owned, &lease_owner, |tx| Box::pin(async move {
            let rows = sqlx::query(
                r#"SELECT instance_id FROM process_instances WHERE tenant_id = $1 AND state = '"Running"'::jsonb"#,
            )
            .bind(&tenant_id_query)
            .fetch_all(&mut *tx.tx)
            .await?;

            use sqlx::Row;
            Ok(rows.iter().map(|r| r.get("instance_id")).collect())
        })).await
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &str,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> Result<Vec<Uuid>> {
        let tenant_id_owned = tenant_id.to_string();
        let owner_owned = owner.to_string();
        let tenant_id_query = tenant_id.to_string();
        let owner_query = owner.to_string();
        self.execute_tenant_scoped(&tenant_id_owned, &owner_owned, |tx| Box::pin(async move {
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
            .bind(&tenant_id_query)
            .bind(&owner_query)
            .bind(limit as i64)
            .bind(lease_ms as f64)
            .fetch_all(&mut *tx.tx)
            .await?;

            use sqlx::Row;
            Ok(rows.iter().map(|r| r.get("instance_id")).collect())
        })).await
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> Result<bool> {
        self.execute_tenant_scoped(tenant_id, owner, |tx| Box::pin(async move {
            Self::claim_instance_for_transition_inner(tx, instance_id, lease_ms).await
        })).await
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
    ) -> Result<()> {
        self.execute_tenant_scoped(tenant_id, owner, |tx| Box::pin(async move {
            Self::release_instance_transition_inner(tx, instance_id).await
        })).await
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
        lease_owner: &str,
        detection_point: &str,
    ) -> Result<()> {
        let lease_owner = lease_owner.to_string();
        let tenant_id_str = tenant_id.to_string();
        let detection_point_str = detection_point.to_string();
        self.execute_tenant_scoped(&tenant_id_str, &lease_owner, |tx| Box::pin(async move {
            Self::quarantine_instance_inner(tx, instance_id).await?;

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
            let event_json = serde_json::to_value(&event)?;

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
            .context("quarantine_instance: failed to append InstanceQuarantined event")?;

            notify_event_tx(&mut tx.tx, instance_id).await?;
            Ok(())
        })).await?;

        tracing::warn!(
            instance_id = %instance_id,
            tenant_id = %tenant_id,
            detection_point = %detection_point,
            "A19: instance quarantined due to integrity hash mismatch"
        );

        Ok(())
    }

    async fn join_get(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
        let tenant_id = self.resolve_tenant_id(instance_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::set_tenant_context(&mut tx, &tenant_id).await?;

        let row = sqlx::query(
            "SELECT arrive_count FROM join_barriers WHERE instance_id = $1 AND join_id = $2",
        )
        .bind(instance_id)
        .bind(join_id as i32)
        .fetch_optional(&mut *tx)
        .await?;

        let count = match row {
            None => 0,
            Some(row) => {
                use sqlx::Row;
                let count: i16 = row.get("arrive_count");
                count as u16
            }
        };

        tx.commit().await?;
        Ok(count)
    }

    async fn commit_tick(
        &self,
        instance_id: Uuid,
        tenant_id: &str,
        lease_owner: &str,
        ops: &[TickOperation],
    ) -> Result<()> {
        let lease_owner = lease_owner.to_string();
        let ops = ops.to_vec();
        self.execute_tenant_scoped(tenant_id, &lease_owner, |tx| Box::pin(async move {
            for op in &ops {
                Self::apply_op(tx, instance_id, op).await?;
            }

            // Post-ops check in the same transaction: release lease if parked/terminal
            let state_val: serde_json::Value = sqlx::query_scalar(
                "SELECT state FROM process_instances WHERE instance_id = $1"
            )
            .bind(instance_id)
            .fetch_one(&mut *tx.tx)
            .await?;

            let parsed_state: ProcessState = serde_json::from_value(state_val)?;

            let has_running_fiber: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM fibers WHERE instance_id = $1 AND wait_state = '\"Running\"'::jsonb)"
            )
            .bind(instance_id)
            .fetch_one(&mut *tx.tx)
            .await?;

            let is_runnable = matches!(parsed_state, ProcessState::Running) && has_running_fiber;

            if !is_runnable {
                sqlx::query(
                    r#"
                    UPDATE process_instances
                    SET lease_owner = NULL,
                        lease_until = now() - interval '1 second'
                    WHERE instance_id = $1
                    "#
                )
                .bind(instance_id)
                .execute(&mut *tx.tx)
                .await?;
            }

            Ok(())
        })).await
    }
}

impl PostgresProcessStore {
    async fn apply_op(
        tx: &mut TenantTx<'_>,
        instance_id: Uuid,
        op: &TickOperation,
    ) -> Result<()> {
        match op {
            TickOperation::SaveFiber { fiber } => {
                let stack = serde_json::to_value(&fiber.stack)?;
                let regs = serde_json::to_value(&fiber.regs)?;
                let wait_state = serde_json::to_value(&fiber.wait)?;

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
                .bind(fiber.pc as i32)
                .bind(&stack)
                .bind(&regs)
                .bind(&wait_state)
                .bind(fiber.loop_epoch as i32)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await?;

                if result.rows_affected() == 0 {
                    return Err(anyhow!(
                        "save_fiber affected 0 rows for instance {} fiber {}; \
                         parent instance may be missing or RLS rejected",
                        instance_id,
                        fiber.fiber_id
                    ));
                }
            }
            TickOperation::DeleteFiber { fiber_id } => {
                sqlx::query("DELETE FROM fibers WHERE instance_id = $1 AND fiber_id = $2")
                    .bind(instance_id)
                    .bind(fiber_id)
                    .execute(&mut *tx.tx)
                    .await?;
            }
            TickOperation::DeleteAllFibers => {
                sqlx::query("DELETE FROM fibers WHERE instance_id = $1")
                    .bind(instance_id)
                    .execute(&mut *tx.tx)
                    .await?;
            }
            TickOperation::JoinArrive { join_id } => {
                sqlx::query(
                    r#"
                    INSERT INTO join_barriers (instance_id, join_id, arrive_count, tenant_id)
                    VALUES ($1, $2, 1, $3)
                    ON CONFLICT (instance_id, join_id) DO UPDATE
                        SET arrive_count = join_barriers.arrive_count + 1
                    "#,
                )
                .bind(instance_id)
                .bind(*join_id as i32)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await?;
            }
            TickOperation::JoinReset { join_id } => {
                sqlx::query(
                    r#"
                    INSERT INTO join_barriers (instance_id, join_id, arrive_count, tenant_id)
                    VALUES ($1, $2, 0, $3)
                    ON CONFLICT (instance_id, join_id) DO UPDATE
                        SET arrive_count = 0
                    "#,
                )
                .bind(instance_id)
                .bind(*join_id as i32)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await?;
            }
            TickOperation::EnqueueJob { job } => {
                Self::enqueue_job_inner(tx, job).await?;
            }
            TickOperation::CancelJobsForInstance => {
                Self::cancel_jobs_for_instance_inner(tx, instance_id).await?;
            }
            TickOperation::AppendEvent { event } => {
                let event_json = serde_json::to_value(event)?;
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
                .await?;
            }
            TickOperation::SaveIncident { incident } => {
                Self::save_incident_inner(tx, incident).await?;
            }
            TickOperation::SaveInstance { instance } => {
                let flags = serde_json::to_value(&instance.flags)?;
                let counters = serde_json::to_value(&instance.counters)?;
                let join_expected = serde_json::to_value(&instance.join_expected)?;
                let state = serde_json::to_value(&instance.state)?;
                let session_stack = serde_json::to_value(&instance.session_stack)?;
                let created_at = epoch_ms_to_datetime(instance.created_at);
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
                    WHERE process_instances.lease_owner = $20
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
                .bind(&tx.lease_owner)
                .execute(&mut *tx.tx)
                .await?;

                tx.assert_rows_affected(&result, 1, "save_instance")?;

                sqlx::query(
                    r#"
                    INSERT INTO payload_history (instance_id, payload_hash, domain_payload, tenant_id)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (instance_id, payload_hash) DO NOTHING
                    "#,
                )
                .bind(instance.instance_id)
                .bind(&instance.domain_payload_hash[..])
                .bind(instance.domain_payload.as_ref())
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await?;
            }
            TickOperation::UpdateInstanceState { state } => {
                let state_val = serde_json::to_value(state)?;
                let result = sqlx::query(
                    "UPDATE process_instances SET state = $1 WHERE instance_id = $2 AND lease_owner = $3",
                )
                .bind(&state_val)
                .bind(instance_id)
                .bind(&tx.lease_owner)
                .execute(&mut *tx.tx)
                .await?;

                tx.assert_rows_affected(&result, 1, "update_instance_state")?;
            }
            TickOperation::ReleaseBufferedMessageClaim { message } => {
                sqlx::query(
                    r#"
                    UPDATE message_buffer
                    SET claim_token = NULL,
                        claim_until = NULL
                    WHERE tenant_id = $1
                      AND message_name = $2
                      AND correlation_key = $3
                      AND msg_id = $4
                      AND claim_token = $5
                    "#,
                )
                .bind(&message.message.tenant_id)
                .bind(&message.message.message_name)
                .bind(&message.message.correlation_key)
                .bind(&message.message.msg_id)
                .bind(&message.claim_token)
                .execute(&mut *tx.tx)
                .await?;
            }
            TickOperation::ConsumeBufferedMessage { message } => {
                let result = sqlx::query(
                    r#"
                    UPDATE message_buffer
                    SET consumed_at = now(),
                        status = 'consumed'
                    WHERE tenant_id = $1
                      AND message_name = $2
                      AND correlation_key = $3
                      AND msg_id = $4
                      AND claim_token = $5
                      AND claim_until = $6
                      AND consumed_at IS NULL
                    "#,
                )
                .bind(&message.message.tenant_id)
                .bind(&message.message.message_name)
                .bind(&message.message.correlation_key)
                .bind(&message.message.msg_id)
                .bind(&message.claim_token)
                .bind(epoch_ms_to_datetime(message.claim_until))
                .execute(&mut *tx.tx)
                .await?;

                if result.rows_affected() != 1 {
                    return Err(anyhow::anyhow!("failed to consume message: claim expired or already consumed"));
                }
            }
            TickOperation::InsertPendingInvocation { pending } => {
                sqlx::query(
                    r#"
                    INSERT INTO bpmn_pending_invocation (
                        callout_id, process_instance_id, node_id, target_domain, verb_id,
                        idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at, tenant_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(pending.callout_id)
                .bind(pending.process_instance_id)
                .bind(&pending.node_id)
                .bind(&pending.target_domain)
                .bind(&pending.verb_id)
                .bind(pending.idempotency_key)
                .bind(pending.execution_id)
                .bind(pending.submitted_at)
                .bind(pending.ack_received_at)
                .bind(pending.timeout_at)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await?;
            }
            TickOperation::InsertOutbox { id, target_domain, target_endpoint, payload, idempotency_key, callout_id } => {
                sqlx::query(
                    r#"
                    INSERT INTO dsl_bus.outbox (
                        id, target_domain, target_endpoint, payload, idempotency_key,
                        execution_id, callout_id, status, attempt_count, next_attempt_at,
                        last_error, created_at, submitted_at, tenant_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                    ON CONFLICT (idempotency_key, target_endpoint) DO NOTHING
                    "#,
                )
                .bind(id)
                .bind(target_domain)
                .bind(target_endpoint)
                .bind(payload)
                .bind(idempotency_key)
                .bind(None::<Uuid>)
                .bind(callout_id)
                .bind("pending")
                .bind(0i16)
                .bind(chrono::Utc::now())
                .bind(None::<String>)
                .bind(chrono::Utc::now())
                .bind(None::<chrono::DateTime<chrono::Utc>>)
                .bind(&tx.tenant_id)
                .execute(&mut *tx.tx)
                .await?;
            }
            TickOperation::TakePendingInvocation { execution_id } => {
                let result = sqlx::query(
                    r#"
                    DELETE FROM bpmn_pending_invocation
                    WHERE execution_id = $1
                    "#,
                )
                .bind(execution_id)
                .execute(&mut *tx.tx)
                .await?;
                if result.rows_affected() != 1 {
                    return Err(anyhow::anyhow!(bpmn_lite_store::store::AlreadyConsumedError));
                }
            }
        }
        Ok(())
    }
}

impl PostgresProcessStore {
    async fn enqueue_job_inner(tx: &mut TenantTx<'_>, activation: &JobActivation) -> Result<()> {
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
        .execute(&mut *tx.tx)
        .await?;

        if result.rows_affected() == 0 {
            let existing: Option<(String,)> =
                sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = $1")
                    .bind(&activation.job_key)
                    .fetch_optional(&mut *tx.tx)
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
            tracing::debug!(
                job_key = %activation.job_key,
                "enqueue_job: duplicate job_key, idempotent no-op"
            );
        }

        Ok(())
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
        .bind(&tx.tenant_id)
        .bind(&tx.lease_owner)
        .bind(lease_ms as f64)
        .fetch_all(&mut *tx.tx)
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

    async fn ack_job_inner(tx: &mut TenantTx<'_>, job_key: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM job_queue WHERE job_key = $1")
            .bind(job_key)
            .execute(&mut *tx.tx)
            .await?;

        if result.rows_affected() == 0 {
            tracing::debug!(
                job_key = %job_key,
                "ack_job: 0 rows deleted (already acked, expired, or cancelled)"
            );
        }

        Ok(())
    }

    async fn retry_claimed_job_inner(
        tx: &mut TenantTx<'_>,
        job_key: &str,
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
        .bind(&tx.lease_owner)
        .bind(claim_token)
        .bind(epoch_ms_to_datetime(not_before_ms))
        .bind(error_class)
        .bind(error_message)
        .execute(&mut *tx.tx)
        .await?;

        tx.assert_rows_affected(&result, 1, "retry_claimed_job")?;
        Ok(true)
    }

    async fn dead_letter_claimed_job_inner(
        tx: &mut TenantTx<'_>,
        job_key: &str,
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
        .bind(&tx.lease_owner)
        .bind(claim_token)
        .bind(error_class)
        .bind(error_message)
        .bind(incident_id)
        .execute(&mut *tx.tx)
        .await?;

        tx.assert_rows_affected(&result, 1, "dead_letter_claimed_job")?;
        Ok(true)
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
        .await?;

        use sqlx::Row;
        Ok(rows.iter().map(|r| r.get("job_key")).collect())
    }

    async fn quarantine_instance_inner(
        tx: &mut TenantTx<'_>,
        instance_id: Uuid,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE process_instances \
             SET quarantine_state = 'integrity_violation' \
             WHERE instance_id = $1",
        )
        .bind(instance_id)
        .execute(&mut *tx.tx)
        .await
        .context("quarantine_instance: failed to set quarantine_state")?;

        tx.assert_rows_affected(&result, 1, "quarantine_instance")
    }

    async fn save_incident_inner(tx: &mut TenantTx<'_>, incident: &Incident) -> Result<()> {
        let error_class = serde_json::to_value(&incident.error_class)?;
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
        .bind(incident.bytecode_addr as i32)
        .bind(&error_class)
        .bind(&incident.message)
        .bind(incident.retry_count as i32)
        .bind(created_at)
        .bind(resolved_at)
        .bind(&incident.resolution)
        .bind(&tx.tenant_id)
        .execute(&mut *tx.tx)
        .await?;

        tx.assert_rows_affected(&result, 1, "save_incident")
    }

    async fn reclaim_stale_jobs_inner(tx: &mut TenantTx<'_>, timeout_ms: u64) -> Result<Vec<StaleReclaimInfo>> {
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
        .await?;

        use sqlx::Row;
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            results.push(StaleReclaimInfo {
                job_key: row.get("job_key"),
                process_instance_id: row.get("process_instance_id"),
                previous_worker_id: row.get("previous_worker_id"),
            });
        }
        Ok(results)
    }

    async fn claim_instance_for_transition_inner(
        tx: &mut TenantTx<'_>,
        instance_id: Uuid,
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
        .bind(&tx.tenant_id)
        .bind(instance_id)
        .bind(&tx.lease_owner)
        .bind(lease_ms as f64)
        .execute(&mut *tx.tx)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    async fn release_instance_transition_inner(
        tx: &mut TenantTx<'_>,
        instance_id: Uuid,
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
        .bind(&tx.tenant_id)
        .bind(instance_id)
        .bind(&tx.lease_owner)
        .execute(&mut *tx.tx)
        .await?;

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
    use bpmn_lite_store::pending::{PendingInvocation, PendingInvocationStore};
    use crate::pending_store::PostgresPendingInvocationStore;
    use sqlx::PgPool;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

    async fn setup() -> (PgPool, PostgresProcessStore, tokio::sync::MutexGuard<'static, ()>) {
        let guard = crate::test_lock::get_mutex().lock().await;
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
        let pool = PgPool::connect(&url).await.expect("connect to db");

        // Run migrations
        let migrator = sqlx::migrate!("./migrations");
        migrator.run(&pool)
            .await
            .expect("run migrations");

        let db_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        let grant_sql = format!("GRANT CONNECT, TEMPORARY ON DATABASE \"{}\" TO bpmn_lite_app", db_name);
        sqlx::query(&grant_sql).execute(&pool).await.unwrap();
        sqlx::query("GRANT USAGE ON SCHEMA public TO bpmn_lite_app").execute(&pool).await.unwrap();
        sqlx::query("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO bpmn_lite_app").execute(&pool).await.unwrap();
        sqlx::query("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO bpmn_lite_app").execute(&pool).await.unwrap();

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
        let bus_admin_pool = PgPool::connect(&bus_admin_url).await.expect("connect to db for bus migrations");
        dsl_bus_storage::migrate(&bus_admin_pool)
            .await
            .expect("run bus migrations");
        bus_admin_pool.close().await;

        // Grant bpmn_lite_app DML-only privileges on dsl_bus tables and sequences
        sqlx::query("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dsl_bus TO bpmn_lite_app").execute(&pool).await.unwrap();
        sqlx::query("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA dsl_bus TO bpmn_lite_app").execute(&pool).await.unwrap();

        use std::str::FromStr;
        let mut options = sqlx::postgres::PgConnectOptions::from_str(&url).expect("parse db url");
        options = options.username("bpmn_lite_app").password("bpmn_lite_app_dev_password");
        let app_pool = PgPool::connect_with(options).await.expect("connect as bpmn_lite_app");

        // Truncate all tables
        sqlx::query("TRUNCATE dsl_bus.outbox CASCADE")
            .execute(&pool)
            .await
            .unwrap();

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

        let store = PostgresProcessStore::new(app_pool);
        (pool, store, guard)
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
        let (_pool, store, _lock) = setup().await;
        let id = Uuid::now_v7();
        let inst = make_instance(id);

        store.save_instance("default", &inst).await.unwrap();
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
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let fid = Uuid::now_v7();

        // Need an instance first (FK constraint)
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
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
        let (_pool, store, _lock) = setup().await;
        let task_type = "create_case".to_string();
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        store.ack_job("default", &batch1[0].job_key).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;

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
    #[ignore]
    async fn test_pg_incidents() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
        let id = Uuid::now_v7();
        store.save_instance("test-owner", &make_instance(id)).await.unwrap();

        // Claim transition
        let claimed = store.claim_instance_for_transition("default", id, "test-owner", 30000).await.unwrap();
        assert!(claimed);

        // Update state
        let new_state = ProcessState::Completed { at: 1700001000000 };
        store
            .update_instance_state("default", "test-owner", id, new_state.clone())
            .await
            .unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();
        assert_eq!(loaded.state, new_state);

        // Update flags
        let new_flags = BTreeMap::from([(5, Value::Bool(false))]);
        store.update_instance_flags("default", "test-owner", id, &new_flags).await.unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();
        assert_eq!(loaded.flags.len(), 1);
        assert_eq!(loaded.flags[&5], Value::Bool(false));

        // Update payload
        let new_payload = r#"{"updated":true}"#;
        let new_hash = test_hash(new_payload);
        store
            .update_instance_payload("default", "test-owner", id, new_payload, &new_hash)
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
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let task_type = "concurrent_task".to_string();
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
        let task_type = "cancel_test".to_string();

        let iid_a = Uuid::now_v7();
        let iid_b = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid_a)).await.unwrap();

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
    #[ignore]
    async fn test_a18_enqueue_job_duplicate_is_idempotent() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        store.save_instance("default", &make_instance(iid)).await.unwrap();

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
        let (_pool, store, _lock) = setup().await;
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
    #[ignore]
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
    // These tests require a real Postgres database and are gated by #[ignore].
    // They verify: hash stored at creation; load returns it; tampering surfaces;
    // quarantined instances are skipped by claim_running_instances.

    /// T-A19-PG-1: save_instance stores an integrity hash; load_instance returns it.
    #[tokio::test]
    #[ignore]
    async fn test_a19_hash_stored_on_save_and_loaded() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance("default", &make_instance(iid)).await.unwrap();

        let loaded = store.load_instance(iid).await.unwrap().unwrap();
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
    #[ignore]
    async fn test_a19_hash_not_overwritten_on_update() {
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance("test-owner", &make_instance(iid)).await.unwrap();

        // Claim it
        let claimed = store.claim_instance_for_transition("default", iid, "test-owner", 30000).await.unwrap();
        assert!(claimed);

        let original_hash = store
            .load_instance(iid)
            .await
            .unwrap()
            .unwrap()
            .integrity_hash;

        // Re-save (simulates tick updating state/flags).
        let inst = store.load_instance(iid).await.unwrap().unwrap();
        store.save_instance("test-owner", &inst).await.unwrap();

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
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance("default", &make_instance(iid)).await.unwrap();

        // The immutability trigger (migration 029) blocks tenant_id mutation
        // at the DB level — defense in depth above the application integrity
        // check. Verify the trigger fires with a P0001 RAISE EXCEPTION.
        let tamper_result = sqlx::query(
            "UPDATE process_instances SET tenant_id = 'evil-tenant' WHERE instance_id = $1",
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

    /// T-A19-PG-4: quarantine_instance marks the row and logs an event.
    #[tokio::test]
    #[ignore]
    async fn test_a19_quarantine_marks_row_and_logs_event() {
        use sqlx::Row;
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance("default", &make_instance(iid)).await.unwrap();
        store
            .quarantine_instance(iid, "default", "default", "grpc_handler")
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
        let (_pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();

        store.save_instance("default", &make_instance(iid)).await.unwrap();

        // Quarantine the instance.
        store
            .quarantine_instance(iid, "default", "default", "scheduler_claim")
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
    #[ignore]
    async fn test_l0_ensure_tenant_sets_pool_id() {
        let (pool, store, _lock) = setup().await;
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
        let (_pool, store, _lock) = setup().await;
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
    #[ignore]
    async fn test_t1_1_rls_mutations_fail_without_tenant_context() {
        let (admin_pool, admin_store, _lock) = setup().await;
        let iid = Uuid::now_v7();
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
            format!("postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@{}", host_part)
        } else {
            "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@localhost/bpmn_lite_test".to_string()
        };

        let app_pool = PgPool::connect(&app_url)
            .await
            .expect("Failed to connect as bpmn_lite_app");
        let app_store = PostgresProcessStore::new(app_pool.clone());

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
        .bind(b"template-t1-1-32bytes-exactly---")
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
        .bind(b"template-t1-1-32bytes-exactly---")
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
        let row_pi: Option<(Uuid,)> = sqlx::query_as("SELECT instance_id FROM process_instances WHERE instance_id = $1")
            .bind(iid)
            .fetch_optional(&app_pool)
            .await
            .unwrap();
        assert!(row_pi.is_none(), "process_instances query without tenant context must return zero rows");

        let row_jq: Option<(String,)> = sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = 'job-t1-1'")
            .fetch_optional(&app_pool)
            .await
            .unwrap();
        assert!(row_jq.is_none(), "job_queue query without tenant context must return zero rows");

        let row_tmpl: Option<(Vec<u8>,)> = sqlx::query_as("SELECT template_id FROM ffi_template WHERE template_id = $1")
            .bind(b"template-t1-1-32bytes-exactly---")
            .fetch_optional(&app_pool)
            .await
            .unwrap();
        assert!(row_tmpl.is_none(), "ffi_template query without tenant context must return zero rows");

        let row_invoc: Option<(Uuid,)> = sqlx::query_as("SELECT invocation_id FROM ffi_invocation_record WHERE caller_process_instance_id = $1")
            .bind(iid)
            .fetch_optional(&app_pool)
            .await
            .unwrap();
        assert!(row_invoc.is_none(), "ffi_invocation_record query without tenant context must return zero rows");

        let row_inc: Option<(Uuid,)> = sqlx::query_as("SELECT incident_id FROM incidents WHERE process_instance_id = $1")
            .bind(iid)
            .fetch_optional(&app_pool)
            .await
            .unwrap();
        assert!(row_inc.is_none(), "incidents query without tenant context must return zero rows");

        // Verify UPDATE fails closed without context
        let update_res = sqlx::query("UPDATE process_instances SET state = '\"Completed\"'::jsonb WHERE instance_id = $1")
            .bind(iid)
            .execute(&app_pool)
            .await
            .unwrap();
        assert_eq!(update_res.rows_affected(), 0, "Update without tenant context must affect zero rows");

        // 4. Verify cross-tenant blocked / wrong tenant context (queries return zero rows)
        let mut tx = app_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL app.current_tenant = 'evil-tenant'").execute(&mut *tx).await.unwrap();

        let row_pi: Option<(Uuid,)> = sqlx::query_as("SELECT instance_id FROM process_instances WHERE instance_id = $1")
            .bind(iid)
            .fetch_optional(&mut *tx).await.unwrap();
        assert!(row_pi.is_none(), "process_instances query with wrong tenant context must return zero rows");

        let row_jq: Option<(String,)> = sqlx::query_as("SELECT job_key FROM job_queue WHERE job_key = 'job-t1-1'")
            .fetch_optional(&mut *tx).await.unwrap();
        assert!(row_jq.is_none(), "job_queue query with wrong tenant context must return zero rows");

        let row_tmpl: Option<(Vec<u8>,)> = sqlx::query_as("SELECT template_id FROM ffi_template WHERE template_id = $1")
            .bind(b"template-t1-1-32bytes-exactly---")
            .fetch_optional(&mut *tx).await.unwrap();
        assert!(row_tmpl.is_none(), "ffi_template query with wrong tenant context must return zero rows");

        let row_invoc: Option<(Uuid,)> = sqlx::query_as("SELECT invocation_id FROM ffi_invocation_record WHERE caller_process_instance_id = $1")
            .bind(iid)
            .fetch_optional(&mut *tx).await.unwrap();
        assert!(row_invoc.is_none(), "ffi_invocation_record query with wrong tenant context must return zero rows");

        let row_inc: Option<(Uuid,)> = sqlx::query_as("SELECT incident_id FROM incidents WHERE process_instance_id = $1")
            .bind(iid)
            .fetch_optional(&mut *tx).await.unwrap();
        assert!(row_inc.is_none(), "incidents query with wrong tenant context must return zero rows");

        tx.rollback().await.unwrap();

        // 5. Test Site B: quarantine_instance with WRONG tenant context
        // Should return Err because process_instances UPDATE affects 0 rows due to RLS.
        let quar_res = app_store.quarantine_instance(iid, "evil-tenant", "default", "test_t1_1").await;
        assert!(quar_res.is_err(), "quarantine_instance must fail with incorrect tenant context");

        // 5. Test Site A: atomic_consume_buffered_message with WRONG tenant context
        // We first need a claimed message in the buffer.
        let msg_id = format!("msg-t1-1-{}", Uuid::now_v7());
        let message_name = "test-message";
        let correlation_key = "corr-t1-1";
        
        let claim_token = Uuid::now_v7();
        let claim_until_ms = (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp_millis();
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

        let consume_res = app_store.atomic_consume_buffered_message(
            &evil_inst,
            &fiber,
            &claimed_msg,
            None,
            &[],
        ).await;

        assert!(consume_res.is_err(), "atomic_consume_buffered_message must fail (return Err) with incorrect tenant context");
    }

    /// E-invariant I2: Verify distinct cross-tenant read/write isolation under bpmn_lite_app role.
    #[tokio::test]
    #[ignore]
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
        let admin_store = PostgresProcessStore::new(admin_pool.clone());
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

        let call_id_b = Uuid::now_v7();
        let idem_key_b = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO bpmn_pending_invocation (
                callout_id, process_instance_id, node_id, target_domain, verb_id,
                idempotency_key, execution_id, tenant_id
            )
            VALUES ($1, $2, 'node-1', 'domain', 'verb', $3, NULL, $4)
            "#
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
            format!("postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@{}", host_part)
        } else {
            "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@localhost/bpmn_lite_test".to_string()
        };

        let app_pool = PgPool::connect(&app_url)
            .await
            .expect("Failed to connect as bpmn_lite_app");

        // 3. Non-vacuity check: admin/no-context view sees B's rows
        let admin_count: (i64,) = sqlx::query_as("SELECT count(*) FROM process_instances WHERE instance_id IN ($1, $2)")
            .bind(iid_a)
            .bind(iid_b)
            .fetch_one(&admin_pool)
            .await
            .unwrap();
        assert_eq!(admin_count.0, 2, "Admin connection must see both tenant rows (non-vacuous)");

        // 4. Read isolation: with app.current_tenant = tenant_a, query A only sees A
        let mut tx = app_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL app.current_tenant = 'tenant-A'").execute(&mut *tx).await.unwrap();

        let visible_rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT instance_id, tenant_id FROM process_instances WHERE instance_id IN ($1, $2)")
            .bind(iid_a)
            .bind(iid_b)
            .fetch_all(&mut *tx)
            .await
            .unwrap();

        assert_eq!(visible_rows.len(), 1, "Tenant A context must only see 1 row");
        assert_eq!(visible_rows[0].0, iid_a, "Visible row must belong to tenant A");
        assert_eq!(visible_rows[0].1, "tenant-A", "Visible row must have tenant-A ID");

        // Assert Tenant A cannot read Tenant B's child rows
        let visible_fibers: Vec<(Uuid,)> = sqlx::query_as("SELECT fiber_id FROM fibers WHERE instance_id = $1")
            .bind(iid_b)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert!(visible_fibers.is_empty(), "Tenant A context must not see Tenant B fibers");

        let visible_calls: Vec<(Uuid,)> = sqlx::query_as("SELECT callout_id FROM bpmn_pending_invocation WHERE process_instance_id = $1")
            .bind(iid_b)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert!(visible_calls.is_empty(), "Tenant A context must not see Tenant B pending invocations");

        let visible_msgs: Vec<(String,)> = sqlx::query_as("SELECT msg_id FROM message_buffer WHERE msg_id = $1")
            .bind(&msg_id_b)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert!(visible_msgs.is_empty(), "Tenant A context must not see Tenant B message buffer rows");

        // 5. Write isolation: with app.current_tenant = tenant_a, update/delete B affects 0 rows
        let update_res = sqlx::query("UPDATE process_instances SET state = '\"Completed\"'::jsonb WHERE instance_id = $1")
            .bind(iid_b)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(update_res.rows_affected(), 0, "Update on Tenant B row under Tenant A context must affect 0 rows");

        let delete_res = sqlx::query("DELETE FROM process_instances WHERE instance_id = $1")
            .bind(iid_b)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(delete_res.rows_affected(), 0, "Delete on Tenant B row under Tenant A context must affect 0 rows");

        let update_msg_res = sqlx::query("UPDATE message_buffer SET status = 'consumed' WHERE msg_id = $1")
            .bind(&msg_id_b)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(update_msg_res.rows_affected(), 0, "Update on Tenant B message_buffer row under Tenant A context must affect 0 rows");

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
        assert!(write_fib_res.is_err(), "Write to fibers with tenant-B ID under tenant-A context must fail WITH CHECK");

        let write_call_res = sqlx::query(
            r#"
            INSERT INTO bpmn_pending_invocation (
                callout_id, process_instance_id, node_id, target_domain, verb_id,
                idempotency_key, execution_id, tenant_id
            )
            VALUES ($1, $2, 'node-1', 'domain', 'verb', $3, NULL, $4)
            "#
        )
        .bind(Uuid::now_v7())
        .bind(iid_b)
        .bind(Uuid::now_v7())
        .bind(tenant_b)
        .execute(&mut *tx)
        .await;
        assert!(write_call_res.is_err(), "Write to bpmn_pending_invocation with tenant-B ID under tenant-A context must fail WITH CHECK");

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
        assert!(write_msg_res.is_err(), "Write to message_buffer with tenant-B ID under tenant-A context must fail WITH CHECK");

        tx.rollback().await.unwrap();
    }

    /// RISK-009: Lease fencing re-enabled. A worker with the wrong lease owner is rejected.
    #[tokio::test]
    #[ignore]
    async fn test_risk_009_lease_fence_rejection() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // 1. Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("owner-a", &inst).await.unwrap();

        // 2. Claim it under owner-a
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "owner-a", 30000).await.unwrap();
        assert!(claimed);

        // 3. Force lease expiry in DB
        sqlx::query("UPDATE process_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // 4. Owner-b claims the expired lease
        let claimed_b = store.claim_instance_for_transition(tenant_id, iid, "owner-b", 30000).await.unwrap();
        assert!(claimed_b);

        // 5. Stale owner-a tries to write -> must be rejected (returns error because rows_affected == 0 in update_instance_state)
        let res_stale = store.update_instance_state(tenant_id, "owner-a", iid, ProcessState::Completed { at: 222 }).await;
        assert!(res_stale.is_err(), "Stale owner-a write must fail the fence");

        // 6. Current owner-b tries to write -> succeeds
        let res_current = store.update_instance_state(tenant_id, "owner-b", iid, ProcessState::Completed { at: 333 }).await;
        assert!(res_current.is_ok(), "Current owner-b write must succeed");
    }

    /// Regression healed correctly: bus_runtime::advance and detect_interrupted_ffi_calls write successfully by claiming the lease.
    #[tokio::test]
    #[ignore]
    async fn test_regression_healed_by_claim() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // 1. Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("scheduler", &inst).await.unwrap();
        
        // Simulating park-release:
        sqlx::query("UPDATE process_instances SET lease_owner = NULL, lease_until = NULL WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // 2. A resumer (e.g. bus callback or recovery) claims the lease and writes
        let resumer_owner = "bus-resumer-x";
        let claimed = store.claim_instance_for_transition(tenant_id, iid, resumer_owner, 30000).await.unwrap();
        assert!(claimed, "Resumer must claim the released lease");

        // 3. Write state-advancing change under the claimed owner
        let res_write = store.update_instance_state(tenant_id, resumer_owner, iid, ProcessState::Completed { at: 999 }).await;
        assert!(res_write.is_ok(), "Resumer write under held lease must succeed");

        // 4. Assert that the write occurred under the claimed owner in the DB
        let loaded = store.load_instance(iid).await.unwrap().unwrap();
        assert!(matches!(loaded.state, ProcessState::Completed { at: 999 }));
    }

    /// E-invariant: After a tick parks an instance, a different worker can claim the lease successfully (lease was released).
    #[tokio::test]
    #[ignore]
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
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "owner-a", 30000).await.unwrap();
        assert!(claimed);

        // 3. Commit a tick that has no running fibers (parks the instance)
        // Since no fibers exist in the fibers table for this instance, has_running_fiber is false.
        store.commit_tick(iid, tenant_id, "owner-a", &[]).await.unwrap();

        // 4. Verify lease is released (lease_owner is NULL)
        let row: (Option<String>,) = sqlx::query_as("SELECT lease_owner FROM process_instances WHERE instance_id = $1")
            .bind(iid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(row.0.is_none(), "Lease owner must be cleared after park");

        // 5. A different worker (owner-b) can claim it successfully
        let claimed_b = store.claim_instance_for_transition(tenant_id, iid, "owner-b", 30000).await.unwrap();
        assert!(claimed_b, "Different worker must be able to claim the released lease");
    }

    /// C2: Postgres single-transaction atomicity test (the decisive proof)
    #[tokio::test]
    #[ignore]
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
        inst.domain_payload = "initial_payload".to_string().into();
        inst.domain_payload_hash = [1u8; 32];
        inst.state = ProcessState::Running;
        
        store.save_instance("default", &inst).await.unwrap();
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "default", 30000).await.unwrap();
        assert!(claimed);
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
            TickOperation::SaveFiber { fiber: child1.clone() },
            TickOperation::SaveFiber { fiber: child2.clone() },
            TickOperation::JoinArrive { join_id: 100 },
            TickOperation::DeleteFiber { fiber_id: parent_fiber_id },
            TickOperation::ConsumeBufferedMessage { message: msg }, // FAIL!
        ];

        // 3. Run the engine's atomic apply; assert it returns Err
        let res = store.commit_tick(iid, tenant_id, "default", &ops).await;
        assert!(res.is_err(), "Expected transaction to fail and roll back");

        // 4. Query the Postgres database directly and assert NONE of the ops persisted:
        // - parent fiber intact
        // - zero child fibers
        // - join count still 0
        // - no new event rows
        // - instance state/payload unchanged
        let fibers = store.load_fibers(iid).await.unwrap();
        assert_eq!(fibers.len(), 1, "Rollback failed: expected exactly 1 fiber (parent)");
        assert_eq!(fibers[0].fiber_id, parent_fiber_id, "Rollback failed: parent fiber not intact");

        let join_count = store.join_get(iid, 100).await.unwrap();
        assert_eq!(join_count, 0, "Rollback failed: join count updated");

        let events = store.read_events(iid, 0).await.unwrap();
        assert_eq!(events.len(), 0, "Rollback failed: events appended");

        let loaded_inst = store.load_instance(iid).await.unwrap().unwrap();
        assert_eq!(loaded_inst.domain_payload.as_ref(), "initial_payload");

        // 5. Re-run the tick without the failing op; assert it commits and all ops persist correctly
        let successful_ops = vec![
            TickOperation::SaveFiber { fiber: child1.clone() },
            TickOperation::SaveFiber { fiber: child2.clone() },
            TickOperation::JoinArrive { join_id: 100 },
            TickOperation::DeleteFiber { fiber_id: parent_fiber_id },
            TickOperation::UpdateInstanceState { state: ProcessState::Completed { at: 123456 } },
        ];
        let res2 = store.commit_tick(iid, tenant_id, "default", &successful_ops).await;
        assert!(res2.is_ok(), "Expected transaction to succeed: {:?}", res2);

        let fibers = store.load_fibers(iid).await.unwrap();
        assert_eq!(fibers.len(), 2, "Expected exactly 2 child fibers");
        assert!(fibers.iter().any(|f| f.fiber_id == child1.fiber_id));
        assert!(fibers.iter().any(|f| f.fiber_id == child2.fiber_id));
        assert!(!fibers.iter().any(|f| f.fiber_id == parent_fiber_id));

        let join_count = store.join_get(iid, 100).await.unwrap();
        assert_eq!(join_count, 1);

        let loaded_inst = store.load_instance(iid).await.unwrap().unwrap();
        assert!(matches!(loaded_inst.state, ProcessState::Completed { at: 123456 }));
    }

    /// E-invariant: Emit atomicity (RISK-003). Atomically inserts outbox, pending, and saves instance, rolling back completely on failure.
    #[tokio::test]
    #[ignore]
    async fn test_risk_003_emit_atomicity() {
        let (pool, store, _lock) = setup().await;
        let iid = Uuid::now_v7();
        let tenant_id = "default";

        // Seed instance
        let mut inst = make_instance(iid);
        inst.tenant_id = tenant_id.to_string();
        store.save_instance("default", &inst).await.unwrap();
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "default", 30000).await.unwrap();
        assert!(claimed);

        // Build a pending record
        let callout_id = Uuid::now_v7();
        let pending = PendingInvocation::new(
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
            TickOperation::InsertPendingInvocation { pending: pending.clone() },
            TickOperation::InsertOutbox {
                id: Uuid::now_v7(),
                target_domain: "domain-x".to_string(),
                target_endpoint: "invocation".to_string(),
                payload: vec![1, 2, 3],
                idempotency_key: pending.idempotency_key,
                callout_id,
            },
            TickOperation::SaveInstance { instance: inst.clone() },
            TickOperation::ConsumeBufferedMessage { message: fail_msg }, // FAIL!
        ];

        let commit_res = store.commit_tick(iid, tenant_id, "default", &ops).await;
        assert!(commit_res.is_err(), "Expected emit commit to fail and roll back");

        // Verify no pending row, no outbox row, and instance not advanced
        let pending_row: (i64,) = sqlx::query_as("SELECT count(*) FROM bpmn_pending_invocation WHERE callout_id = $1")
            .bind(callout_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending_row.0, 0, "Rollback failed: pending row found");

        let outbox_row: (i64,) = sqlx::query_as("SELECT count(*) FROM dsl_bus.outbox WHERE callout_id = $1")
            .bind(callout_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(outbox_row.0, 0, "Rollback failed: outbox row found");

        // Now run the successful commit
        let successful_ops = vec![
            TickOperation::InsertPendingInvocation { pending: pending.clone() },
            TickOperation::InsertOutbox {
                id: Uuid::now_v7(),
                target_domain: "domain-x".to_string(),
                target_endpoint: "invocation".to_string(),
                payload: vec![1, 2, 3],
                idempotency_key: pending.idempotency_key,
                callout_id,
            },
        ];
        store.commit_tick(iid, tenant_id, "default", &successful_ops).await.unwrap();

        // Verify both rows now exist
        let pending_row2: (i64,) = sqlx::query_as("SELECT count(*) FROM bpmn_pending_invocation WHERE callout_id = $1")
            .bind(callout_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending_row2.0, 1, "Pending row not found after successful commit");

        let outbox_row2: (i64,) = sqlx::query_as("SELECT count(*) FROM dsl_bus.outbox WHERE callout_id = $1")
            .bind(callout_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(outbox_row2.0, 1, "Outbox row not found after successful commit");
    }

    /// E-invariant: Duplicate-result idempotency (RISK-004). Delivering the same result twice results in a no-op on the second run.
    #[tokio::test]
    #[ignore]
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
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "owner-first", 30000).await.unwrap();
        assert!(claimed);

        // First delivery: commit take pending + state change
        inst.state = ProcessState::Running; // advance state
        let ops = vec![
            TickOperation::TakePendingInvocation { execution_id },
            TickOperation::SaveInstance { instance: inst.clone() },
        ];
        let first_res = store.commit_tick(iid, tenant_id, "owner-first", &ops).await;
        assert!(first_res.is_ok(), "First delivery must succeed");

        // Release first lease manually (as we didn't park)
        store.release_instance_transition(tenant_id, iid, "owner-first").await.unwrap();

        // Second delivery (re-delivery of same execution_id):
        // Claim transition lease for the second delivery
        let claimed_second = store.claim_instance_for_transition(tenant_id, iid, "owner-second", 30000).await.unwrap();
        assert!(claimed_second);

        // Try to commit the exact same ops (re-delivery)
        let second_ops = vec![
            TickOperation::TakePendingInvocation { execution_id },
            TickOperation::SaveInstance { instance: inst.clone() },
        ];
        let second_res = store.commit_tick(iid, tenant_id, "owner-second", &second_ops).await;
        assert!(second_res.is_err(), "Second delivery must fail because row is already gone");
        assert!(second_res.unwrap_err().to_string().contains("already consumed"));
    }

    /// E-invariant F2 negative test: a non-dedup commit_tick failure on the advance path (e.g. lease fence failure)
    /// must not be swallowed as AlreadyConsumedError. The error must propagate and the pending row must NOT be deleted.
    #[tokio::test]
    #[ignore]
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
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "actual-owner", 30000).await.unwrap();
        assert!(claimed);

        // Attempt commit_tick under a different owner "wrong-owner" -> should fail due to lease fence rejection!
        let ops = vec![
            TickOperation::TakePendingInvocation { execution_id },
            TickOperation::SaveInstance { instance: inst.clone() },
        ];
        let res = store.commit_tick(iid, tenant_id, "wrong-owner", &ops).await;
        assert!(res.is_err(), "Expected lease fence error to propagate");
        
        let err = res.unwrap_err();
        // Assert it is NOT AlreadyConsumedError
        assert!(!err.is::<bpmn_lite_store::store::AlreadyConsumedError>(), "Lease fence error must not be masked as AlreadyConsumedError");
        
        // Assert that the pending row was NOT deleted (since the transaction rolled back)
        let row_count: (i64,) = sqlx::query_as("SELECT count(*) FROM bpmn_pending_invocation WHERE execution_id = $1")
            .bind(execution_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row_count.0, 1, "Pending row must still exist since transaction rolled back");
    }

    /// E-invariant: Concurrent Claim races and Concurrent Recovery (T3.3.1)
    #[tokio::test]
    #[ignore]
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
        let claimed = store.claim_instance_for_transition(tenant_id, iid, "owner-temp", 30000).await.unwrap();
        assert!(claimed);
        sqlx::query("UPDATE process_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // Race two claimers concurrently
        let store_arc = Arc::new(store);
        let s1 = store_arc.clone();
        let s2 = store_arc.clone();

        let t1 = tokio::spawn(async move {
            s1.claim_instance_for_transition("default", iid, "claimer-1", 30000).await
        });
        let t2 = tokio::spawn(async move {
            s2.claim_instance_for_transition("default", iid, "claimer-2", 30000).await
        });

        let r1 = t1.await.unwrap().unwrap();
        let r2 = t2.await.unwrap().unwrap();

        // Assert exactly one won, and the other returned false (loser no-ops gracefully)
        assert!(r1 != r2, "Exactly one claimer must succeed");
        assert!(r1 || r2, "At least one claimer must succeed");

        // Now test concurrent recovery:
        // Set the instance to Failed (simulating crash)
        let mut inst = store_arc.load_instance(iid).await.unwrap().unwrap();
        inst.state = ProcessState::Failed { incident_id: Uuid::now_v7() };
        // Save using whichever won the lease
        let active_owner = if r1 { "claimer-1" } else { "claimer-2" };
        store_arc.save_instance(active_owner, &inst).await.unwrap();

        // Expire lease again so it's reclaimable by recovery
        sqlx::query("UPDATE process_instances SET lease_until = now() - interval '1 second' WHERE instance_id = $1")
            .bind(iid)
            .execute(&pool)
            .await
            .unwrap();

        // Simulating two recovery processes racing
        let s1_rec = store_arc.clone();
        let s2_rec = store_arc.clone();

        let rec1 = tokio::spawn(async move {
            let owner = "recovery-1";
            let claimed = s1_rec.claim_instance_for_transition("default", iid, owner, 30000).await.unwrap();
            if claimed {
                // Recover the instance
                let mut inst = s1_rec.load_instance(iid).await.unwrap().unwrap();
                inst.state = ProcessState::Running;
                s1_rec.save_instance(owner, &inst).await.unwrap();
                s1_rec.release_instance_transition("default", iid, owner).await.unwrap();
                true
            } else {
                false
            }
        });

        let rec2 = tokio::spawn(async move {
            let owner = "recovery-2";
            let claimed = s2_rec.claim_instance_for_transition("default", iid, owner, 30000).await.unwrap();
            if claimed {
                // Recover the instance
                let mut inst = s2_rec.load_instance(iid).await.unwrap().unwrap();
                inst.state = ProcessState::Running;
                s2_rec.save_instance(owner, &inst).await.unwrap();
                s2_rec.release_instance_transition("default", iid, owner).await.unwrap();
                true
            } else {
                false
            }
        });

        let res_rec1 = rec1.await.unwrap();
        let res_rec2 = rec2.await.unwrap();

        assert!(res_rec1 != res_rec2, "Exactly one recovery runner must claim the instance");
    }

    struct ViolatingTestStore {
        inner: Arc<PostgresProcessStore>,
        violate_instance_id: Uuid,
        should_fail_load_integrity: std::sync::atomic::AtomicBool,
        should_fail_commit_integrity: std::sync::atomic::AtomicBool,
        should_fail_generic: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl ProcessStore for ViolatingTestStore {
        async fn save_instance(&self, lease_owner: &str, instance: &ProcessInstance) -> Result<()> {
            self.inner.save_instance(lease_owner, instance).await
        }
        async fn load_instance(&self, id: Uuid) -> Result<Option<ProcessInstance>> {
            if id == self.violate_instance_id {
                if self.should_fail_load_integrity.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(anyhow!(bpmn_lite_types::integrity::IntegrityViolation {
                        instance_id: id,
                        tenant_id: "default".to_string(),
                        detection_point: "test_violation_load".to_string(),
                    }));
                }
                if self.should_fail_generic.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(anyhow!("generic load failure"));
                }
            }
            self.inner.load_instance(id).await
        }
        async fn update_instance_state(&self, tenant_id: &str, lease_owner: &str, id: Uuid, state: ProcessState) -> Result<()> {
            self.inner.update_instance_state(tenant_id, lease_owner, id, state).await
        }
        async fn update_instance_flags(&self, tenant_id: &str, lease_owner: &str, id: Uuid, flags: &BTreeMap<FlagKey, Value>) -> Result<()> {
            self.inner.update_instance_flags(tenant_id, lease_owner, id, flags).await
        }
        async fn update_instance_payload(&self, tenant_id: &str, lease_owner: &str, id: Uuid, payload: &str, hash: &[u8; 32]) -> Result<()> {
            self.inner.update_instance_payload(tenant_id, lease_owner, id, payload, hash).await
        }
        async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()> {
            self.inner.save_fiber(instance_id, fiber).await
        }
        async fn load_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<Option<Fiber>> {
            self.inner.load_fiber(instance_id, fiber_id).await
        }
        async fn load_fibers(&self, instance_id: Uuid) -> Result<Vec<Fiber>> {
            self.inner.load_fibers(instance_id).await
        }
        async fn delete_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<()> {
            self.inner.delete_fiber(instance_id, fiber_id).await
        }
        async fn delete_all_fibers(&self, instance_id: Uuid) -> Result<()> {
            self.inner.delete_all_fibers(instance_id).await
        }
        async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
            self.inner.join_arrive(instance_id, join_id).await
        }
        async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()> {
            self.inner.join_reset(instance_id, join_id).await
        }
        async fn join_delete_all(&self, instance_id: Uuid) -> Result<()> {
            self.inner.join_delete_all(instance_id).await
        }
        async fn dedupe_get(&self, key: &str) -> Result<Option<JobCompletion>> {
            self.inner.dedupe_get(key).await
        }
        async fn dedupe_put(&self, key: &str, completion: &JobCompletion) -> Result<()> {
            self.inner.dedupe_put(key, completion).await
        }
        async fn record_message_delivery(&self, tenant_id: &str, instance_id: Uuid, msg_id: &str) -> Result<bool> {
            self.inner.record_message_delivery(tenant_id, instance_id, msg_id).await
        }
        async fn enqueue_job(&self, activation: &JobActivation) -> Result<()> {
            self.inner.enqueue_job(activation).await
        }
        async fn dequeue_jobs(&self, task_types: &[String], max: usize, tenant_id: &str, worker_id: &str, lease_ms: u64) -> Result<Vec<JobActivation>> {
            self.inner.dequeue_jobs(task_types, max, tenant_id, worker_id, lease_ms).await
        }
        async fn ack_job(&self, tenant_id: &str, job_key: &str) -> Result<()> {
            self.inner.ack_job(tenant_id, job_key).await
        }
        async fn validate_job_claim(&self, tenant_id: &str, job_key: &str, worker_id: &str, claim_token: &str) -> Result<bool> {
            self.inner.validate_job_claim(tenant_id, job_key, worker_id, claim_token).await
        }
        async fn retry_claimed_job(&self, tenant_id: &str, job_key: &str, worker_id: &str, claim_token: &str, error_class: &str, error_message: &str, not_before_ms: i64) -> Result<bool> {
            self.inner.retry_claimed_job(tenant_id, job_key, worker_id, claim_token, error_class, error_message, not_before_ms).await
        }
        async fn dead_letter_claimed_job(&self, tenant_id: &str, job_key: &str, worker_id: &str, claim_token: &str, error_class: &str, error_message: &str, incident_id: Uuid) -> Result<bool> {
            self.inner.dead_letter_claimed_job(tenant_id, job_key, worker_id, claim_token, error_class, error_message, incident_id).await
        }
        async fn cancel_jobs_for_instance(&self, instance_id: Uuid) -> Result<Vec<String>> {
            self.inner.cancel_jobs_for_instance(instance_id).await
        }
        async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> Result<()> {
            self.inner.store_program(version, program).await
        }
        async fn load_program(&self, version: [u8; 32]) -> Result<Option<CompiledProgram>> {
            self.inner.load_program(version).await
        }
        async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> Result<()> {
            self.inner.store_plan(plan_hash, plan_json).await
        }
        async fn load_plan(&self, plan_hash: [u8; 32]) -> Result<Option<String>> {
            self.inner.load_plan(plan_hash).await
        }
        async fn dead_letter_put(&self, name: u32, corr_key: &Value, payload: &[u8], ttl_ms: u64) -> Result<()> {
            self.inner.dead_letter_put(name, corr_key, payload, ttl_ms).await
        }
        async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> Result<Option<Vec<u8>>> {
            self.inner.dead_letter_take(name, corr_key).await
        }
        async fn buffer_message(&self, tenant_id: &str, message_name: &str, correlation_key: &str, msg_id: &str, payload: &[u8], payload_hash: Option<[u8; 32]>, ttl_ms: u64, process_instance_id: Option<Uuid>) -> Result<BufferMessageResult> {
            self.inner.buffer_message(tenant_id, message_name, correlation_key, msg_id, payload, payload_hash, ttl_ms, process_instance_id).await
        }
        async fn claim_buffered_message(&self, tenant_id: &str, message_name: &str, correlation_key: &str, claim_ms: u64) -> Result<Option<ClaimedBufferedMessage>> {
            self.inner.claim_buffered_message(tenant_id, message_name, correlation_key, claim_ms).await
        }
        async fn atomic_consume_buffered_message(&self, instance: &ProcessInstance, fiber: &Fiber, message: &ClaimedBufferedMessage, payload_update: Option<&PayloadUpdate>, events: &[RuntimeEvent]) -> Result<bool> {
            self.inner.atomic_consume_buffered_message(instance, fiber, message, payload_update, events).await
        }
        async fn release_buffered_message_claim(&self, message: &ClaimedBufferedMessage) -> Result<bool> {
            self.inner.release_buffered_message_claim(message).await
        }
        async fn reclaim_stale_buffered_message_claims(&self) -> Result<u32> {
            self.inner.reclaim_stale_buffered_message_claims().await
        }
        async fn prune_expired_messages(&self) -> Result<u32> {
            self.inner.prune_expired_messages().await
        }
        async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64> {
            self.inner.append_event(instance_id, event).await
        }
        async fn read_events(&self, instance_id: Uuid, from_seq: u64) -> Result<Vec<(u64, RuntimeEvent)>> {
            self.inner.read_events(instance_id, from_seq).await
        }
        async fn save_payload_version(&self, instance_id: Uuid, hash: &[u8; 32], payload: &str) -> Result<()> {
            self.inner.save_payload_version(instance_id, hash, payload).await
        }
        async fn load_payload_version(&self, instance_id: Uuid, hash: &[u8; 32]) -> Result<Option<String>> {
            self.inner.load_payload_version(instance_id, hash).await
        }
        async fn save_incident(&self, incident: &Incident) -> Result<()> {
            self.inner.save_incident(incident).await
        }
        async fn load_incidents(&self, instance_id: Uuid) -> Result<Vec<Incident>> {
            self.inner.load_incidents(instance_id).await
        }
        async fn atomic_start(&self, tenant_id: &str, lease_owner: &str, instance: &ProcessInstance, root_fiber: &Fiber, event: &RuntimeEvent) -> Result<u64> {
            self.inner.atomic_start(tenant_id, lease_owner, instance, root_fiber, event).await
        }
        async fn atomic_complete(&self, tenant_id: &str, lease_owner: &str, instance: &ProcessInstance, completion: &JobCompletion, events: &[RuntimeEvent]) -> Result<()> {
            self.inner.atomic_complete(tenant_id, lease_owner, instance, completion, events).await
        }
        async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> Result<u32> {
            self.inner.reclaim_stale_jobs(timeout_ms).await
        }
        async fn prune_dedupe_cache(&self, older_than_ms: u64) -> Result<u32> {
            self.inner.prune_dedupe_cache(older_than_ms).await
        }
        async fn list_running_instances(&self, tenant_id: &str) -> Result<Vec<Uuid>> {
            self.inner.list_running_instances(tenant_id).await
        }
        async fn claim_running_instances(&self, tenant_id: &str, owner: &str, limit: usize, lease_ms: u64) -> Result<Vec<Uuid>> {
            self.inner.claim_running_instances(tenant_id, owner, limit, lease_ms).await
        }
        async fn claim_instance_for_transition(&self, tenant_id: &str, instance_id: Uuid, owner: &str, lease_ms: u64) -> Result<bool> {
            self.inner.claim_instance_for_transition(tenant_id, instance_id, owner, lease_ms).await
        }
        async fn release_instance_transition(&self, tenant_id: &str, instance_id: Uuid, owner: &str) -> Result<()> {
            self.inner.release_instance_transition(tenant_id, instance_id, owner).await
        }
        async fn health_check(&self) -> Result<()> {
            self.inner.health_check().await
        }
        async fn ensure_tenant(&self, tenant_id: &str) -> Result<()> {
            self.inner.ensure_tenant(tenant_id).await
        }
        async fn list_tenants(&self) -> Result<Vec<String>> {
            self.inner.list_tenants().await
        }
        async fn list_tenants_in_pool(&self, pool_id: &str) -> Result<Vec<String>> {
            self.inner.list_tenants_in_pool(pool_id).await
        }
        async fn quarantine_instance(&self, instance_id: Uuid, tenant_id: &str, lease_owner: &str, detection_point: &str) -> Result<()> {
            self.inner.quarantine_instance(instance_id, tenant_id, lease_owner, detection_point).await
        }
        async fn join_get(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
            self.inner.join_get(instance_id, join_id).await
        }
        async fn commit_tick(&self, instance_id: Uuid, tenant_id: &str, lease_owner: &str, ops: &[TickOperation]) -> Result<()> {
            if instance_id == self.violate_instance_id && self.should_fail_commit_integrity.load(std::sync::atomic::Ordering::Relaxed) {
                let lease_owner_owned = lease_owner.to_string();
                let tenant_id_owned = tenant_id.to_string();
                let tenant_id_for_closure = tenant_id.to_string();
                let ops = ops.to_vec();
                return self.inner.execute_tenant_scoped(&tenant_id_owned, &lease_owner_owned, |tx| Box::pin(async move {
                    for op in &ops {
                        PostgresProcessStore::apply_op(tx, instance_id, op).await?;
                    }

                    // Post-ops check in the same transaction: release lease if parked/terminal
                    let state_val: serde_json::Value = sqlx::query_scalar(
                        "SELECT state FROM process_instances WHERE instance_id = $1"
                    )
                    .bind(instance_id)
                    .fetch_one(&mut *tx.tx)
                    .await?;

                    let parsed_state: ProcessState = serde_json::from_value(state_val)?;

                    let has_running_fiber: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM fibers WHERE instance_id = $1 AND wait_state = '\"Running\"'::jsonb)"
                    )
                    .bind(instance_id)
                    .fetch_one(&mut *tx.tx)
                    .await?;

                    let is_runnable = matches!(parsed_state, ProcessState::Running) && has_running_fiber;

                    if !is_runnable {
                        sqlx::query(
                            r#"
                            UPDATE process_instances
                            SET lease_owner = NULL,
                                lease_until = now() - interval '1 second'
                            WHERE instance_id = $1
                            "#
                        )
                        .bind(instance_id)
                        .execute(&mut *tx.tx)
                        .await?;
                    }

                    Err(anyhow!(bpmn_lite_types::integrity::IntegrityViolation {
                        instance_id,
                        tenant_id: tenant_id_for_closure,
                        detection_point: "test_violation_commit".to_string(),
                    }))
                })).await;
            }
            self.inner.commit_tick(instance_id, tenant_id, lease_owner, ops).await
        }
    }

    /// E-invariant #1 & #2: Violation -> quarantine, not crash, not churn. Quarantine survives rollback.
    /// Drives a tick through the production path, rolls it back, and verifies state changes do not persist while quarantine does.
    #[tokio::test]
    #[ignore]
    async fn test_pg_integrity_violation_quarantines_and_rolls_back() {
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
        let loaded_before = store.load_instance(iid).await.unwrap().unwrap();
        assert_eq!(loaded_before.quarantine_state, None);
        let fibers_before = store.load_fibers(iid).await.unwrap();
        assert_eq!(fibers_before[0].pc, 0);

        // 2. Wrap the store in ViolatingTestStore, violating on commit_tick.
        let violating_store = Arc::new(ViolatingTestStore {
            inner: store.clone(),
            violate_instance_id: iid,
            should_fail_load_integrity: std::sync::atomic::AtomicBool::new(false),
            should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(true),
            should_fail_generic: std::sync::atomic::AtomicBool::new(false),
        });
        let engine_with_violating_store = BpmnLiteEngine::new(violating_store.clone());

        // 3. Drive the tick. The engine will run the fiber, try to commit_tick,
        // which fails with IntegrityViolation, rolls back, and then quarantines.
        let tick_res = engine_with_violating_store.tick_instance(iid).await;
        assert!(tick_res.is_ok(), "Tick must return Ok(()) (graceful skip) on IntegrityViolation, got {:?}", tick_res);

        // 4. Assert both halves post-rollback:
        // A. The state change (instance's current_node_id and job queue enqueueing) did NOT persist (rolled back).
        let loaded_after = store.load_instance(iid).await.unwrap().unwrap();
        assert_eq!(loaded_after.current_node_id.as_deref(), None, "Instance current_node_id must be rolled back (still None)");

        // Job queue must not contain any jobs for our instance
        let jobs = store.dequeue_jobs(&["do_work".to_string()], 100, "default", "test-worker", 5000).await.unwrap();
        let has_job_for_our_instance = jobs.iter().any(|j| j.process_instance_id == iid);
        assert!(!has_job_for_our_instance, "Job for our instance must not have been enqueued (rolled back)");

        // B. The quarantine_state DID persist (due to separate connection write).
        assert_eq!(loaded_after.quarantine_state.as_deref(), Some("integrity_violation"), "quarantine_state must survive rollback");

        // C. Quarantined instance is excluded from claims.
        let claimed = store.claim_running_instances("default", "test-scheduler", 10, 5000).await.unwrap();
        assert!(!claimed.contains(&iid), "quarantined instance must not be returned by claim_running_instances");
    }

    /// E-invariant #3: Discrimination / no over-quarantine.
    #[tokio::test]
    #[ignore]
    async fn test_pg_non_integrity_failure_does_not_quarantine() {
        let (_pool, store, _lock) = setup().await;
        let store = Arc::new(store);
        let iid = Uuid::now_v7();

        store.ensure_tenant("default").await.unwrap();
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
        assert!(tick_res.is_err(), "Tick must propagate non-integrity errors, got {:?}", tick_res);

        let loaded = store.load_instance(iid).await.unwrap().unwrap();
        assert_eq!(loaded.quarantine_state, None, "Ordinary failure must not quarantine instance");

        let claimed = store.claim_running_instances("default", "test-scheduler", 10, 5000).await.unwrap();
        assert!(claimed.contains(&iid), "Ordinary failed instance must remain claimable/retryable");
    }

    /// Recovery path: a corrupt instance encountered during detect_interrupted_ffi_calls
    /// is quarantined (under the recovery owner), does not abort the scan, and recovery
    /// continues to other instances.
    #[tokio::test]
    #[ignore]
    async fn test_pg_integrity_violation_in_startup_recovery() {
        let (pool, store, _lock) = setup().await;
        let store = Arc::new(store);

        let iid_corrupt = Uuid::now_v7();
        let iid_healthy = Uuid::now_v7();

        store.ensure_tenant("default").await.unwrap();

        // 1. Publish FFI template as NonIdempotent
        let template_id = [1u8; 32];
        let template_id_hex = hex(&template_id);
        use ffi_catalogue::FfiTemplateStore;
        let ffi_store = Arc::new(crate::ffi_template_store::PostgresFfiTemplateStore::new(pool.clone()));
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
        catalogue.load_into_cache("default").await.unwrap();
        let dispatcher = Arc::new(ffi_dispatcher::FfiDispatcher::new(catalogue));

        // 2. Save both instances
        let inst_corrupt = make_instance(iid_corrupt);
        let inst_healthy = make_instance(iid_healthy);
        store.save_instance("test-owner", &inst_corrupt).await.unwrap();
        store.save_instance("test-owner", &inst_healthy).await.unwrap();

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
            caller_pc: 0,
            owner_type: "engine".to_string(),
        };
        let ev_healthy = RuntimeEvent::FfiInvocationPending {
            invocation_id: Uuid::now_v7(),
            template_id_hex: template_id_hex.clone(),
            caller_task_id: "task1".to_string(),
            caller_pc: 0,
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

        // 4. Run detect_interrupted_ffi_calls. It should recover 1 (healthy), quarantine corrupt, and complete.
        let recovered = engine.detect_interrupted_ffi_calls("default").await.unwrap();
        assert_eq!(recovered, 2, "Both pending invocations should be scanned");

        // 5. Assert:
        // A. iid_corrupt is quarantined
        let corrupt_loaded = store.load_instance(iid_corrupt).await.unwrap().unwrap();
        assert_eq!(corrupt_loaded.quarantine_state.as_deref(), Some("integrity_violation"));

        // B. iid_healthy was successfully recovered (its state is Failed due to incident creation)
        let healthy_loaded = store.load_instance(iid_healthy).await.unwrap().unwrap();
        assert!(matches!(healthy_loaded.state, ProcessState::Failed { .. }), "Healthy instance must be Failed, got {:?}", healthy_loaded.state);
        assert_eq!(healthy_loaded.quarantine_state, None, "Healthy instance must not be quarantined");
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

