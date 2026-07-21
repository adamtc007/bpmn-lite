use bpmn_lite_types::{Timestamp, Uuid};
use std::sync::atomic::{AtomicU64, Ordering};

/// The only engine boundary permitted to read ambient time or allocate IDs.
pub trait RuntimeContext: Send + Sync {
    fn logical_time(&self) -> Result<Timestamp, RuntimeContextError>;
    fn new_id(&self) -> Uuid;
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeContextError {
    #[error("system clock is before the Unix epoch")]
    BeforeUnixEpoch,
    #[error("system clock exceeds the workflow timestamp range")]
    TimestampOverflow,
}

#[derive(Debug, Default)]
pub struct SystemRuntimeContext;

impl RuntimeContext for SystemRuntimeContext {
    fn logical_time(&self) -> Result<Timestamp, RuntimeContextError> {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| RuntimeContextError::BeforeUnixEpoch)?
            .as_millis();
        i64::try_from(millis).map_err(|_| RuntimeContextError::TimestampOverflow)
    }

    fn new_id(&self) -> Uuid {
        Uuid::now_v7()
    }
}

/// Reproducible clock/ID source for transition and recovery tests.
#[derive(Debug)]
pub struct DeterministicRuntimeContext {
    logical_time: Timestamp,
    seed: u128,
    next: AtomicU64,
}

impl DeterministicRuntimeContext {
    pub fn new(logical_time: Timestamp, seed: Uuid) -> Self {
        Self {
            logical_time,
            seed: seed.as_u128(),
            next: AtomicU64::new(0),
        }
    }
}

impl RuntimeContext for DeterministicRuntimeContext {
    fn logical_time(&self) -> Result<Timestamp, RuntimeContextError> {
        Ok(self.logical_time)
    }

    fn new_id(&self) -> Uuid {
        let ordinal = u128::from(self.next.fetch_add(1, Ordering::Relaxed));
        Uuid::from_u128(self.seed.wrapping_add(ordinal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_context_replays_clock_and_id_sequence() {
        let seed = Uuid::from_u128(41);
        let first = DeterministicRuntimeContext::new(1234, seed);
        let second = DeterministicRuntimeContext::new(1234, seed);
        assert_eq!(
            first.logical_time().unwrap(),
            second.logical_time().unwrap()
        );
        assert_eq!(first.new_id(), second.new_id());
        assert_eq!(first.new_id(), second.new_id());
    }
}
