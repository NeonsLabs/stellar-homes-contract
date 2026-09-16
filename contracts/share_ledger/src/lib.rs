#![no_std]
//! ShareLedger — the cap table, and the rental income each share has earned.
//!
//! One property's shares are tracked entirely separately from another's. There
//! is no global balance and no pooling: a holder's position is keyed by
//! `(property, holder)`, so nothing that happens to one building can reach the
//! shareholders of another.
//!
//! The ledger also owns the income accounting, because the cap table is the
//! only thing that knows who held what and when. It holds no money — the
//! IncomeDistributor custodies the settlement asset and tells the ledger how
//! much arrived; the ledger decides whose it is.
//!
//! ## How income is split without iterating over holders
//!
//! A contract cannot loop over every shareholder to pay them, so entitlement is
//! accrued rather than pushed. Each property carries a running
//! `acc_per_share`: the total income ever deposited for it, divided by its
//! issued shares, scaled by [`SCALE`] to survive integer division. A holder's
//! position records `reward_debt`, the value of that accumulator against their
//! balance the last time it was settled. What they are owed is the growth in
//! the accumulator since, times their balance.
//!
//! Every balance change settles the position first, banking what the old
//! balance earned into `credited` before resetting `reward_debt` against the
//! new one. Selling shares therefore never forfeits income already earned, and
//! buying them never lays claim to income earned before the purchase.
//!
//! The remainder of each division is not lost. It is kept on the property in
//! the accumulator's own scaled units and folded into the next deposit, so rent
//! that does not divide evenly is paid out later rather than stranded.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, Symbol,
};

/// Ledgers per day at Stellar's 5-second ledger close time.
const DAY_IN_LEDGERS: u32 = 17_280;
/// Storage lifetimes, in ledgers. Entries are extended to about 120 days
/// whenever they fall below about 90, so they stay live while in use without
/// paying rent on every call. Both are well under the network's maximum entry
/// lifetime of about 180 days.
const EXTEND_TO: u32 = 120 * DAY_IN_LEDGERS;
const THRESHOLD: u32 = 90 * DAY_IN_LEDGERS;

/// Fixed-point scale for `acc_per_share`.
///
/// Income per share is a fraction, and integer division would throw most of it
/// away on a property with many shares. Scaling by 10^12 before dividing keeps
/// twelve digits of it. The registry caps a property at 10^12 shares, so the
/// scaled accumulator stays far inside `i128` even after a lifetime of rent.
pub const SCALE: i128 = 1_000_000_000_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 2,
    NotAuthorized = 3,
    AlreadySet = 4,
    NoPendingAction = 5,
    ActionPending = 6,
    TimelockNotExpired = 7,
    InvalidAmount = 8,
    InsufficientShares = 9,
    OfferingNotSet = 10,
    DistributorNotSet = 11,
    NothingIssued = 12,
    SelfTransfer = 13,
}

/// One holder's stake in one property.
///
/// `reward_debt` is bookkeeping, not a liability: it is the value of the
/// property's accumulator against this balance at the last settlement, and is
/// subtracted so the holder is paid only for growth since then. `credited` is
/// income already earned and set aside, waiting to be claimed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub balance: i128,
    pub reward_debt: i128,
    pub credited: i128,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    /// The Offering contract, the only caller allowed to issue shares.
    Offering,
    /// The IncomeDistributor, the only caller allowed to accrue and pay out.
    Distributor,
    Position(u64, Address),
    /// Shares issued so far for a property.
    Issued(u64),
    /// Scaled income per share, accumulated over the property's life.
    Acc(u64),
    /// Remainder from the last division, in scaled units, folded into the
    /// next deposit.
    Carry(u64),
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

const LEDGER: Symbol = symbol_short!("ledger");

#[contract]
pub struct ShareLedgerContract;

#[contractimpl]
impl ShareLedgerContract {
    /// Runs once, atomically, as part of the deploy transaction. There is no
    /// separate initialize call for anyone to front-run between deployment and
    /// setup, so nobody else can claim the admin role.
    pub fn __constructor(env: Env, admin: Address, timelock_secs: u64) {
        Self::extend_instance(&env);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::TimelockSecs, &timelock_secs);
    }

    // --- Administration ---

    /// Register the Offering contract, the only caller allowed to issue shares.
    /// Can be set only once: rewiring it later would let the admin point the
    /// ledger at a contract that mints itself a majority of any property, so a
    /// change goes through an upgrade and its timelock instead.
    pub fn set_offering(env: Env, admin: Address, offering: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::Offering) {
            panic_with_error!(&env, Error::AlreadySet);
        }
        env.storage().instance().set(&DataKey::Offering, &offering);
    }

    /// Register the IncomeDistributor, the only caller allowed to record
    /// arriving income and to draw a holder's accrued balance down. Can be set
    /// only once, for the same reason.
    pub fn set_distributor(env: Env, admin: Address, distributor: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::Distributor) {
            panic_with_error!(&env, Error::AlreadySet);
        }
        env.storage()
            .instance()
            .set(&DataKey::Distributor, &distributor);
    }

    /// Schedule an upgrade. It can execute only once the timelock set at
    /// deployment has elapsed, and can be cancelled at any time before that, so
    /// shareholders and the admin's other signers see the change coming.
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

    // --- Primary issuance ---

    /// Credit shares to an investor as a settled offering is claimed. Offering
    /// contract only — shares exist for no other reason.
    ///
    /// The ledger does not police the property's total supply; the Offering
    /// contract sells against the cap the registry recorded and can never issue
    /// more than it sold.
    pub fn issue(env: Env, caller: Address, property: u64, to: Address, shares: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_wired(&env, &caller, DataKey::Offering, Error::OfferingNotSet);
        if shares <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let acc = Self::acc_of(&env, property);
        let mut position = Self::position_of(&env, property, &to);
        Self::settle(&mut position, acc);
        position.balance += shares;
        Self::reset_debt(&mut position, acc);
        Self::save_position(&env, property, &to, &position);

        let issued = Self::issued_of(&env, property) + shares;
        Self::set_issued(&env, property, issued);

        env.events().publish(
            (LEDGER, symbol_short!("issue")),
            (property, to, shares, issued),
        );
    }

    // --- Secondary market ---

    /// Move shares between holders. The seller signs; both sides' income is
    /// settled first, so the seller keeps everything earned up to this moment
    /// and the buyer earns only from here on.
    pub fn transfer(env: Env, property: u64, from: Address, to: Address, shares: i128) {
        Self::extend_instance(&env);
        from.require_auth();
        if shares <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        // Without this, settling `from` and then `to` would read and write the
        // same position twice and double-count the accrual.
        if from == to {
            panic_with_error!(&env, Error::SelfTransfer);
        }

        let acc = Self::acc_of(&env, property);

        let mut seller = Self::position_of(&env, property, &from);
        if seller.balance < shares {
            panic_with_error!(&env, Error::InsufficientShares);
        }
        Self::settle(&mut seller, acc);
        seller.balance -= shares;
        Self::reset_debt(&mut seller, acc);
        Self::save_position(&env, property, &from, &seller);

        let mut buyer = Self::position_of(&env, property, &to);
        Self::settle(&mut buyer, acc);
        buyer.balance += shares;
        Self::reset_debt(&mut buyer, acc);
        Self::save_position(&env, property, &to, &buyer);

        env.events().publish(
            (LEDGER, symbol_short!("transfer")),
            (property, from, to, shares),
        );
    }

    // --- Income accounting ---

    /// Record income that arrived for a property, spreading it over every share
    /// issued at this moment. IncomeDistributor only; it holds the money.
    ///
    /// The remainder of the division is kept in scaled units and folded into
    /// the next deposit, so nothing is lost to truncation over a property's
    /// life however awkwardly the rent divides.
    pub fn accrue(env: Env, caller: Address, property: u64, amount: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_wired(
            &env,
            &caller,
            DataKey::Distributor,
            Error::DistributorNotSet,
        );
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let issued = Self::issued_of(&env, property);
        // Nothing is issued before an offering settles. Income arriving then
        // has nobody to belong to, and spreading it later would hand it to
        // whoever bought first, so it is refused outright.
        if issued <= 0 {
            panic_with_error!(&env, Error::NothingIssued);
        }

        // The carry is already scaled, so it is added after the multiplication.
        // Truncation here is sub-unit and never compounds: what does not divide
        // stays in the carry and is distributed by a later deposit.
        let scaled = amount * SCALE + Self::carry_of(&env, property);
        Self::set_acc(
            &env,
            property,
            Self::acc_of(&env, property) + scaled / issued,
        );
        Self::set_carry(&env, property, scaled % issued);

        env.events().publish(
            (LEDGER, symbol_short!("accrue")),
            (property, amount, issued),
        );
    }

    /// Settle a holder's position and hand back everything owed, zeroing their
    /// credit. IncomeDistributor only, which pays out what this returns.
    ///
    /// Returning the amount rather than paying it keeps custody in one place:
    /// the ledger never moves money, and the distributor never does arithmetic.
    pub fn take_accrued(env: Env, caller: Address, property: u64, holder: Address) -> i128 {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_wired(
            &env,
            &caller,
            DataKey::Distributor,
            Error::DistributorNotSet,
        );

        let acc = Self::acc_of(&env, property);
        let mut position = Self::position_of(&env, property, &holder);
        Self::settle(&mut position, acc);
        let owed = position.credited;
        position.credited = 0;
        Self::save_position(&env, property, &holder, &position);

        if owed > 0 {
            env.events()
                .publish((LEDGER, symbol_short!("settled")), (property, holder, owed));
        }
        owed
    }

    // --- Getters ---

    pub fn balance_of(env: Env, property: u64, holder: Address) -> i128 {
        Self::position_of(&env, property, &holder).balance
    }

    pub fn position_of_holder(env: Env, property: u64, holder: Address) -> Position {
        Self::position_of(&env, property, &holder)
    }

    /// What `holder` could claim right now, without changing anything.
    pub fn accrued_of(env: Env, property: u64, holder: Address) -> i128 {
        let acc = Self::acc_of(&env, property);
        let mut position = Self::position_of(&env, property, &holder);
        Self::settle(&mut position, acc);
        position.credited
    }

    pub fn total_issued(env: Env, property: u64) -> i128 {
        Self::issued_of(&env, property)
    }

    pub fn acc_per_share(env: Env, property: u64) -> i128 {
        Self::acc_of(&env, property)
    }

    /// Income that did not divide evenly, in the accumulator's scaled units,
    /// waiting to be folded into the next deposit.
    pub fn carry_of_property(env: Env, property: u64) -> i128 {
        Self::carry_of(&env, property)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_offering(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Offering)
    }

    pub fn get_distributor(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Distributor)
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

    /// Bank what the current balance has earned since the last settlement.
    /// Must run before any balance change, and is idempotent once
    /// [`Self::reset_debt`] has followed it.
    fn settle(position: &mut Position, acc: i128) {
        let entitled = position.balance * acc / SCALE;
        position.credited += entitled - position.reward_debt;
        position.reward_debt = entitled;
    }

    /// Re-anchor the position against a changed balance, so the holder is never
    /// paid for growth that happened before they held these shares.
    fn reset_debt(position: &mut Position, acc: i128) {
        position.reward_debt = position.balance * acc / SCALE;
    }

    fn position_of(env: &Env, property: u64, holder: &Address) -> Position {
        let key = DataKey::Position(property, holder.clone());
        match env.storage().persistent().get::<DataKey, Position>(&key) {
            Some(position) => {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, THRESHOLD, EXTEND_TO);
                position
            }
            None => Position {
                balance: 0,
                reward_debt: 0,
                credited: 0,
            },
        }
    }

    /// Persist a position and extend its lifetime. Every mutation goes through
    /// here, so a position holding shares or unclaimed income is renewed each
    /// time it is used.
    fn save_position(env: &Env, property: u64, holder: &Address, position: &Position) {
        let key = DataKey::Position(property, holder.clone());
        env.storage().persistent().set(&key, position);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn read_property_i128(env: &Env, key: DataKey) -> i128 {
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

    fn write_property_i128(env: &Env, key: DataKey, value: i128) {
        env.storage().persistent().set(&key, &value);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn issued_of(env: &Env, property: u64) -> i128 {
        Self::read_property_i128(env, DataKey::Issued(property))
    }

    fn set_issued(env: &Env, property: u64, value: i128) {
        Self::write_property_i128(env, DataKey::Issued(property), value);
    }

    fn acc_of(env: &Env, property: u64) -> i128 {
        Self::read_property_i128(env, DataKey::Acc(property))
    }

    fn set_acc(env: &Env, property: u64, value: i128) {
        Self::write_property_i128(env, DataKey::Acc(property), value);
    }

    fn carry_of(env: &Env, property: u64) -> i128 {
        Self::read_property_i128(env, DataKey::Carry(property))
    }

    fn set_carry(env: &Env, property: u64, value: i128) {
        Self::write_property_i128(env, DataKey::Carry(property), value);
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

    fn require_wired(env: &Env, caller: &Address, key: DataKey, missing: Error) {
        let wired: Address = env
            .storage()
            .instance()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(env, missing));
        if *caller != wired {
            panic_with_error!(env, Error::NotAuthorized);
        }
    }
}

#[cfg(test)]
mod test;
