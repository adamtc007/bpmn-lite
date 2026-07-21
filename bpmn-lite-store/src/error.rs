#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("claim storage unavailable: {0}")]
    Unavailable(String),
    #[error("invalid claim data: {0}")]
    Invalid(String),
    #[error("claim integrity failure: {0}")]
    Integrity(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("transition revision conflict")]
    Conflict,
    #[error("stale transition fencing token")]
    StaleFence,
    #[error("transition integrity failure: {0}")]
    Integrity(String),
    #[error("transition storage unavailable: {0}")]
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed { new_revision: u64 },
    IdempotentNoOp,
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactStoreError {
    #[error("artifact hash collision for {hash:?}")]
    ArtifactCollision { hash: [u8; 32] },
    #[error("artifact failed verification: {0}")]
    InvalidArtifact(String),
    #[error("artifact storage unavailable: {0}")]
    Unavailable(String),
}

/// Backend-neutral failure for persistence operations outside the fenced
/// claim/commit and immutable-artifact paths, which retain their narrower
/// error taxonomies.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store integrity failure: {0}")]
    Integrity(String),
    #[error("invalid store input: {0}")]
    Invalid(String),
    #[error("store resource not found: {0}")]
    NotFound(String),
    #[error("store unavailable: {0}")]
    Unavailable(String),
}

impl StoreError {
    pub fn integrity(error: impl std::fmt::Display) -> Self {
        Self::Integrity(error.to_string())
    }

    pub fn invalid(error: impl std::fmt::Display) -> Self {
        Self::Invalid(error.to_string())
    }

    pub fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }
}

pub type StoreResult<T> = std::result::Result<T, StoreError>;
