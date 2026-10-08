//! Batch helper structures for institutional investor allocations.
use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchDepositItem {
    pub investor: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchWithdrawItem {
    pub investor: Address,
    pub shares: i128,
}
