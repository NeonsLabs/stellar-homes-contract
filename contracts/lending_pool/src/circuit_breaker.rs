//! Emergency pause state handling for uncommitted capital protection.
use soroban_sdk::contracttype;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CircuitBreakerState {
    pub paused: bool,
    pub paused_at: u64,
}

impl CircuitBreakerState {
    pub fn can_commit(&self) -> bool {
        !self.paused
    }
}
