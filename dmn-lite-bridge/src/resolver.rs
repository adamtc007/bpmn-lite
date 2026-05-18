//! `ValueResolver` — converts symbol strings to `TypedValue::Enum` for
//! SemOsDomain input fields. Optional; without it, SemOsDomain inputs fall
//! back to `TypedValue::Str(symbol)` which will cause a dmn-lite
//! `InputTypeMismatch` at evaluation time.

use dmn_lite_types::ids::DomainId;
use dmn_lite_types::ir::TypedValue;

/// Resolves a bare symbol string (e.g. `"LU"`) to a `TypedValue::Enum` for
/// a declared Sem OS domain input field.
pub trait ValueResolver: Send + Sync {
    fn resolve(&self, domain_id: &DomainId, symbol: &str) -> Option<TypedValue>;
}

/// A no-op resolver that always returns `None` (falls back to Str).
pub struct NoopResolver;

impl ValueResolver for NoopResolver {
    fn resolve(&self, _domain_id: &DomainId, _symbol: &str) -> Option<TypedValue> {
        None
    }
}
