#![no_std]
//! Offering — the primary sale of a property's shares.
//!
//! Investors subscribe by paying the settlement asset into escrow. Nothing is
//! issued and nothing reaches the sponsor while the sale is open: the money
//! sits in this contract until the sale closes one way or the other.
//!
//! A sale closes when it sells out, or when its deadline passes. If the shares
//! subscribed reach the soft cap the sponsor set, the raise settles — the
//! sponsor is paid, the protocol fee goes to the treasury, and the registry
//! records the property as owned by its shareholders. If it does not, the raise
//! fails and every subscriber can take their money back in full.
//!
//! Closing is permissionless. Anyone may call it once the conditions are met,
//! and what it does is fixed entirely by the sale's own state, so a sponsor
//! cannot strand subscribers by declining to close a raise that failed.
//!
//! Shares and refunds are pulled, not pushed. A contract cannot loop over every
//! subscriber to pay them, so each investor claims their own shares after a
//! settlement or their own refund after a failure.

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

/// Basis points in one whole.
const BPS_DENOMINATOR: i128 = 10_000;
/// The most the protocol may ever take from a raise, whatever the admin sets.
/// Hard-coded rather than stored, so raising it needs a code upgrade and the
/// upgrade timelock, not a parameter change.
const MAX_FEE_BPS: u32 = 1_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 2,
    NotAuthorized = 3,
    NoPendingAction = 5,
    ActionPending = 6,
    TimelockNotExpired = 7,
    UnknownSale = 8,
    SaleExists = 9,
    SaleNotOpen = 10,
    SaleNotSettled = 11,
    SaleNotFailed = 12,
    Deadline = 13,
    StillOpen = 14,
    InvalidAmount = 15,
    InvalidSoftCap = 16,
    InvalidDuration = 17,
    InvalidFee = 18,
    Oversubscribed = 19,
    NotSponsor = 20,
    PropertyNotOffered = 21,
    NothingSubscribed = 22,
}

/// Where a sale stands. `Open` is the only state money can enter or leave in;
/// the other two are terminal and decide which way it leaves.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SaleStatus {
    Open,
    Settled,
    Failed,
}

/// One property's primary sale.
///
/// `total_shares` and `price_per_share` are copied from the registry when the
/// sale opens, so the terms an investor subscribes on cannot move under them
/// afterwards.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sale {
    pub property: u64,
    pub sponsor: Address,
    pub total_shares: i128,
    pub price_per_share: i128,
    /// The soft cap: below this the raise is unwound and everyone is refunded.
    pub min_shares: i128,
    pub subscribed: i128,
    pub deadline: u64,
    pub status: SaleStatus,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    Registry,
    ShareLedger,
    SettlementToken,
    Treasury,
    FeeBps,
    Sale(u64),
    Subscription(u64, Address),
}

/// A sensitive admin change that must wait out the timelock before it runs.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Replace the contract's code, keeping its address and storage.
    Upgrade(BytesN<32>),
    /// Change where the protocol fee is sent.
    SetTreasury(Address),
    /// Change the protocol fee, in basis points of a settled raise.
    SetFeeBps(u32),
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
    /// Returns `(sponsor, total_shares, price_per_share, is_offering)` — the
    /// terms of a sale and whether the property is cleared to be sold.
    fn offering_terms(env: Env, property_id: u64) -> (Address, i128, i128, bool);
    fn mark_owned(env: Env, caller: Address, property_id: u64);
    fn mark_failed(env: Env, caller: Address, property_id: u64);
}

/// The slice of the ShareLedger this contract calls.
#[contractclient(name = "LedgerClient")]
pub trait Ledger {
    fn issue(env: Env, caller: Address, property: u64, to: Address, shares: i128);
}

const OFFERING: Symbol = symbol_short!("offering");

#[contract]
pub struct OfferingContract;

#[contractimpl]
impl OfferingContract {
    /// Runs once, atomically, as part of the deploy transaction. There is no
    /// separate initialize call for anyone to front-run between deployment and
    /// setup, so nobody else can claim the admin role.
    pub fn __constructor(
        env: Env,
        admin: Address,
        registry: Address,
        share_ledger: Address,
        settlement_token: Address,
        treasury: Address,
        fee_bps: u32,
        timelock_secs: u64,
    ) {
        if fee_bps > MAX_FEE_BPS {
            panic_with_error!(&env, Error::InvalidFee);
        }
        Self::extend_instance(&env);
        let storage = env.storage().instance();
        storage.set(&DataKey::Admin, &admin);
        storage.set(&DataKey::Registry, &registry);
        storage.set(&DataKey::ShareLedger, &share_ledger);
        storage.set(&DataKey::SettlementToken, &settlement_token);
        storage.set(&DataKey::Treasury, &treasury);
        storage.set(&DataKey::FeeBps, &fee_bps);
        storage.set(&DataKey::TimelockSecs, &timelock_secs);
    }

    // --- Administration ---

    /// Schedule an upgrade, a change of treasury or a change of fee. Each can
    /// execute only once the timelock set at deployment has elapsed, and can be
    /// cancelled at any time before that, so investors and the admin's other
    /// signers see every such change coming.
    pub fn schedule_action(env: Env, admin: Address, action: Action) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::Scheduled) {
            panic_with_error!(&env, Error::ActionPending);
        }
        // Checked on the way in as well as on the way out, so an impossible fee
        // is rejected at once rather than sitting in the queue for two days.
        if let Action::SetFeeBps(bps) = action {
            if bps > MAX_FEE_BPS {
                panic_with_error!(&env, Error::InvalidFee);
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
            Action::SetTreasury(treasury) => {
                env.storage().instance().set(&DataKey::Treasury, &treasury)
            }
            Action::SetFeeBps(bps) => {
                if bps > MAX_FEE_BPS {
                    panic_with_error!(&env, Error::InvalidFee);
                }
                env.storage().instance().set(&DataKey::FeeBps, &bps)
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

    // --- Sponsor operations ---

    /// Start selling a property the registry has cleared for offering.
    ///
    /// The terms are copied from the registry here and never re-read, so an
    /// investor who subscribes is buying on the terms they saw.
    pub fn open(
        env: Env,
        sponsor: Address,
        property: u64,
        min_shares: i128,
        duration_secs: u64,
    ) -> Sale {
        Self::extend_instance(&env);
        sponsor.require_auth();
        if duration_secs == 0 {
            panic_with_error!(&env, Error::InvalidDuration);
        }
        // One sale per property, ever. A failed raise is re-registered as a new
        // property rather than retried under the same id, so subscribers to the
        // first raise can never be mixed with subscribers to a second.
        if env.storage().persistent().has(&DataKey::Sale(property)) {
            panic_with_error!(&env, Error::SaleExists);
        }

        let (registered_sponsor, total_shares, price_per_share, is_offering) =
            Self::registry(&env).offering_terms(&property);
        if !is_offering {
            panic_with_error!(&env, Error::PropertyNotOffered);
        }
        if registered_sponsor != sponsor {
            panic_with_error!(&env, Error::NotSponsor);
        }
        if min_shares <= 0 || min_shares > total_shares {
            panic_with_error!(&env, Error::InvalidSoftCap);
        }

        let sale = Sale {
            property,
            sponsor: sponsor.clone(),
            total_shares,
            price_per_share,
            min_shares,
            subscribed: 0,
            deadline: env.ledger().timestamp() + duration_secs,
            status: SaleStatus::Open,
        };
        Self::save_sale(&env, &sale);
        env.events().publish(
            (OFFERING, symbol_short!("open")),
            (property, sponsor, min_shares, sale.deadline),
        );
        sale
    }

    // --- Investor operations ---

    /// Subscribe for `shares`, paying `shares * price_per_share` into escrow.
    pub fn subscribe(env: Env, property: u64, investor: Address, shares: i128) {
        Self::extend_instance(&env);
        investor.require_auth();
        if shares <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let mut sale = Self::sale_of(&env, property);
        Self::require_open(&env, &sale);
        if env.ledger().timestamp() >= sale.deadline {
            panic_with_error!(&env, Error::Deadline);
        }
        if sale.subscribed + shares > sale.total_shares {
            panic_with_error!(&env, Error::Oversubscribed);
        }

        let cost = shares * sale.price_per_share;
        Self::token(&env).transfer(&investor, &env.current_contract_address(), &cost);

        sale.subscribed += shares;
        Self::save_sale(&env, &sale);
        Self::set_subscription(
            &env,
            property,
            &investor,
            Self::subscription_of(&env, property, &investor) + shares,
        );

        env.events().publish(
            (OFFERING, symbol_short!("subscrib")),
            (property, investor, shares, cost),
        );
    }

    /// Take back some or all of a subscription while the sale is still open.
    ///
    /// An investor is not locked in by a raise that has not closed: the money
    /// is still theirs until it settles.
    pub fn withdraw_subscription(env: Env, property: u64, investor: Address, shares: i128) {
        Self::extend_instance(&env);
        investor.require_auth();
        if shares <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let mut sale = Self::sale_of(&env, property);
        Self::require_open(&env, &sale);
        // Once the deadline passes the sale is decided by its subscriptions;
        // letting someone pull out then could tip a settling raise into failure
        // after the fact.
        if env.ledger().timestamp() >= sale.deadline {
            panic_with_error!(&env, Error::Deadline);
        }

        let held = Self::subscription_of(&env, property, &investor);
        if shares > held {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        sale.subscribed -= shares;
        Self::save_sale(&env, &sale);
        Self::set_subscription(&env, property, &investor, held - shares);

        let refund = shares * sale.price_per_share;
        Self::token(&env).transfer(&env.current_contract_address(), &investor, &refund);

        env.events().publish(
            (OFFERING, symbol_short!("unsubscr")),
            (property, investor, shares, refund),
        );
    }

    /// Take delivery of shares from a settled sale.
    pub fn claim_shares(env: Env, property: u64, investor: Address) -> i128 {
        Self::extend_instance(&env);
        let sale = Self::sale_of(&env, property);
        if sale.status != SaleStatus::Settled {
            panic_with_error!(&env, Error::SaleNotSettled);
        }

        let shares = Self::subscription_of(&env, property, &investor);
        if shares <= 0 {
            panic_with_error!(&env, Error::NothingSubscribed);
        }
        // Zeroed before the issue call, so a re-entrant ledger cannot be used
        // to claim the same subscription twice.
        Self::set_subscription(&env, property, &investor, 0);
        Self::share_ledger(&env).issue(
            &env.current_contract_address(),
            &property,
            &investor,
            &shares,
        );

        env.events().publish(
            (OFFERING, symbol_short!("claimed")),
            (property, investor, shares),
        );
        shares
    }

    /// Take back the full subscription from a failed sale.
    pub fn refund(env: Env, property: u64, investor: Address) -> i128 {
        Self::extend_instance(&env);
        let sale = Self::sale_of(&env, property);
        if sale.status != SaleStatus::Failed {
            panic_with_error!(&env, Error::SaleNotFailed);
        }

        let shares = Self::subscription_of(&env, property, &investor);
        if shares <= 0 {
            panic_with_error!(&env, Error::NothingSubscribed);
        }
        Self::set_subscription(&env, property, &investor, 0);

        let amount = shares * sale.price_per_share;
        Self::token(&env).transfer(&env.current_contract_address(), &investor, &amount);

        env.events().publish(
            (OFFERING, symbol_short!("refund")),
            (property, investor, amount),
        );
        amount
    }

    // --- Permissionless close ---

    /// Close a sale and settle or unwind it. Anyone may call this; the outcome
    /// is decided entirely by the sale's own state.
    ///
    /// A sale that has sold out closes immediately. Otherwise it closes when its
    /// deadline has passed: at or above the soft cap it settles and the sponsor
    /// is paid for the shares actually sold; below it, the raise is unwound and
    /// every subscriber can take their money back.
    pub fn close(env: Env, property: u64) -> SaleStatus {
        Self::extend_instance(&env);
        let mut sale = Self::sale_of(&env, property);
        Self::require_open(&env, &sale);

        let sold_out = sale.subscribed == sale.total_shares;
        if !sold_out && env.ledger().timestamp() < sale.deadline {
            panic_with_error!(&env, Error::StillOpen);
        }

        let registry = Self::registry(&env);
        let this = env.current_contract_address();

        if sale.subscribed >= sale.min_shares {
            sale.status = SaleStatus::Settled;
            Self::save_sale(&env, &sale);

            // Only the shares actually sold are paid for and, later, issued.
            // Anything unsold is simply never issued, so income divides over
            // the shares that exist rather than the shares that were planned.
            let proceeds = sale.subscribed * sale.price_per_share;
            let fee = proceeds * Self::fee_bps(&env) as i128 / BPS_DENOMINATOR;
            let token = Self::token(&env);
            if fee > 0 {
                token.transfer(&this, &Self::treasury(&env), &fee);
            }
            token.transfer(&this, &sale.sponsor, &(proceeds - fee));

            registry.mark_owned(&this, &property);
        } else {
            sale.status = SaleStatus::Failed;
            Self::save_sale(&env, &sale);
            registry.mark_failed(&this, &property);
        }

        env.events().publish(
            (OFFERING, symbol_short!("close")),
            (property, sale.status, sale.subscribed),
        );
        sale.status
    }

    // --- Getters ---

    pub fn get_sale(env: Env, property: u64) -> Sale {
        Self::sale_of(&env, property)
    }

    pub fn get_subscription(env: Env, property: u64, investor: Address) -> i128 {
        Self::subscription_of(&env, property, &investor)
    }

    /// Whether [`Self::close`] would succeed right now.
    pub fn is_closable(env: Env, property: u64) -> bool {
        let sale = Self::sale_of(&env, property);
        sale.status == SaleStatus::Open
            && (sale.subscribed == sale.total_shares || env.ledger().timestamp() >= sale.deadline)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_registry(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Registry).unwrap()
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

    pub fn get_treasury(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Treasury).unwrap()
    }

    pub fn get_fee_bps(env: Env) -> u32 {
        Self::fee_bps(&env)
    }

    /// The ceiling on the protocol fee. Fixed in code, not configuration.
    pub fn get_max_fee_bps(_env: Env) -> u32 {
        MAX_FEE_BPS
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

    fn sale_of(env: &Env, property: u64) -> Sale {
        let key = DataKey::Sale(property);
        let sale: Sale = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(env, Error::UnknownSale));
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
        sale
    }

    /// Persist a sale and extend its lifetime. Every mutation goes through
    /// here, so a live sale is renewed each time it is used.
    fn save_sale(env: &Env, sale: &Sale) {
        let key = DataKey::Sale(sale.property);
        env.storage().persistent().set(&key, sale);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn subscription_of(env: &Env, property: u64, investor: &Address) -> i128 {
        let key = DataKey::Subscription(property, investor.clone());
        match env.storage().persistent().get::<DataKey, i128>(&key) {
            Some(shares) => {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, THRESHOLD, EXTEND_TO);
                shares
            }
            None => 0,
        }
    }

    /// A spent or emptied subscription is removed rather than stored as zero,
    /// so a claimed investor stops paying rent on an entry that says nothing.
    fn set_subscription(env: &Env, property: u64, investor: &Address, shares: i128) {
        let key = DataKey::Subscription(property, investor.clone());
        if shares == 0 {
            env.storage().persistent().remove(&key);
            return;
        }
        env.storage().persistent().set(&key, &shares);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn require_open(env: &Env, sale: &Sale) {
        if sale.status != SaleStatus::Open {
            panic_with_error!(env, Error::SaleNotOpen);
        }
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

    fn registry(env: &Env) -> RegistryClient<'_> {
        let address: Address = env
            .storage()
            .instance()
            .get(&DataKey::Registry)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized));
        RegistryClient::new(env, &address)
    }

    fn share_ledger(env: &Env) -> LedgerClient<'_> {
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

    fn treasury(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Treasury)
            .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
    }

    fn fee_bps(env: &Env) -> u32 {
        env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0)
    }
}

#[cfg(test)]
mod test;
