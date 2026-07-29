//! Typed views over the per-domain outbox / inbox rows.

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

/// Crate-level result alias.
pub type Result<T> = std::result::Result<T, BusStorageError>;

#[derive(Debug, Error)]
pub enum BusStorageError {
    #[error("bus storage query failed: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("invalid bus column '{column}' value '{value}'")]
    InvalidColumn { column: &'static str, value: String },

    #[error("outbox dispatch lease was lost for row {0}")]
    LostClaim(Uuid),
}

/// Outbox / inbox endpoint discriminator. Both tables store these as
/// `TEXT` columns with a `CHECK` constraint so unknown values cannot
/// reach the typed layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusEndpoint {
    /// `InvocationService.Submit` payload.
    Invocation,
    /// `ResultService.DeliverResult` payload.
    Result,
}

impl BusEndpoint {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invocation => "invocation",
            Self::Result => "result",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s {
            "invocation" => Ok(Self::Invocation),
            "result" => Ok(Self::Result),
            other => Err(BusStorageError::InvalidColumn {
                column: "target_endpoint",
                value: other.to_owned(),
            }),
        }
    }
}

/// Outbox row state machine: `pending → submitted` (success) or
/// `pending → retrying → pending` (transient failure) or
/// `pending → failed` (terminal failure after exhausting retries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxStatus {
    Pending,
    Dispatching,
    Submitted,
    Retrying,
    Failed,
}

impl OutboxStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatching => "dispatching",
            Self::Submitted => "submitted",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "dispatching" => Ok(Self::Dispatching),
            "submitted" => Ok(Self::Submitted),
            "retrying" => Ok(Self::Retrying),
            "failed" => Ok(Self::Failed),
            other => Err(BusStorageError::InvalidColumn {
                column: "status",
                value: other.to_owned(),
            }),
        }
    }
}

/// One outbox row. Constructed by callers via the public builder
/// pattern below; the storage layer never invents `id` or
/// `idempotency_key`.
///
/// The delivery-guarantee bookkeeping fields (`status`, `attempt_count`,
/// `next_attempt_at`, `last_error`, `submitted_at`, `claim_token`) are
/// `pub(crate)` — visible throughout this crate (`outbox.rs`'s
/// `row_to_entry` reads the real row back from Postgres into them) but
/// not settable by an external caller hand-building an entry. The real
/// state-machine transitions (`claim_pending_outbox`, `mark_outbox_
/// submitted`, `mark_outbox_retry`) are SQL-guarded (`WHERE status =
/// 'dispatching' AND claim_token = $N`) regardless of what an in-memory
/// struct claims, so this closes a construction-time forgery risk, not
/// a runtime one. `attempt_count` and `claim_token` have real external
/// readers (the sender's backoff/claim-token bookkeeping) and get pub
/// accessors below; the rest have none today.
#[derive(Debug, Clone)]
pub struct OutboxEntry {
    pub id: Uuid,
    pub target_domain: String,
    pub target_endpoint: BusEndpoint,
    pub payload: Vec<u8>,
    pub idempotency_key: Uuid,
    pub execution_id: Option<Uuid>,
    pub callout_id: Option<Uuid>,
    pub(crate) status: OutboxStatus,
    pub(crate) attempt_count: i32,
    pub(crate) next_attempt_at: DateTime<Utc>,
    pub(crate) last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub(crate) submitted_at: Option<DateTime<Utc>>,
    pub tenant_id: String,
    pub(crate) claim_token: Option<Uuid>,
}

impl OutboxEntry {
    /// Construct a fresh pending row ready for [`insert_outbox`].
    ///
    /// `id` and `idempotency_key` are caller-supplied — typically both
    /// are UUIDv7 minted by the sender at enqueue time.
    ///
    /// [`insert_outbox`]: crate::insert_outbox
    pub fn new_pending(
        id: Uuid,
        target_domain: impl Into<String>,
        target_endpoint: BusEndpoint,
        payload: Vec<u8>,
        idempotency_key: Uuid,
        tenant_id: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            target_domain: target_domain.into(),
            target_endpoint,
            payload,
            idempotency_key,
            execution_id: None,
            callout_id: None,
            status: OutboxStatus::Pending,
            attempt_count: 0,
            next_attempt_at: now,
            last_error: None,
            created_at: now,
            submitted_at: None,
            tenant_id,
            claim_token: None,
        }
    }

    /// Attach a bpmn-lite caller-side `callout_id` (§8.3) so the sender
    /// loop can correlate a process-instance callout with the row.
    fn with_callout_id(mut self, callout_id: Uuid) -> Self {
        self.callout_id = Some(callout_id);
        self
    }

    /// Current retry count. Read by the sender to compute exponential
    /// backoff before calling [`crate::mark_outbox_retry`].
    pub fn attempt_count(&self) -> i32 {
        self.attempt_count
    }

    /// The dispatch lease token assigned by [`crate::claim_pending_outbox`],
    /// if this row is currently claimed. The sender must pass this back
    /// unchanged to [`crate::mark_outbox_submitted`] /
    /// [`crate::mark_outbox_retry`] — those functions re-check it against
    /// the row's actual claim_token in SQL, so a forged value here is
    /// harmless (it would just fail with [`crate::BusStorageError::LostClaim`]),
    /// but the type still shouldn't invite hand-editing it.
    pub fn claim_token(&self) -> Option<Uuid> {
        self.claim_token
    }
}

/// Inbox row state machine: `received → processed`. The row is keyed
/// by `idempotency_key`, so the receiver gRPC handler can short-circuit
/// duplicate Submits and reply with the cached outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InboxStatus {
    Received,
    Processed,
}

impl InboxStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Processed => "processed",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s {
            "received" => Ok(Self::Received),
            "processed" => Ok(Self::Processed),
            other => Err(BusStorageError::InvalidColumn {
                column: "status",
                value: other.to_owned(),
            }),
        }
    }
}

/// One inbox row. `status`/`processed_at` are `pub(crate)` for the same
/// reason as `OutboxEntry`'s state fields — see that struct's doc
/// comment. Zero external readers or writers of either field exist
/// today; `execution_id` does have a real external reader (the
/// duplicate-Submit replay path) and stays `pub`.
#[derive(Debug, Clone)]
pub struct InboxEntry {
    pub idempotency_key: Uuid,
    pub source_domain: String,
    pub endpoint: BusEndpoint,
    pub execution_id: Option<Uuid>,
    pub received_at: DateTime<Utc>,
    pub(crate) processed_at: Option<DateTime<Utc>>,
    pub(crate) status: InboxStatus,
    pub payload: Option<Vec<u8>>,
    pub tenant_id: String,
}

impl InboxEntry {
    /// Build a freshly-received row for [`insert_inbox`].
    ///
    /// [`insert_inbox`]: crate::insert_inbox
    pub fn new_received(
        idempotency_key: Uuid,
        source_domain: impl Into<String>,
        endpoint: BusEndpoint,
        execution_id: Option<Uuid>,
        payload: Option<Vec<u8>>,
        tenant_id: String,
    ) -> Self {
        Self {
            idempotency_key,
            source_domain: source_domain.into(),
            endpoint,
            execution_id,
            received_at: Utc::now(),
            processed_at: None,
            status: InboxStatus::Received,
            payload,
            tenant_id,
        }
    }
}

/// Result of an idempotent insert: did the row actually land, or did
/// the unique-key check short-circuit it?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    Duplicate,
}

impl InsertOutcome {
    pub const fn was_inserted(self) -> bool {
        matches!(self, Self::Inserted)
    }
}
