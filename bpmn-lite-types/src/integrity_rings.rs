//! D3 — five concentric integrity rings (EOP-VS-BPMN-ISA-002 §6).
//!
//! V1 declares the single shared error type every ring's detection surface
//! reports through (`IntegrityError`), starting with the Ring-1 tripwire
//! variant. The rings' actual detection logic (pre-decode hash
//! verification, frame-chain three-way agreement, runtime shadow asserts,
//! replay divergence, atomic quarantine) is wired incrementally across
//! V2-V4; V1 only fixes the type every ring reports through, per the
//! plan's "checked pre-decode, wired fully in V2, declared here" split.
//!
//! Distinct from `crate::integrity::IntegrityViolation` — that is the
//! unrelated A19 instance-field tamper-detection hash, not the D3 ring
//! model.

use serde::{Deserialize, Serialize};

/// Which tripwire surface fired. Each surface accepts exactly one value;
/// any other value is corruption or foreign data (V&S §6: "Versioning is a
/// tripwire, not a dispatch key").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum TripwireSurface {
    ArtifactAbi,
    SnapshotSchema,
    JournalSchema,
}

impl std::fmt::Display for TripwireSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::ArtifactAbi => "ArtifactAbi",
            Self::SnapshotSchema => "SnapshotSchema",
            Self::JournalSchema => "JournalSchema",
        };
        write!(f, "{name}")
    }
}

/// The typed error every D3 ring reports through, naming the firing ring
/// (V&S §6, Ring 5: "typed `IntegrityError` variant naming the ring"). Only
/// the Ring-1 tripwire variant is declared in V1; Ring 2-5 variants are
/// added as V2-V4 wire their detection logic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrityError {
    /// Ring 1: a versioned surface decoded to anything other than its one
    /// accepted value. Enforcement (checked pre-decode, on the envelope,
    /// before the corrupted frame reaches the deserializer) lands in V2;
    /// this variant is declared now so downstream call sites — including
    /// the pre-existing `PersistenceEnvelopeError::UnsupportedVersion` and
    /// `ReplayError::ArtifactAbiMismatch` tripwires — have a stable target
    /// type to converge on rather than three divergent ad hoc mechanisms.
    #[error("tripwire violation on {surface}: expected schema {expected}, found {actual}")]
    Tripwire {
        surface: TripwireSurface,
        expected: u32,
        actual: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tripwire_names_the_firing_surface() {
        let error = IntegrityError::Tripwire {
            surface: TripwireSurface::SnapshotSchema,
            expected: 1,
            actual: 99,
        };
        assert_eq!(
            error.to_string(),
            "tripwire violation on SnapshotSchema: expected schema 1, found 99"
        );
    }
}
