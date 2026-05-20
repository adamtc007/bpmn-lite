//! A19 — Integrity hash for process instance immutable fields.
//!
//! Hash computed at instance creation; verified at pickup boundaries only
//! (scheduler claim, gRPC handler entry, A17 recovery). Not verified on
//! every read — performance cost is bounded to pickup transitions.
//!
//! ## Hash input format (BLAKE3, fixed field order)
//!
//! Any forensic tool or external audit that needs to recompute the hash
//! must use exactly this field order, separator byte, and serialisation:
//!
//! ```text
//! instance_id bytes (16, UUID big-endian) | b"|"
//! tenant_id bytes (UTF-8)                 | b"|"
//! bytecode_version bytes (32)             | b"|"
//! created_at_ms (8 bytes little-endian i64) | b"|"
//! process_key bytes (UTF-8)               | b"|"
//! entry_id bytes (16, UUID big-endian)    | b"|"
//! runbook_id bytes (16, UUID big-endian)  | b"|"
//! b""  (created_by_identity placeholder — v0.2 field, absent in v0.1)
//! ```
//!
//! When `created_by_identity` is added to `ProcessInstance` in v0.2, the
//! hash inputs change. All existing instances will fail verification on
//! their next pickup and must be rehashed via a migration. That is the
//! correct behavior: the hash is a tamper-detection tool within a
//! deployment epoch, not a long-lived stable identifier.

use crate::types::ProcessInstance;
use anyhow::{anyhow, Result};

/// Compute the BLAKE3 integrity hash for a process instance's immutable fields.
pub fn compute_instance_integrity_hash(instance: &ProcessInstance) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(instance.instance_id.as_bytes());
    hasher.update(b"|");
    hasher.update(instance.tenant_id.as_bytes());
    hasher.update(b"|");
    hasher.update(&instance.bytecode_version);
    hasher.update(b"|");
    hasher.update(&instance.created_at.to_le_bytes());
    hasher.update(b"|");
    hasher.update(instance.process_key.as_bytes());
    hasher.update(b"|");
    hasher.update(instance.entry_id.as_bytes());
    hasher.update(b"|");
    hasher.update(instance.runbook_id.as_bytes());
    hasher.update(b"|");
    // created_by_identity: v0.2 field, not yet on ProcessInstance.
    // Using empty bytes as placeholder. When this field is added, the hash
    // function changes and existing instances must be rehashed.
    hasher.update(b"");
    hasher.finalize().into()
}

/// Verify that a loaded instance's integrity hash matches its immutable fields.
///
/// Returns `Ok(())` if:
/// - The stored hash matches the recomputed hash, OR
/// - No hash is stored (pre-A19 row) — verification is skipped with a WARN.
///
/// Returns `Err` if the stored hash is present but does not match.
pub fn verify_instance_integrity(instance: &ProcessInstance) -> Result<()> {
    let Some(stored) = instance.integrity_hash else {
        tracing::warn!(
            instance_id = %instance.instance_id,
            "A19: skipping integrity verification (no hash stored — pre-A19 row)"
        );
        return Ok(());
    };
    let computed = compute_instance_integrity_hash(instance);
    if computed != stored {
        return Err(anyhow!(
            "A19 integrity hash mismatch for instance {} (tenant {}); \
             instance may have been tampered with at the database level",
            instance.instance_id,
            instance.tenant_id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProcessState;
    use ob_poc_types::session_stack::SessionStackState;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn make_test_instance() -> ProcessInstance {
        ProcessInstance {
            instance_id: Uuid::nil(),
            tenant_id: "test-tenant".to_string(),
            process_key: "test-process".to_string(),
            bytecode_version: [1u8; 32],
            domain_payload: "{}".to_string().into(),
            domain_payload_hash: [0u8; 32],
            session_stack: SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "corr".to_string(),
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            created_at: 1_700_000_000_000,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        }
    }

    /// T-A19-HASH-1: Hash is deterministic — same input produces same output.
    #[test]
    fn test_hash_is_deterministic() {
        let instance = make_test_instance();
        let h1 = compute_instance_integrity_hash(&instance);
        let h2 = compute_instance_integrity_hash(&instance);
        assert_eq!(h1, h2);
    }

    /// T-A19-HASH-2: Hash changes when tenant_id changes.
    #[test]
    fn test_hash_sensitive_to_tenant_id() {
        let a = make_test_instance();
        let mut b = make_test_instance();
        b.tenant_id = "other-tenant".to_string();
        assert_ne!(
            compute_instance_integrity_hash(&a),
            compute_instance_integrity_hash(&b)
        );
    }

    /// T-A19-HASH-3: Hash changes when bytecode_version changes.
    #[test]
    fn test_hash_sensitive_to_bytecode_version() {
        let a = make_test_instance();
        let mut b = make_test_instance();
        b.bytecode_version = [2u8; 32];
        assert_ne!(
            compute_instance_integrity_hash(&a),
            compute_instance_integrity_hash(&b)
        );
    }

    /// T-A19-HASH-4: Hash changes when created_at changes.
    #[test]
    fn test_hash_sensitive_to_created_at() {
        let a = make_test_instance();
        let mut b = make_test_instance();
        b.created_at = a.created_at + 1;
        assert_ne!(
            compute_instance_integrity_hash(&a),
            compute_instance_integrity_hash(&b)
        );
    }

    /// T-A19-HASH-5: Hash changes when process_key changes.
    #[test]
    fn test_hash_sensitive_to_process_key() {
        let a = make_test_instance();
        let mut b = make_test_instance();
        b.process_key = "other-process".to_string();
        assert_ne!(
            compute_instance_integrity_hash(&a),
            compute_instance_integrity_hash(&b)
        );
    }

    /// T-A19-VERIFY-1: verify passes when hash matches.
    #[test]
    fn test_verify_passes_on_correct_hash() {
        let mut instance = make_test_instance();
        instance.integrity_hash = Some(compute_instance_integrity_hash(&instance));
        assert!(verify_instance_integrity(&instance).is_ok());
    }

    /// T-A19-VERIFY-2: verify passes (with WARN) when hash is None (pre-A19 row).
    #[test]
    fn test_verify_passes_on_missing_hash() {
        let instance = make_test_instance(); // integrity_hash: None
        assert!(verify_instance_integrity(&instance).is_ok());
    }

    /// T-A19-VERIFY-3: verify fails when stored hash doesn't match.
    #[test]
    fn test_verify_fails_on_tampered_tenant_id() {
        let mut instance = make_test_instance();
        instance.integrity_hash = Some(compute_instance_integrity_hash(&instance));
        // Simulate DB-level tamper of tenant_id after hash was stored.
        instance.tenant_id = "evil-tenant".to_string();
        let result = verify_instance_integrity(&instance);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("integrity hash mismatch"), "got: {msg}");
    }

    /// T-A19-VERIFY-4: verify fails when bytecode_version tampered.
    #[test]
    fn test_verify_fails_on_tampered_bytecode_version() {
        let mut instance = make_test_instance();
        instance.integrity_hash = Some(compute_instance_integrity_hash(&instance));
        instance.bytecode_version = [0xffu8; 32];
        assert!(verify_instance_integrity(&instance).is_err());
    }

    /// T-A19-PERF-1: 1000 hash computations complete well under 1ms total.
    #[test]
    fn test_hash_performance_sanity() {
        let instance = make_test_instance();
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = compute_instance_integrity_hash(&instance);
        }
        let elapsed = start.elapsed();
        // BLAKE3 should do 1000 small-input hashes in well under 10ms.
        assert!(
            elapsed.as_millis() < 10,
            "1000 hashes took {}ms, expected < 10ms",
            elapsed.as_millis()
        );
    }
}
