//! `FfiExecutionOwner` — the trait implemented by every registered owner.
//!
//! Per A2 §5. Owners are async; the dispatcher awaits each invocation.
//! In-process for v1.1; out-of-process owners (RPC, subprocess) are a
//! future extension that does not change this trait surface.

use crate::wire::{FfiCall, FfiResult};
use async_trait::async_trait;

/// Implementor of a foreign function vocabulary.
///
/// Contract:
///
/// 1. `owner_type()` is the stable string registered with the dispatcher
///    (e.g. `"dmn-lite"`). Two owners with the same `owner_type` may not
///    be registered simultaneously.
/// 2. `supports_template(id)` is consulted at startup validation. An owner
///    whose `owner_type` matches a template's `owner_type` in the catalogue
///    must return `true` for that template's id, otherwise startup fails.
/// 3. `invoke(call)` returns `FfiResult`. The owner MUST NOT panic; any
///    error must be reported via `FfiResult::Incident`.
/// 4. An owner declared `Idempotent` in the catalogue MUST produce the same
///    result for the same `FfiCall`. The dispatcher relies on this for
///    crash-recovery re-invocation.
#[async_trait]
pub trait FfiExecutionOwner: Send + Sync {
    /// Stable registration identifier.
    fn owner_type(&self) -> &str;

    /// Invoke the template identified by `call.template_id`.
    async fn invoke(&self, call: FfiCall) -> anyhow::Result<FfiResult>;

    /// True if this owner handles the given template_id. Default
    /// implementation accepts any template (suitable for owners that
    /// validate templates lazily at invocation time).
    fn supports_template(&self, _template_id: &[u8; 32]) -> bool {
        true
    }
}
