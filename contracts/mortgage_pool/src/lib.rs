#![no_std]
//! MortgagePool — the loan itself: what was borrowed, what has been released
//! against the build, and what is still owed.
//!
//! A diaspora borrower applies against a property a trustee has registered and
//! an oracle has valued. Once underwriting approves the application, the
//! principal is committed against the LendingPool's capital — but none of it
//! reaches anyone yet. It is released one construction stage at a time, and
//! only for a stage a registered inspector has signed off in the registry.
//! Money follows the building.
//!
//! This contract holds no funds. It decides what is owed and instructs the
//! LendingPool to move money; the LendingPool holds every cent and does not
//! know what a mortgage is. Neither contract alone can pay the wrong party.
//!
//! ## How interest is charged
//!
//! Interest accrues monthly on the principal actually outstanding, at the
//! annual rate fixed when the loan was approved. Principal amortises in equal
//! monthly instalments over the term — a constant-amortisation loan, where each
//! payment is the month's interest plus a fixed slice of principal, so the
//! payment falls as the balance does.
//!
//! This is deliberately **not** the level-payment annuity the front end
//! projects. An annuity needs `(1 + r)^n`, and there is no way to evaluate that
//! in integer arithmetic without either an approximation the borrower would
//! have to trust or a fixed-point exponent routine large enough to be its own
//! audit surface. Constant amortisation is exact in integers, so what the chain
//! charges can be checked by hand. The contract is the source of truth for what
//! is owed; any schedule rendered elsewhere is a projection.
//!
//! Interest is only ever charged on money that has actually been disbursed, so
//! a borrower whose build has stalled at the foundation is not paying for the
//! roofing tranche they have not received.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, Address, BytesN, Env, Symbol,
};

/// Ledgers per day at Stellar's 5-second ledger close time.
const DAY_IN_LEDGERS: u32 = 17_280;
/// Storage lifetimes, in ledgers. Entries are extended to about 120 days
/// whenever they fall below about 90, so they stay live while in use without
/// paying rent on every call. Both are well under the network's maximum entry
/// lifetime of about 180 days.
const EXTEND_TO: u32 = 120 * DAY_IN_LEDGERS;
const THRESHOLD: u32 = 90 * DAY_IN_LEDGERS;

/// Basis points in one whole.
const BPS_DENOMINATOR: i128 = 10_000;
/// Months in a year, for converting an annual rate to a monthly one.
const MONTHS_PER_YEAR: i128 = 12;
/// A billing month, fixed at 30 days.
///
/// Calendar months would need date arithmetic the contract has no way to do,
/// and a fixed period means the borrower can compute the next charge without
/// knowing which month it falls in. The small drift against the calendar is
/// disclosed rather than hidden.
const SECONDS_PER_MONTH: u64 = 30 * 24 * 60 * 60;

/// Construction stages. Must match the registry's `MILESTONE_COUNT`: the pool
/// divides the principal into this many tranches, and asks the registry whether
/// each stage is signed off.
pub const MILESTONE_COUNT: u32 = 5;

/// The most that may be lent against a property's valuation, in basis points.
/// A hard ceiling in code rather than configuration, so raising it needs an
/// upgrade and its timelock.
const MAX_LTV_BPS: i128 = 8_000;

/// The highest annual rate the pool will write, in basis points.
const MAX_RATE_BPS: u32 = 3_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 2,
    NotAuthorized = 3,
    NoPendingAction = 5,
    ActionPending = 6,
    TimelockNotExpired = 7,
    UnknownMortgage = 8,
    WrongStatus = 9,
    InvalidAmount = 10,
    InvalidTerm = 11,
    InvalidRate = 12,
    PropertyNotVerified = 13,
    ExceedsLtv = 14,
    NotBorrower = 15,
    StageNotReleasable = 16,
    NothingToDisburse = 17,
    Underpaid = 18,
    NotInArrears = 19,
    PropertyHasMortgage = 20,
    InvalidGrace = 21,
}

/// Where a loan stands. The names match the backend's mortgage status so the
/// two never have to be translated.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MortgageStatus {
    /// Submitted by the borrower, not yet underwritten.
    Applied,
    /// Approved; principal committed against the pool, nothing released.
    Approved,
    /// At least one tranche has been released.
    Funded,
    /// Repayment has begun.
    Repaying,
    /// Cleared in full.
    PaidOff,
    /// Written off after arrears ran past the grace period.
    Defaulted,
}

/// One mortgage.
///
/// `outstanding` is principal actually disbursed and not yet repaid, which is
/// what interest is charged on. `principal` is the whole facility, most of
/// which may still be undrawn.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mortgage {
    pub id: u64,
    pub property_id: u64,
    pub borrower: Address,
    /// The full approved facility.
    pub principal: i128,
    pub term_months: u32,
    /// Annual interest rate in basis points, fixed at approval.
    pub rate_bps: u32,
    pub status: MortgageStatus,
    /// Released to the build so far.
    pub disbursed: i128,
    /// Disbursed principal not yet repaid; interest is charged on this.
    pub outstanding: i128,
    /// Interest charged and not yet paid.
    pub interest_accrued: i128,
    /// Sub-unit remainder from the last accrual, carried so that charging a
    /// month at a time and charging several at once come to the same figure.
    pub interest_carry: i128,
    pub total_repaid: i128,
    pub interest_paid: i128,
    pub payments_made: u32,
    /// When interest was last brought up to date.
    pub last_accrued_at: u64,
    /// When the next instalment falls due. Zero until the first disbursement.
    pub next_payment_due: u64,
    pub created_at: u64,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    Registry,
    LendingPool,
    /// How long after a missed instalment before anyone may default the loan.
    GraceSecs,
    NextId,
    Underwriter(Address),
    Mortgage(u64),
    /// The live mortgage against a property, so one cannot be financed twice.
    PropertyMortgage(u64),
}

/// A sensitive admin change that must wait out the timelock before it runs.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Replace the contract's code, keeping its address and storage.
    Upgrade(BytesN<32>),
    /// Change how long arrears are tolerated before default.
    SetGraceSecs(u64),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledAction {
    pub action: Action,
    /// Earliest ledger time at which the action may execute.
    pub eta: u64,
}

/// The slice of the PropertyRegistry this contract calls.
///
/// Declared as a client rather than a dependency so the registry's code is not
/// linked into this contract's wasm.
#[contractclient(name = "RegistryClient")]
pub trait Registry {
    /// Returns `(trustee, usdc_value, is_verified)`.
    fn lending_terms(env: Env, property_id: u64) -> (Address, i128, bool);
    fn is_releasable(env: Env, property_id: u64, stage: u32) -> bool;
    fn mark_released(env: Env, caller: Address, property_id: u64, stage: u32);
    fn mark_mortgaged(env: Env, caller: Address, property_id: u64);
    fn mark_repaid(env: Env, caller: Address, property_id: u64);
    fn mark_defaulted(env: Env, caller: Address, property_id: u64);
}

/// The slice of the LendingPool this contract calls.
#[contractclient(name = "PoolClient")]
pub trait Pool {
    fn reserve(env: Env, caller: Address, amount: i128);
    fn unreserve(env: Env, caller: Address, amount: i128);
    fn disburse(env: Env, caller: Address, to: Address, amount: i128);
    fn repay(env: Env, caller: Address, from: Address, principal: i128, interest: i128);
    fn write_off(env: Env, caller: Address, principal: i128);
}

const MORTGAGE: Symbol = symbol_short!("mortgage");

#[contract]
pub struct MortgagePoolContract;

#[contractimpl]
impl MortgagePoolContract {
    /// Runs once, atomically, as part of the deploy transaction. There is no
    /// separate initialize call for anyone to front-run between deployment and
    /// setup, so nobody else can claim the admin role.
    pub fn __constructor(
        env: Env,
        admin: Address,
        registry: Address,
        lending_pool: Address,
        grace_secs: u64,
        timelock_secs: u64,
    ) {
        Self::extend_instance(&env);
        let storage = env.storage().instance();
        storage.set(&DataKey::Admin, &admin);
        storage.set(&DataKey::Registry, &registry);
        storage.set(&DataKey::LendingPool, &lending_pool);
        storage.set(&DataKey::GraceSecs, &grace_secs);
        storage.set(&DataKey::TimelockSecs, &timelock_secs);
        storage.set(&DataKey::NextId, &1u64);
    }

    // --- Administration ---

    /// Authorize or revoke an underwriter — the party that approves or declines
    /// applications. Revocation takes effect on the next invocation.
    pub fn set_underwriter(env: Env, admin: Address, underwriter: Address, authorized: bool) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        let key = DataKey::Underwriter(underwriter.clone());
        if authorized {
            env.storage().persistent().set(&key, &true);
            env.storage()
                .persistent()
                .extend_ttl(&key, THRESHOLD, EXTEND_TO);
        } else {
            env.storage().persistent().remove(&key);
        }
        env.events().publish(
            (MORTGAGE, symbol_short!("undrwrtr")),
            (underwriter, authorized),
        );
    }

    /// Schedule an upgrade or a change to the grace period. Each can execute
    /// only once the timelock set at deployment has elapsed, and can be
    /// cancelled at any time before that, so borrowers and investors see every
    /// such change coming.
    pub fn schedule_action(env: Env, admin: Address, action: Action) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::Scheduled) {
            panic_with_error!(&env, Error::ActionPending);
        }
        // A zero grace period would let a loan default the instant an
        // instalment is late, so it is refused on the way into the queue as
        // well as on the way out.
        if let Action::SetGraceSecs(secs) = action {
            if secs == 0 {
                panic_with_error!(&env, Error::InvalidGrace);
            }
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
            Action::SetGraceSecs(secs) => {
                if secs == 0 {
                    panic_with_error!(&env, Error::InvalidGrace);
                }
                env.storage().instance().set(&DataKey::GraceSecs, &secs)
            }
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

    // --- Borrower operations ---

    /// Apply for a mortgage against a verified property. Returns its id.
    ///
    /// Nothing is committed here; the application is a record until an
    /// underwriter approves it.
    pub fn apply(
        env: Env,
        borrower: Address,
        property_id: u64,
        principal: i128,
        term_months: u32,
        rate_bps: u32,
    ) -> u64 {
        Self::extend_instance(&env);
        borrower.require_auth();
        if principal <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if term_months == 0 {
            panic_with_error!(&env, Error::InvalidTerm);
        }
        if rate_bps > MAX_RATE_BPS {
            panic_with_error!(&env, Error::InvalidRate);
        }
        // One live mortgage per property. A second facility against the same
        // collateral would let the same house back two loans.
        if env
            .storage()
            .persistent()
            .has(&DataKey::PropertyMortgage(property_id))
        {
            panic_with_error!(&env, Error::PropertyHasMortgage);
        }

        let (_trustee, valuation, verified) = Self::registry(&env).lending_terms(&property_id);
        if !verified {
            panic_with_error!(&env, Error::PropertyNotVerified);
        }
        // The valuation is what the loan is secured against, so the ceiling is
        // checked at application rather than only at approval.
        if principal * BPS_DENOMINATOR > valuation * MAX_LTV_BPS {
            panic_with_error!(&env, Error::ExceedsLtv);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let now = env.ledger().timestamp();
        let mortgage = Mortgage {
            id,
            property_id,
            borrower: borrower.clone(),
            principal,
            term_months,
            rate_bps,
            status: MortgageStatus::Applied,
            disbursed: 0,
            outstanding: 0,
            interest_accrued: 0,
            interest_carry: 0,
            total_repaid: 0,
            interest_paid: 0,
            payments_made: 0,
            last_accrued_at: 0,
            next_payment_due: 0,
            created_at: now,
        };
        Self::save(&env, &mortgage);
        Self::set_property_mortgage(&env, property_id, id);

        env.events().publish(
            (MORTGAGE, symbol_short!("applied")),
            (id, property_id, borrower, principal),
        );
        id
    }

    /// Pay an instalment.
    ///
    /// Interest is brought up to date first, then the payment clears accrued
    /// interest before it touches principal — so a borrower who pays only the
    /// interest never sees their balance grow, and one who pays more always
    /// reduces what they owe.
    ///
    /// The payment must cover at least the instalment currently due. Anything
    /// above that goes straight to principal and shortens the loan.
    pub fn repay(env: Env, mortgage_id: u64, amount: i128) {
        Self::extend_instance(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        mortgage.borrower.require_auth();
        if mortgage.status != MortgageStatus::Funded && mortgage.status != MortgageStatus::Repaying
        {
            panic_with_error!(&env, Error::WrongStatus);
        }

        Self::accrue(&env, &mut mortgage);

        let payoff = mortgage.outstanding + mortgage.interest_accrued;
        let due = Self::instalment_due(&mortgage);
        // Either settle the loan outright or cover the instalment; anything
        // between is refused so a part payment cannot quietly stall the
        // schedule while looking like a payment was made.
        if amount < due && amount < payoff {
            panic_with_error!(&env, Error::Underpaid);
        }
        let amount = if amount > payoff { payoff } else { amount };

        let interest_part = if amount >= mortgage.interest_accrued {
            mortgage.interest_accrued
        } else {
            amount
        };
        let principal_part = amount - interest_part;

        mortgage.interest_accrued -= interest_part;
        mortgage.outstanding -= principal_part;
        mortgage.total_repaid += amount;
        mortgage.interest_paid += interest_part;
        mortgage.payments_made += 1;
        mortgage.next_payment_due += SECONDS_PER_MONTH;
        mortgage.status = MortgageStatus::Repaying;

        Self::pool(&env).repay(
            &env.current_contract_address(),
            &mortgage.borrower,
            &principal_part,
            &interest_part,
        );

        // Cleared in full. Any facility never drawn is released back to the
        // pool so it can fund somebody else's build.
        if mortgage.outstanding == 0 && mortgage.interest_accrued == 0 {
            let undrawn = mortgage.principal - mortgage.disbursed;
            if undrawn > 0 {
                Self::pool(&env).unreserve(&env.current_contract_address(), &undrawn);
            }
            mortgage.status = MortgageStatus::PaidOff;
            Self::clear_property_mortgage(&env, mortgage.property_id);
            Self::registry(&env)
                .mark_repaid(&env.current_contract_address(), &mortgage.property_id);
        }

        Self::save(&env, &mortgage);
        env.events().publish(
            (MORTGAGE, symbol_short!("repaid")),
            (mortgage_id, amount, principal_part, interest_part),
        );
    }

    // --- Underwriter operations ---

    /// Approve an application and commit its principal against the pool.
    ///
    /// The capital stops being withdrawable by investors from this moment, even
    /// though it will be released to the build over months. A borrower whose
    /// foundation passes inspection should not find the money gone.
    pub fn approve(env: Env, underwriter: Address, mortgage_id: u64) {
        Self::extend_instance(&env);
        Self::require_underwriter(&env, &underwriter);
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        if mortgage.status != MortgageStatus::Applied {
            panic_with_error!(&env, Error::WrongStatus);
        }
        // Re-checked at approval: a valuation can be revised between applying
        // and underwriting, and the commitment is made against today's number.
        let (_trustee, valuation, verified) =
            Self::registry(&env).lending_terms(&mortgage.property_id);
        if !verified {
            panic_with_error!(&env, Error::PropertyNotVerified);
        }
        if mortgage.principal * BPS_DENOMINATOR > valuation * MAX_LTV_BPS {
            panic_with_error!(&env, Error::ExceedsLtv);
        }

        Self::pool(&env).reserve(&env.current_contract_address(), &mortgage.principal);

        mortgage.status = MortgageStatus::Approved;
        Self::save(&env, &mortgage);

        env.events().publish(
            (MORTGAGE, symbol_short!("approved")),
            (mortgage_id, underwriter, mortgage.principal),
        );
    }

    /// Decline an application before it is approved, freeing the property for
    /// another attempt.
    pub fn decline(env: Env, underwriter: Address, mortgage_id: u64) {
        Self::extend_instance(&env);
        Self::require_underwriter(&env, &underwriter);
        let mortgage = Self::mortgage_of(&env, mortgage_id);
        if mortgage.status != MortgageStatus::Applied {
            panic_with_error!(&env, Error::WrongStatus);
        }
        // Nothing was reserved, so there is nothing to give back.
        Self::clear_property_mortgage(&env, mortgage.property_id);
        env.storage()
            .persistent()
            .remove(&DataKey::Mortgage(mortgage_id));

        env.events().publish(
            (MORTGAGE, symbol_short!("declined")),
            (mortgage_id, underwriter),
        );
    }

    // --- Milestone-gated disbursement ---

    /// Release the tranche for a construction stage.
    ///
    /// Permissionless: the registry decides whether the stage is signed off and
    /// unpaid, and the tranche size is fixed by the loan, so there is nothing
    /// for a caller to influence. A trustee should not have to wait on the
    /// platform to press a button once the inspector has signed.
    ///
    /// The money goes to the property's trustee, never to the borrower — the
    /// point of staged release is that the funds reach the build.
    pub fn disburse(env: Env, mortgage_id: u64, stage: u32) -> i128 {
        Self::extend_instance(&env);
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        if mortgage.status != MortgageStatus::Approved && mortgage.status != MortgageStatus::Funded
        {
            panic_with_error!(&env, Error::WrongStatus);
        }

        let registry = Self::registry(&env);
        if !registry.is_releasable(&mortgage.property_id, &stage) {
            panic_with_error!(&env, Error::StageNotReleasable);
        }

        let tranche = Self::tranche_for(&mortgage, stage);
        if tranche <= 0 {
            panic_with_error!(&env, Error::NothingToDisburse);
        }

        // Interest is brought up to date before the balance grows, so the new
        // tranche is not charged for time it was not outstanding.
        Self::accrue(&env, &mut mortgage);

        let (trustee, _valuation, _verified) = registry.lending_terms(&mortgage.property_id);
        let this = env.current_contract_address();

        // Marked released before the money moves, so a re-entrant registry
        // cannot be used to draw the same stage twice.
        registry.mark_released(&this, &mortgage.property_id, &stage);
        Self::pool(&env).disburse(&this, &trustee, &tranche);

        let first_draw = mortgage.disbursed == 0;
        mortgage.disbursed += tranche;
        mortgage.outstanding += tranche;

        if first_draw {
            let now = env.ledger().timestamp();
            mortgage.status = MortgageStatus::Funded;
            mortgage.last_accrued_at = now;
            mortgage.next_payment_due = now + SECONDS_PER_MONTH;
            registry.mark_mortgaged(&this, &mortgage.property_id);
        }

        Self::save(&env, &mortgage);
        env.events().publish(
            (MORTGAGE, symbol_short!("disbursd")),
            (mortgage_id, stage, tranche, trustee),
        );
        tranche
    }

    // --- Default ---

    /// Write off a loan whose arrears have run past the grace period.
    ///
    /// Permissionless, and what it does is fixed by the loan's own state, so
    /// the platform cannot keep a bad loan off the books by declining to act on
    /// it. The undrawn facility goes back to the pool; the disbursed balance is
    /// written off against investors' capital, which is the risk they took.
    pub fn mark_default(env: Env, mortgage_id: u64) {
        Self::extend_instance(&env);
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        if mortgage.status != MortgageStatus::Funded && mortgage.status != MortgageStatus::Repaying
        {
            panic_with_error!(&env, Error::WrongStatus);
        }
        if !Self::is_in_default(&env, &mortgage) {
            panic_with_error!(&env, Error::NotInArrears);
        }

        Self::accrue(&env, &mut mortgage);
        let this = env.current_contract_address();
        let pool = Self::pool(&env);

        let undrawn = mortgage.principal - mortgage.disbursed;
        if undrawn > 0 {
            pool.unreserve(&this, &undrawn);
        }
        if mortgage.outstanding > 0 {
            pool.write_off(&this, &mortgage.outstanding);
        }

        mortgage.status = MortgageStatus::Defaulted;
        Self::clear_property_mortgage(&env, mortgage.property_id);
        Self::registry(&env).mark_defaulted(&this, &mortgage.property_id);
        Self::save(&env, &mortgage);

        env.events().publish(
            (MORTGAGE, symbol_short!("default")),
            (mortgage_id, mortgage.outstanding, undrawn),
        );
    }

    // --- Getters ---

    pub fn get_mortgage(env: Env, mortgage_id: u64) -> Mortgage {
        Self::mortgage_of(&env, mortgage_id)
    }

    /// The loan as it stands right now, with interest brought up to date.
    /// A view: it computes the accrual without writing it.
    pub fn current_balance(env: Env, mortgage_id: u64) -> (i128, i128) {
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        Self::accrue(&env, &mut mortgage);
        (mortgage.outstanding, mortgage.interest_accrued)
    }

    /// What must be paid to keep the loan current: interest owed plus this
    /// period's slice of principal.
    pub fn amount_due(env: Env, mortgage_id: u64) -> i128 {
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        Self::accrue(&env, &mut mortgage);
        let due = Self::instalment_due(&mortgage);
        let payoff = mortgage.outstanding + mortgage.interest_accrued;
        if due > payoff {
            payoff
        } else {
            due
        }
    }

    /// What it would cost to clear the loan outright today.
    pub fn payoff_amount(env: Env, mortgage_id: u64) -> i128 {
        let mut mortgage = Self::mortgage_of(&env, mortgage_id);
        Self::accrue(&env, &mut mortgage);
        mortgage.outstanding + mortgage.interest_accrued
    }

    /// The tranche a given construction stage releases.
    pub fn tranche_amount(env: Env, mortgage_id: u64, stage: u32) -> i128 {
        Self::tranche_for(&Self::mortgage_of(&env, mortgage_id), stage)
    }

    /// Whether [`Self::mark_default`] would succeed right now.
    pub fn is_defaultable(env: Env, mortgage_id: u64) -> bool {
        let mortgage = Self::mortgage_of(&env, mortgage_id);
        (mortgage.status == MortgageStatus::Funded || mortgage.status == MortgageStatus::Repaying)
            && Self::is_in_default(&env, &mortgage)
    }

    pub fn mortgage_for_property(env: Env, property_id: u64) -> Option<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::PropertyMortgage(property_id))
    }

    pub fn is_underwriter(env: Env, underwriter: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Underwriter(underwriter))
            .unwrap_or(false)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_registry(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Registry).unwrap()
    }

    pub fn get_lending_pool(env: Env) -> Address {
        env.storage().instance().get(&DataKey::LendingPool).unwrap()
    }

    pub fn get_grace_secs(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::GraceSecs)
            .unwrap_or(0)
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

    pub fn get_max_ltv_bps(_env: Env) -> i128 {
        MAX_LTV_BPS
    }

    pub fn get_max_rate_bps(_env: Env) -> u32 {
        MAX_RATE_BPS
    }

    pub fn get_seconds_per_month(_env: Env) -> u64 {
        SECONDS_PER_MONTH
    }

    // --- Internals ---

    /// Keep the contract instance, and with it the configuration and the
    /// contract code, from being archived while the protocol is in use.
    fn extend_instance(env: &Env) {
        env.storage().instance().extend_ttl(THRESHOLD, EXTEND_TO);
    }

    /// Charge interest for every whole month that has passed since the last
    /// accrual, on the balance outstanding.
    ///
    /// Only whole months are charged, and `last_accrued_at` advances by exactly
    /// the months charged — never to `now` — so no part-month is ever lost or
    /// double-charged however often this is called.
    ///
    /// The division's remainder is carried rather than dropped. Without it the
    /// interest would depend on how often accrual happened to be triggered: a
    /// borrower who called the contract every month would round down twelve
    /// times a year, while one who left it alone would round down once, and the
    /// first would pay less for the same debt. With the carry, any sequence of
    /// accruals over the same period comes to exactly the same figure.
    fn accrue(env: &Env, mortgage: &mut Mortgage) {
        if mortgage.last_accrued_at == 0 || mortgage.outstanding <= 0 {
            return;
        }
        let now = env.ledger().timestamp();
        if now <= mortgage.last_accrued_at {
            return;
        }
        let months = (now - mortgage.last_accrued_at) / SECONDS_PER_MONTH;
        if months == 0 {
            return;
        }
        let divisor = BPS_DENOMINATOR * MONTHS_PER_YEAR;
        let numerator = mortgage.outstanding * mortgage.rate_bps as i128 * months as i128
            + mortgage.interest_carry;
        mortgage.interest_accrued += numerator / divisor;
        mortgage.interest_carry = numerator % divisor;
        mortgage.last_accrued_at += months * SECONDS_PER_MONTH;
    }

    /// This period's instalment: interest owed plus a fixed slice of principal.
    ///
    /// The slice is the whole facility divided by the term, so the loan clears
    /// on schedule even though the balance is drawn down in stages. The final
    /// instalment is whatever is left.
    fn instalment_due(mortgage: &Mortgage) -> i128 {
        let slice = mortgage.principal / mortgage.term_months as i128;
        let principal_part = if slice > mortgage.outstanding {
            mortgage.outstanding
        } else {
            slice
        };
        mortgage.interest_accrued + principal_part
    }

    /// The principal released at a construction stage: an equal share of the
    /// facility, with the last stage taking the rounding remainder so the whole
    /// facility is drawable.
    fn tranche_for(mortgage: &Mortgage, stage: u32) -> i128 {
        let each = mortgage.principal / MILESTONE_COUNT as i128;
        if stage == MILESTONE_COUNT - 1 {
            mortgage.principal - each * (MILESTONE_COUNT as i128 - 1)
        } else {
            each
        }
    }

    /// A loan is in default once an instalment has been due, unpaid, for longer
    /// than the grace period.
    fn is_in_default(env: &Env, mortgage: &Mortgage) -> bool {
        if mortgage.next_payment_due == 0 {
            return false;
        }
        let grace: u64 = env
            .storage()
            .instance()
            .get(&DataKey::GraceSecs)
            .unwrap_or(0);
        env.ledger().timestamp() > mortgage.next_payment_due + grace
    }

    fn mortgage_of(env: &Env, mortgage_id: u64) -> Mortgage {
        let key = DataKey::Mortgage(mortgage_id);
        let mortgage: Mortgage = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(env, Error::UnknownMortgage));
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
        mortgage
    }

    /// Persist a mortgage and extend its lifetime. Every mutation goes through
    /// here, so a live loan is renewed each time it is touched.
    fn save(env: &Env, mortgage: &Mortgage) {
        let key = DataKey::Mortgage(mortgage.id);
        env.storage().persistent().set(&key, mortgage);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn set_property_mortgage(env: &Env, property_id: u64, mortgage_id: u64) {
        let key = DataKey::PropertyMortgage(property_id);
        env.storage().persistent().set(&key, &mortgage_id);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    /// Free a property once its loan is closed, so it can be financed again.
    fn clear_property_mortgage(env: &Env, property_id: u64) {
        env.storage()
            .persistent()
            .remove(&DataKey::PropertyMortgage(property_id));
    }

    fn registry(env: &Env) -> RegistryClient<'_> {
        let address: Address = env
            .storage()
            .instance()
            .get(&DataKey::Registry)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        RegistryClient::new(env, &address)
    }

    fn pool(env: &Env) -> PoolClient<'_> {
        let address: Address = env
            .storage()
            .instance()
            .get(&DataKey::LendingPool)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        PoolClient::new(env, &address)
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

    fn require_underwriter(env: &Env, underwriter: &Address) {
        underwriter.require_auth();
        let key = DataKey::Underwriter(underwriter.clone());
        if !env.storage().persistent().get(&key).unwrap_or(false) {
            panic_with_error!(env, Error::NotAuthorized);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }
}

#[cfg(test)]
mod test;
