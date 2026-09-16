#![no_std]
//! LendingPool — the investors' capital, and the yield the mortgages pay it.
//!
//! Investors deposit the settlement asset and receive a claim on the pool
//! proportional to what they put in. The MortgagePool borrows against that
//! capital to fund approved mortgages, and pays interest back as borrowers
//! repay. The interest is what investors earn.
//!
//! This contract is the treasury: it holds every cent, and it is the only
//! contract that moves money. It does not know what a mortgage is. The
//! MortgagePool tells it how much to reserve, where to pay a tranche, and how
//! much of an arriving repayment is interest — and the pool does the
//! accounting and the transfers.
//!
//! ## Committed money cannot be withdrawn
//!
//! When a mortgage is approved, its whole principal is **reserved** against the
//! pool even though it will be paid out in tranches over months of building.
//! Only unreserved capital can be withdrawn by investors. Without that, a pool
//! could approve a mortgage, let its investors withdraw, and then find itself
//! unable to pay the roofing tranche of a half-built house.
//!
//! ## How yield is split without iterating over investors
//!
//! A contract cannot loop over every depositor to pay them, so interest is
//! accrued rather than pushed. The pool carries a running `acc_per_share`:
//! every unit of interest ever received, divided by the shares outstanding at
//! the time, scaled by [`SCALE`] to survive integer division. An investor's
//! position records what that accumulator was worth against their shares at the
//! last settlement; what they are owed is the growth since, times their shares.
//!
//! Every share change settles first and re-anchors after, so depositing does
//! not lay claim to interest earned before the deposit, and withdrawing does
//! not forfeit interest already earned.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    Address, BytesN, Env, Symbol,
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
/// Interest per share is a fraction, and integer division would throw most of
/// it away on a pool with many shares. Scaling by 10^12 before dividing keeps
/// twelve digits of it, and leaves the accumulator far inside `i128` even after
/// a lifetime of repayments.
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
    InsufficientAvailable = 10,
    InsufficientReserved = 11,
    MortgagePoolNotSet = 12,
    NothingDeposited = 13,
    NothingToClaim = 14,
}

/// One investor's stake in the pool.
///
/// `shares` is their claim on the capital. `reward_debt` is bookkeeping, not a
/// liability: it is the accumulator's value against these shares at the last
/// settlement, subtracted so they earn only on growth since. `credited` is
/// interest already earned and set aside, waiting to be claimed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub shares: i128,
    pub reward_debt: i128,
    pub credited: i128,
}

/// The pool's capital at a glance.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolState {
    /// Principal investors have put in and not withdrawn.
    pub total_capital: i128,
    /// Committed to approved mortgages, whether or not yet disbursed.
    pub total_reserved: i128,
    /// Principal currently out with borrowers.
    pub total_lent: i128,
    /// Interest received over the pool's life.
    pub total_interest: i128,
    /// Principal written off by defaults.
    pub total_written_off: i128,
    pub total_shares: i128,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    SettlementToken,
    /// The MortgagePool, the only caller allowed to reserve, lend or settle.
    MortgagePool,
    Position(Address),
    TotalCapital,
    TotalReserved,
    TotalLent,
    TotalInterest,
    TotalWrittenOff,
    TotalShares,
    Acc,
    Carry,
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

const POOL: Symbol = symbol_short!("pool");

#[contract]
pub struct LendingPoolContract;

#[contractimpl]
impl LendingPoolContract {
    /// Runs once, atomically, as part of the deploy transaction. There is no
    /// separate initialize call for anyone to front-run between deployment and
    /// setup, so nobody else can claim the admin role.
    pub fn __constructor(env: Env, admin: Address, settlement_token: Address, timelock_secs: u64) {
        Self::extend_instance(&env);
        let storage = env.storage().instance();
        storage.set(&DataKey::Admin, &admin);
        storage.set(&DataKey::SettlementToken, &settlement_token);
        storage.set(&DataKey::TimelockSecs, &timelock_secs);
    }

    // --- Administration ---

    /// Register the MortgagePool, the only caller allowed to reserve capital,
    /// draw it down or settle a repayment. Can be set only once: rewiring it
    /// later would let the admin point the treasury at a contract that pays
    /// itself, so a change goes through an upgrade and its timelock instead.
    pub fn set_mortgage_pool(env: Env, admin: Address, pool: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::MortgagePool) {
            panic_with_error!(&env, Error::AlreadySet);
        }
        env.storage().instance().set(&DataKey::MortgagePool, &pool);
    }

    /// Schedule an upgrade. It can execute only once the timelock set at
    /// deployment has elapsed, and can be cancelled at any time before that, so
    /// investors and the admin's other signers see the change coming.
    ///
    /// There is no admin path that moves capital: no sweep, no forced
    /// withdrawal, and no way to mint shares.
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

    // --- Investor operations ---

    /// Put capital into the pool.
    ///
    /// Shares are issued one-for-one with the settlement asset. The pool does
    /// not revalue shares as interest arrives — interest is tracked separately
    /// and claimed, which keeps a deposit's value in the asset it was made in
    /// and means a late depositor cannot buy into interest already earned.
    pub fn deposit(env: Env, investor: Address, amount: i128) {
        Self::extend_instance(&env);
        investor.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        Self::token(&env).transfer(&investor, &env.current_contract_address(), &amount);

        let acc = Self::acc(&env);
        let mut position = Self::position_of(&env, &investor);
        Self::settle(&mut position, acc);
        position.shares += amount;
        Self::reset_debt(&mut position, acc);
        Self::save_position(&env, &investor, &position);

        Self::add(&env, DataKey::TotalShares, amount);
        let capital = Self::add(&env, DataKey::TotalCapital, amount);

        env.events().publish(
            (POOL, symbol_short!("deposit")),
            (investor, amount, capital),
        );
    }

    /// Take capital out. Only what is not committed to an approved mortgage
    /// can be withdrawn; interest is claimed separately.
    pub fn withdraw(env: Env, investor: Address, amount: i128) {
        Self::extend_instance(&env);
        investor.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let acc = Self::acc(&env);
        let mut position = Self::position_of(&env, &investor);
        if position.shares < amount {
            panic_with_error!(&env, Error::InsufficientShares);
        }
        // Capital committed to a half-built house is not anyone's to withdraw.
        if amount > Self::available_of(&env) {
            panic_with_error!(&env, Error::InsufficientAvailable);
        }

        Self::settle(&mut position, acc);
        position.shares -= amount;
        Self::reset_debt(&mut position, acc);
        Self::save_position(&env, &investor, &position);

        Self::add(&env, DataKey::TotalShares, -amount);
        let capital = Self::add(&env, DataKey::TotalCapital, -amount);

        Self::token(&env).transfer(&env.current_contract_address(), &investor, &amount);

        env.events().publish(
            (POOL, symbol_short!("withdraw")),
            (investor, amount, capital),
        );
    }

    /// Collect interest earned and not yet taken.
    pub fn claim_interest(env: Env, investor: Address) -> i128 {
        Self::extend_instance(&env);
        investor.require_auth();

        let acc = Self::acc(&env);
        let mut position = Self::position_of(&env, &investor);
        Self::settle(&mut position, acc);
        let owed = position.credited;
        if owed <= 0 {
            panic_with_error!(&env, Error::NothingToClaim);
        }
        position.credited = 0;
        Self::save_position(&env, &investor, &position);

        Self::token(&env).transfer(&env.current_contract_address(), &investor, &owed);

        env.events()
            .publish((POOL, symbol_short!("interest")), (investor, owed));
        owed
    }

    // --- MortgagePool operations ---

    /// Commit capital to an approved mortgage. It stops being withdrawable
    /// immediately, though it is paid out only as milestones are signed off.
    pub fn reserve(env: Env, caller: Address, amount: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_pool(&env, &caller);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if amount > Self::available_of(&env) {
            panic_with_error!(&env, Error::InsufficientAvailable);
        }
        let reserved = Self::add(&env, DataKey::TotalReserved, amount);
        env.events()
            .publish((POOL, symbol_short!("reserve")), (amount, reserved));
    }

    /// Release a commitment without paying it out — a mortgage that was
    /// cancelled, or the undrawn remainder of one that defaulted part-built.
    pub fn unreserve(env: Env, caller: Address, amount: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_pool(&env, &caller);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if amount > Self::total(&env, DataKey::TotalReserved) {
            panic_with_error!(&env, Error::InsufficientReserved);
        }
        let reserved = Self::add(&env, DataKey::TotalReserved, -amount);
        env.events()
            .publish((POOL, symbol_short!("unreserve")), (amount, reserved));
    }

    /// Pay a tranche of committed capital to a recipient — in practice the
    /// trustee or builder drawing a construction stage.
    ///
    /// The amount must already be reserved, so the pool can never disburse
    /// money it has not set aside.
    pub fn disburse(env: Env, caller: Address, to: Address, amount: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_pool(&env, &caller);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if amount > Self::total(&env, DataKey::TotalReserved) {
            panic_with_error!(&env, Error::InsufficientReserved);
        }

        Self::add(&env, DataKey::TotalReserved, -amount);
        Self::add(&env, DataKey::TotalCapital, -amount);
        let lent = Self::add(&env, DataKey::TotalLent, amount);

        Self::token(&env).transfer(&env.current_contract_address(), &to, &amount);

        env.events()
            .publish((POOL, symbol_short!("disburse")), (to, amount, lent));
    }

    /// Take a repayment in: `principal` returns to the pool's capital,
    /// `interest` is shared out across investors.
    ///
    /// Split by the caller rather than here, because only the MortgagePool
    /// knows a loan's balance and rate. The pool's job is to bank it.
    pub fn repay(env: Env, caller: Address, from: Address, principal: i128, interest: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_pool(&env, &caller);
        if principal < 0 || interest < 0 || principal + interest <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        Self::token(&env).transfer(
            &from,
            &env.current_contract_address(),
            &(principal + interest),
        );

        if principal > 0 {
            Self::add(&env, DataKey::TotalCapital, principal);
            Self::add(&env, DataKey::TotalLent, -principal);
        }
        if interest > 0 {
            Self::accrue_interest(&env, interest);
            Self::add(&env, DataKey::TotalInterest, interest);
        }

        env.events()
            .publish((POOL, symbol_short!("repay")), (from, principal, interest));
    }

    /// Write off principal that will not come back.
    ///
    /// The loss falls on the pool's capital, which is what investors are
    /// exposed to. Interest already credited is untouched: it was earned.
    pub fn write_off(env: Env, caller: Address, principal: i128) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_pool(&env, &caller);
        if principal <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        Self::add(&env, DataKey::TotalLent, -principal);
        let total = Self::add(&env, DataKey::TotalWrittenOff, principal);

        env.events()
            .publish((POOL, symbol_short!("writeoff")), (principal, total));
    }

    // --- Getters ---

    /// Capital that is neither committed to a mortgage nor already lent out.
    pub fn available(env: Env) -> i128 {
        Self::available_of(&env)
    }

    pub fn shares_of(env: Env, investor: Address) -> i128 {
        Self::position_of(&env, &investor).shares
    }

    pub fn position_of_investor(env: Env, investor: Address) -> Position {
        Self::position_of(&env, &investor)
    }

    /// Interest `investor` could claim right now, without changing anything.
    pub fn claimable_interest(env: Env, investor: Address) -> i128 {
        let acc = Self::acc(&env);
        let mut position = Self::position_of(&env, &investor);
        Self::settle(&mut position, acc);
        position.credited
    }

    pub fn pool_state(env: Env) -> PoolState {
        PoolState {
            total_capital: Self::total(&env, DataKey::TotalCapital),
            total_reserved: Self::total(&env, DataKey::TotalReserved),
            total_lent: Self::total(&env, DataKey::TotalLent),
            total_interest: Self::total(&env, DataKey::TotalInterest),
            total_written_off: Self::total(&env, DataKey::TotalWrittenOff),
            total_shares: Self::total(&env, DataKey::TotalShares),
        }
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_mortgage_pool(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::MortgagePool)
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

    /// Spread interest across the shares outstanding right now.
    ///
    /// The remainder of the division is kept in scaled units and folded into
    /// the next payment, so truncation never compounds and the pool can always
    /// cover what it owes.
    fn accrue_interest(env: &Env, amount: i128) {
        let shares = Self::total(env, DataKey::TotalShares);
        if shares <= 0 {
            // Interest arriving with no investors would have nobody to belong
            // to. It stays in the pool's capital rather than being lost.
            Self::add(env, DataKey::TotalCapital, amount);
            return;
        }
        let scaled = amount * SCALE + Self::total(env, DataKey::Carry);
        Self::set(env, DataKey::Acc, Self::acc(env) + scaled / shares);
        Self::set(env, DataKey::Carry, scaled % shares);
    }

    /// Bank what the current shares have earned since the last settlement.
    /// Must run before any share change, paired with [`Self::reset_debt`].
    fn settle(position: &mut Position, acc: i128) {
        let entitled = position.shares * acc / SCALE;
        position.credited += entitled - position.reward_debt;
        position.reward_debt = entitled;
    }

    /// Re-anchor against a changed share count, so an investor is never paid
    /// for growth that happened before they held these shares.
    fn reset_debt(position: &mut Position, acc: i128) {
        position.reward_debt = position.shares * acc / SCALE;
    }

    fn position_of(env: &Env, investor: &Address) -> Position {
        let key = DataKey::Position(investor.clone());
        match env.storage().persistent().get::<DataKey, Position>(&key) {
            Some(position) => {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, THRESHOLD, EXTEND_TO);
                position
            }
            None => Position {
                shares: 0,
                reward_debt: 0,
                credited: 0,
            },
        }
    }

    fn save_position(env: &Env, investor: &Address, position: &Position) {
        let key = DataKey::Position(investor.clone());
        env.storage().persistent().set(&key, position);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn available_of(env: &Env) -> i128 {
        Self::total(env, DataKey::TotalCapital) - Self::total(env, DataKey::TotalReserved)
    }

    fn total(env: &Env, key: DataKey) -> i128 {
        env.storage().instance().get(&key).unwrap_or(0)
    }

    fn set(env: &Env, key: DataKey, value: i128) {
        env.storage().instance().set(&key, &value);
    }

    /// Adjust a running total and return the new value.
    fn add(env: &Env, key: DataKey, delta: i128) -> i128 {
        let updated = Self::total(env, key.clone()) + delta;
        Self::set(env, key, updated);
        updated
    }

    fn acc(env: &Env) -> i128 {
        Self::total(env, DataKey::Acc)
    }

    fn token(env: &Env) -> token::Client<'_> {
        let address: Address = env
            .storage()
            .instance()
            .get(&DataKey::SettlementToken)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        token::Client::new(env, &address)
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

    fn require_pool(env: &Env, caller: &Address) {
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::MortgagePool)
            .unwrap_or_else(|| panic_with_error!(env, Error::MortgagePoolNotSet));
        if *caller != pool {
            panic_with_error!(env, Error::NotAuthorized);
        }
    }
}

#[cfg(test)]
mod test;
