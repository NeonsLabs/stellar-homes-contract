//! Multi-oracle valuation consensus helper.
use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleAppraisal {
    pub oracle: Address,
    pub valuation_units: i128,
    pub timestamp: u64,
}

pub fn calculate_median_valuation(mut values: [i128; 3]) -> i128 {
    values.sort();
    values[1]
}
