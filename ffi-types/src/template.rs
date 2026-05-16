//! `FfiTemplate` — the published-function catalogue entry.
//!
//! Per A2 §3. A template's identity is the BLAKE3 digest of its canonical
//! encoding (see [`crate::canonical`]). Templates are immutable after
//! publication; a changed payload produces a new template_id.

use crate::idempotency::Idempotency;
use crate::schema::FieldSchema;
use serde::{Deserialize, Serialize};

/// The tenant ID under which globally-visible templates are stored.
///
/// Templates with this tenant_id are visible to all tenants. The constant
/// is a stable UUID string; in production the value `"00000000-0000-0000-0000-000000000000"`
/// is reserved. Promotion-to-GLOBAL governance is out of scope for v1.1.
pub const GLOBAL_TENANT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// A published FFI template.
///
/// Identity is `template_id` — the BLAKE3 digest of the canonical encoding
/// (see [`crate::canonical::compute_template_id`]). Same `owner_type` +
/// same schemas + same `idempotency` + same `owner_metadata` → same
/// `template_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FfiTemplate {
    /// 32-byte BLAKE3 digest of the canonical encoding.
    pub template_id: [u8; 32],

    /// Owner vocabulary. Registered values: "dmn-lite", "http", "grpc",
    /// "bpmn-lite". Free-text; the dispatcher matches against registered
    /// owner types by string equality.
    pub owner_type: String,

    /// Owner-specific binary metadata. For dmn-lite: serialised
    /// `VerifiedDecision` bytes. For HTTP: JSON describing url, method,
    /// headers template. For gRPC: proto descriptor. The bpmn-lite engine
    /// treats this as opaque and forwards it (via the catalogue) to the
    /// registered owner at call time.
    pub owner_metadata: Vec<u8>,

    pub input_schema: Vec<FieldSchema>,
    pub output_schema: Vec<FieldSchema>,

    pub idempotency: Idempotency,

    /// Tenant scope. Either a tenant UUID string or [`GLOBAL_TENANT_ID`].
    pub tenant_id: String,

    /// Epoch milliseconds when the template was published.
    pub published_at: i64,

    /// Free-text publisher identifier (user, service, system).
    pub publisher: String,
}
