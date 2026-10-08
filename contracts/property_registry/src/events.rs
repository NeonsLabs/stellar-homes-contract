//! Standardized event topics for milestone inspection evidence.
use soroban_sdk::{contracttype, Address, BytesN};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneSubmittedEvent {
    pub property_id: u64,
    pub stage: u32,
    pub trustee: Address,
    pub evidence_hash: BytesN<32>,
}
