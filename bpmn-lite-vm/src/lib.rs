//! Compatibility utilities retained after the interpreter moved into
//! `bpmn-lite-kernel` in T7. This crate owns no workflow mutation path.

// Test-only (2026-07-30 clean-build pass): documented as the intended
// runtime implementation for BindingSource::DomainPayloadRef /
// BindingTarget::DomainPayloadWrite (see bpmn-lite-types::ffi_bindings'
// doc comments, which name read()/write_at_path() by full path), but
// bpmn-lite-kernel's dispatch loop doesn't call into it yet -- validated by
// this module's own 25 tests, not wired to a runtime path.
#[cfg(test)]
mod json_path;

pub fn compute_hash(data: &str) -> [u8; 32] {
    blake3::hash(data.as_bytes()).into()
}
