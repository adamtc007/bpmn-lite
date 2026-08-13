//! Compatibility utilities retained after the interpreter moved into
//! `bpmn-lite-kernel` in T7. This crate owns no workflow mutation path.
//!
//! `compute_hash` (H2, EOP-PLAN-CRATE-HYGIENE-001) was retired here — it
//! duplicated `bpmn_lite_types::EffectId::content_hash`, the crate that
//! already owns the domain payload-hash contract; every caller was
//! migrated to call that directly.

// Test-only (2026-07-30 clean-build pass): documented as the intended
// runtime implementation for BindingSource::DomainPayloadRef /
// BindingTarget::DomainPayloadWrite (see bpmn-lite-types::ffi_bindings'
// doc comments, which name read()/write_at_path() by full path), but
// bpmn-lite-kernel's dispatch loop doesn't call into it yet -- validated by
// this module's own 25 tests, not wired to a runtime path.
#[cfg(test)]
mod json_path;
