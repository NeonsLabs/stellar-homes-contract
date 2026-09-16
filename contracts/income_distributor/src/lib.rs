#![no_std]
//! IncomeDistributor — custody of rental income, and the window shareholders
//! collect it through.
//!
//! Rent arrives off-chain and is deposited here in the settlement asset. This
//! contract holds it and nothing else: it does no arithmetic about who is owed
//! what. That belongs to the ShareLedger, which is the only thing that knows
//! who held which shares when. The split is deliberate — the contract holding
//! the money cannot decide whose it is, and the contract deciding whose it is
//! cannot move it.
//!
//! Holders are never paid automatically. A contract cannot loop over every
//! shareholder, so each one claims their own, whenever they like, however many
//! deposits have piled up since they last did.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, token, Address, BytesN, Env, Symbol,
};

/// Ledgers per day at Stellar's 5-second ledger close time.
const DAY_IN_LEDGERS: u32 = 17_280;
/// Storage lifetimes, in ledgers. Entries are extended to about 120 days
/// whenever they fall below about 90, so they stay live while in use without
/// paying rent on every call. Both are well under the network's maximum entry
/// lifetime of about 180 days.
const EXTEND_TO: u32 = 120 * DAY_IN_LEDGERS;
const THRESHOLD: u32 = 90 * DAY_IN_LEDGERS;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 2,
    NotAuthorized = 3,
    NoPendingAction = 5,
    ActionPending = 6,
    TimelockNotExpired = 7,
    InvalidAmount = 8,
    NothingToClaim = 9,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    ShareLedger,
    SettlementToken,
    /// Running total ever deposited for a property, for reporting.
    Deposited(u64),
    /// Running total ever paid out for a property, for reporting.
    Claimed(u64),
}

/// A sensitive admin change that must wait out the timelock before it runs.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Replace the contract's code, keeping its address and storage.
    Upgrade(BytesN<32>),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledAction {
    pub action: Action,
    /// Earliest ledger time at which the action may execute.
    pub eta: u64,
}

/// The slice of the ShareLedger this contract calls.
///
/// Declared as a client rather than a dependency so the ledger's code is not
/// linked into this contract's wasm.
#[contractclient(name = "LedgerClient")]
pub trait Ledger {
    fn accrue(env: Env, caller: Address, property: u64, amount: i128);
    fn take_accrued(env: Env, caller: Address, property: u64, holder: Address) -> i128;
    fn accrued_of(env: Env, property: u64, holder: Address) -> i128;
}

const INCOME: Symbol = symbol_short!("income");

#[contract]
pub struct IncomeDistributorContract;

#[contractimpl]
impl IncomeDistributorContract {
    /// Runs once, atomically, as part of the deploy transaction. There is no
    /// separate initialize call for anyone to front-run between deployment and
    /// setup, so nobody else can claim the admin role.
    pub fn __constructor(
        env: Env,
        admin: Address,
        share_ledger: Address,
        settlement_token: Address,
        timelock_secs: u64,
    ) {
        Self::extend_instance(&env);
        let storage = env.storage().instance();
        storage.set(&DataKey::Admin, &admin);
        storage.set(&DataKey::ShareLedger, &share_ledger);
        storage.set(&DataKey::SettlementToken, &settlement_token);
        storage.set(&DataKey::TimelockSecs, &timelock_secs);
    }

    // --- Income ---

    /// Pay rent in for a property, to be shared out across its shares.
    ///
    /// Deliberately open to anyone. The usual depositor is the sponsor or the
    /// managing agent, but a deposit only ever gives money away to the people
    /// who already hold the shares, so there is nothing to gain by making one
    /// and nothing to protect by restricting it.
    ///
    /// Whether the money can be shared out at all is the ledger's call: it
    /// refuses a deposit for a property with no shares issued, which would have
    /// nobody to belong to.
    pub fn deposit_income(env: Env, property: u64, from: Address, amount: i128) {
        Self::extend_instance(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        // Accrued before the transfer, so a property that cannot take the money
        // rejects it before any of it is moved.
        Self::ledger(&env).accrue(&env.current_contract_address(), &property, &amount);
        Self::token(&env).transfer(&from, &env.current_contract_address(), &amount);

        let total = Self::total(&env, DataKey::Deposited(property)) + amount;
        Self::set_total(&env, DataKey::Deposited(property), total);

        env.events().publish(
            (INCOME, symbol_short!("deposit")),
            (property, from, amount, total),
        );
    }

    /// Collect everything a holder has earned on a property and not yet taken.
    ///
    /// The amount comes from the ledger, which settles the holder's position in
    /// the same call and zeroes it, so a second claim in the same breath finds
    /// nothing left.
    pub fn claim(env: Env, property: u64, holder: Address) -> i128 {
        Self::extend_instance(&env);
        holder.require_auth();

        let owed =
            Self::ledger(&env).take_accrued(&env.current_contract_address(), &property, &holder);
        if owed <= 0 {
            panic_with_error!(&env, Error::NothingToClaim);
        }

        Self::token(&env).transfer(&env.current_contract_address(), &holder, &owed);

        let total = Self::total(&env, DataKey::Claimed(property)) + owed;
        Self::set_total(&env, DataKey::Claimed(property), total);

        env.events()
            .publish((INCOME, symbol_short!("claim")), (property, holder, owed));
        owed
    }

    // --- Administration ---

    /// Schedule an upgrade. It can execute only once the timelock set at
    /// deployment has elapsed, and can be cancelled at any time before that, so
    /// shareholders and the admin's other signers see the change coming.
    ///
    /// There is no admin path that moves money: unclaimed income stays
    /// claimable by its owner indefinitely, and no sweep exists to take it back.
    pub fn schedule_action(env: Env, admin: Address, action: Action) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::Scheduled) {
            panic_with_error!(&env, Error::ActionPending);
        }
        let delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TimelockSecs)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotInitialized));
        let scheduled = ScheduledAction {
            action,
            eta: env.ledger().timestamp() + delay,
        };
        env.storage()
            .instance()
            .set(&DataKey::Scheduled, &scheduled);
    }

    /// Carry out the scheduled action once its timelock has elapsed.
    pub fn execute_action(env: Env, admin: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        let scheduled: ScheduledAction = env
            .storage()
            .instance()
            .get(&DataKey::Scheduled)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NoPendingAction));
        if env.ledger().timestamp() < scheduled.eta {
            panic_with_error!(&env, Error::TimelockNotExpired);
        }
        env.storage().instance().remove(&DataKey::Scheduled);
        match scheduled.action {
            Action::Upgrade(wasm_hash) => env.deployer().update_current_contract_wasm(wasm_hash),
        }
    }

    /// Withdraw the scheduled action before it executes.
    pub fn cancel_action(env: Env, admin: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if !env.storage().instance().has(&DataKey::Scheduled) {
            panic_with_error!(&env, Error::NoPendingAction);
        }
        env.storage().instance().remove(&DataKey::Scheduled);
    }

    /// Begin handing the admin role to `new_admin`. Nothing changes until the
    /// new admin accepts, so a mistyped address cannot lock the protocol out.
    pub fn propose_admin(env: Env, admin: Address, new_admin: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
    }

    /// Complete a handover proposed by the current admin.
    pub fn accept_admin(env: Env, new_admin: Address) {
        new_admin.require_auth();
        Self::extend_instance(&env);
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotAuthorized));
        if new_admin != pending {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
    }

    // --- Getters ---

    /// What `holder` could claim right now, without changing anything.
    pub fn claimable(env: Env, property: u64, holder: Address) -> i128 {
        Self::ledger(&env).accrued_of(&property, &holder)
    }

    pub fn total_deposited(env: Env, property: u64) -> i128 {
        Self::total(&env, DataKey::Deposited(property))
    }

    pub fn total_claimed(env: Env, property: u64) -> i128 {
        Self::total(&env, DataKey::Claimed(property))
    }

    /// Income received for a property and not yet collected by its holders.
    pub fn unclaimed(env: Env, property: u64) -> i128 {
        Self::total(&env, DataKey::Deposited(property))
            - Self::total(&env, DataKey::Claimed(property))
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_share_ledger(env: Env) -> Address {
        env.storage().instance().get(&DataKey::ShareLedger).unwrap()
    }

    pub fn get_settlement_token(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::SettlementToken)
            .unwrap()
    }

    pub fn get_scheduled_action(env: Env) -> Option<ScheduledAction> {
        env.storage().instance().get(&DataKey::Scheduled)
    }

    pub fn get_timelock_secs(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::TimelockSecs)
            .unwrap_or(0)
    }

    // --- Internals ---

    /// Keep the contract instance, and with it the configuration and the
    /// contract code, from being archived while the protocol is in use.
    fn extend_instance(env: &Env) {
        env.storage().instance().extend_ttl(THRESHOLD, EXTEND_TO);
    }

    fn total(env: &Env, key: DataKey) -> i128 {
        match env.storage().persistent().get::<DataKey, i128>(&key) {
            Some(value) => {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, THRESHOLD, EXTEND_TO);
                value
            }
            None => 0,
        }
    }

    fn set_total(env: &Env, key: DataKey, value: i128) {
        env.storage().persistent().set(&key, &value);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn require_admin(env: &Env, admin: &Address) {
        admin.require_auth();
        let stored: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        if *admin != stored {
            panic_with_error!(env, Error::NotAuthorized);
        }
    }

    fn ledger(env: &Env) -> LedgerClient<'_> {
        let address: Address = env
            .storage()
            .instance()
            .get(&DataKey::ShareLedger)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        LedgerClient::new(env, &address)
    }

    fn token(env: &Env) -> token::Client<'_> {
        let address: Address = env
            .storage()
            .instance()
            .get(&DataKey::SettlementToken)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        token::Client::new(env, &address)
    }
}

#[cfg(test)]
mod test;
