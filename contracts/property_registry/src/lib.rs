#![no_std]
//! PropertyRegistry — what a property is, who speaks for it, and where it is
//! in its life.
//!
//! The registry holds no money and issues no shares. It is the one place that
//! records which sponsor is responsible for a property, what independent
//! appraisers think it is worth, and whether it is still being offered, owned
//! by its shareholders, or retired. The Offering contract reads that record
//! before it takes a cent, and writes back the outcome of the sale.
//!
//! A property cannot reach the market on its sponsor's word alone: an offering
//! may only open once a registered appraiser has published a valuation, and
//! the sponsor may not appraise their own property.

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

/// The largest share count a property may be divided into. Bounded so that
/// `shares * price_per_share` cannot approach the range of `i128` and so that
/// the per-share accounting in the ShareLedger keeps its precision.
const MAX_TOTAL_SHARES: i128 = 1_000_000_000_000;

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
    UnknownProperty = 8,
    InvalidShareCount = 9,
    InvalidPrice = 10,
    InvalidValuation = 11,
    NotSponsor = 12,
    NotAppraiser = 13,
    SelfAppraisal = 14,
    NotAppraised = 15,
    WrongStatus = 16,
    OfferingNotSet = 17,
}

/// Where a property sits in its life.
///
/// The only paths are `Draft -> Offering`, and from there either `Owned` when
/// the sale settles or `Failed` when it does not. A settled property ends at
/// `Retired` when the building is sold and shareholders are bought out. There
/// is no route back into `Offering`: a failed raise is re-registered as a new
/// property, so the cap table of a settled offering can never be reopened.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PropertyStatus {
    /// Registered, not yet on sale.
    Draft,
    /// Shares are being sold by the Offering contract.
    Offering,
    /// The sale settled; the property belongs to its shareholders.
    Owned,
    /// The sale did not reach its soft cap and was unwound.
    Failed,
    /// Wound up. No further income is expected.
    Retired,
}

/// A property as the protocol knows it.
///
/// Nothing here identifies a person or a street address. `document_hash` is the
/// digest of the off-chain prospectus, deed and title report; the registry
/// stores only the digest so that the paperwork can be published, mirrored and
/// verified anywhere without putting it on a public ledger.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Property {
    pub id: u64,
    pub sponsor: Address,
    pub document_hash: BytesN<32>,
    pub total_shares: i128,
    /// Primary-sale price of one share, in units of the settlement asset.
    pub price_per_share: i128,
    pub status: PropertyStatus,
    /// Most recent independent appraisal, in units of the settlement asset.
    /// Zero until an appraiser has published one.
    pub valuation: i128,
    /// Ledger timestamp of that appraisal.
    pub valued_at: u64,
    /// The appraiser who published it, so a stale or disputed valuation can be
    /// traced back to the party that signed it.
    pub appraiser: Option<Address>,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    /// The Offering contract, the only caller allowed to settle a sale.
    Offering,
    /// Monotonic source of property ids.
    NextId,
    Sponsor(Address),
    Appraiser(Address),
    Property(u64),
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

const REGISTRY: Symbol = symbol_short!("registry");

#[contract]
pub struct PropertyRegistryContract;

#[contractimpl]
impl PropertyRegistryContract {
    /// Runs once, atomically, as part of the deploy transaction. There is no
    /// separate initialize call for anyone to front-run between deployment and
    /// setup, so nobody else can claim the admin role.
    pub fn __constructor(env: Env, admin: Address, timelock_secs: u64) {
        Self::extend_instance(&env);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::TimelockSecs, &timelock_secs);
        env.storage().instance().set(&DataKey::NextId, &1u64);
    }

    // --- Administration ---

    /// Register the Offering contract, the only caller allowed to record the
    /// outcome of a sale. Can be set only once: rewiring it later would let the
    /// admin point the registry at a contract that marks any property owned, so
    /// a change goes through an upgrade and its timelock instead.
    pub fn set_offering(env: Env, admin: Address, offering: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::Offering) {
            panic_with_error!(&env, Error::AlreadySet);
        }
        env.storage().instance().set(&DataKey::Offering, &offering);
    }

    /// Authorize or revoke a sponsor. Revocation takes effect on the next
    /// invocation; properties the sponsor already registered keep their record,
    /// but a revoked sponsor can no longer register or open anything new.
    pub fn set_sponsor(env: Env, admin: Address, sponsor: Address, authorized: bool) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        Self::set_role(&env, DataKey::Sponsor(sponsor.clone()), authorized);
        env.events()
            .publish((REGISTRY, symbol_short!("sponsor")), (sponsor, authorized));
    }

    /// Authorize or revoke an appraiser. An appraiser's signature is what lets
    /// a property go on sale, so the set is deliberately small and admin-held.
    pub fn set_appraiser(env: Env, admin: Address, appraiser: Address, authorized: bool) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        Self::set_role(&env, DataKey::Appraiser(appraiser.clone()), authorized);
        env.events().publish(
            (REGISTRY, symbol_short!("appraisr")),
            (appraiser, authorized),
        );
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

    // --- Sponsor operations ---

    /// Record a new property and return its id. The property starts in `Draft`:
    /// it cannot be offered until an appraiser has valued it.
    pub fn register_property(
        env: Env,
        sponsor: Address,
        document_hash: BytesN<32>,
        total_shares: i128,
        price_per_share: i128,
    ) -> u64 {
        Self::extend_instance(&env);
        Self::require_sponsor(&env, &sponsor);
        if total_shares <= 0 || total_shares > MAX_TOTAL_SHARES {
            panic_with_error!(&env, Error::InvalidShareCount);
        }
        if price_per_share <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let property = Property {
            id,
            sponsor: sponsor.clone(),
            document_hash,
            total_shares,
            price_per_share,
            status: PropertyStatus::Draft,
            valuation: 0,
            valued_at: 0,
            appraiser: None,
        };
        Self::save(&env, &property);
        env.events()
            .publish((REGISTRY, symbol_short!("register")), (id, sponsor));
        id
    }

    /// Put a valued property on sale. Only its own sponsor may do this, and
    /// only once an appraisal exists — the raise is sized against a number
    /// somebody independent has signed.
    pub fn open_offering(env: Env, sponsor: Address, property_id: u64) {
        Self::extend_instance(&env);
        Self::require_sponsor(&env, &sponsor);
        let mut property = Self::property_of(&env, property_id);
        if property.sponsor != sponsor {
            panic_with_error!(&env, Error::NotSponsor);
        }
        if property.status != PropertyStatus::Draft {
            panic_with_error!(&env, Error::WrongStatus);
        }
        if property.valuation <= 0 {
            panic_with_error!(&env, Error::NotAppraised);
        }
        property.status = PropertyStatus::Offering;
        Self::save(&env, &property);
        Self::publish_status(&env, property_id, PropertyStatus::Offering);
    }

    // --- Appraiser operations ---

    /// Publish an independent valuation. A sponsor may never value their own
    /// property, even if the admin has granted them both roles.
    pub fn publish_valuation(env: Env, appraiser: Address, property_id: u64, valuation: i128) {
        Self::extend_instance(&env);
        Self::require_appraiser(&env, &appraiser);
        if valuation <= 0 {
            panic_with_error!(&env, Error::InvalidValuation);
        }
        let mut property = Self::property_of(&env, property_id);
        if property.sponsor == appraiser {
            panic_with_error!(&env, Error::SelfAppraisal);
        }
        if property.status == PropertyStatus::Retired || property.status == PropertyStatus::Failed {
            panic_with_error!(&env, Error::WrongStatus);
        }
        property.valuation = valuation;
        property.valued_at = env.ledger().timestamp();
        property.appraiser = Some(appraiser.clone());
        Self::save(&env, &property);
        env.events().publish(
            (REGISTRY, symbol_short!("valuation")),
            (property_id, appraiser, valuation),
        );
    }

    // --- Offering callbacks ---

    /// Record that a sale settled. Offering contract only.
    pub fn mark_owned(env: Env, caller: Address, property_id: u64) {
        Self::transition_from_offering(&env, caller, property_id, PropertyStatus::Owned);
    }

    /// Record that a sale missed its soft cap and was unwound. Offering
    /// contract only.
    pub fn mark_failed(env: Env, caller: Address, property_id: u64) {
        Self::transition_from_offering(&env, caller, property_id, PropertyStatus::Failed);
    }

    /// Wind up an owned property once the building has been sold and
    /// shareholders bought out off-chain. Admin only, and one-way.
    pub fn retire_property(env: Env, admin: Address, property_id: u64) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        let mut property = Self::property_of(&env, property_id);
        if property.status != PropertyStatus::Owned {
            panic_with_error!(&env, Error::WrongStatus);
        }
        property.status = PropertyStatus::Retired;
        Self::save(&env, &property);
        Self::publish_status(&env, property_id, PropertyStatus::Retired);
    }

    // --- Getters ---

    pub fn get_property(env: Env, property_id: u64) -> Property {
        Self::property_of(&env, property_id)
    }

    pub fn get_status(env: Env, property_id: u64) -> PropertyStatus {
        Self::property_of(&env, property_id).status
    }

    /// The terms the Offering contract needs to run a sale, and nothing else:
    /// `(sponsor, total_shares, price_per_share, is_offering)`.
    ///
    /// Returned as plain values rather than a [`Property`] so the Offering
    /// contract can call across without linking this contract's types into its
    /// own wasm. Widening what it returns widens that interface, so keep it to
    /// what a sale actually needs.
    pub fn offering_terms(env: Env, property_id: u64) -> (Address, i128, i128, bool) {
        let property = Self::property_of(&env, property_id);
        (
            property.sponsor,
            property.total_shares,
            property.price_per_share,
            property.status == PropertyStatus::Offering,
        )
    }

    pub fn is_sponsor(env: Env, sponsor: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Sponsor(sponsor))
            .unwrap_or(false)
    }

    pub fn is_appraiser(env: Env, appraiser: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Appraiser(appraiser))
            .unwrap_or(false)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_offering(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Offering)
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

    pub fn get_next_id(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::NextId).unwrap_or(1)
    }

    // --- Internals ---

    /// Keep the contract instance, and with it the configuration and the
    /// contract code, from being archived while the protocol is in use.
    fn extend_instance(env: &Env) {
        env.storage().instance().extend_ttl(THRESHOLD, EXTEND_TO);
    }

    fn set_role(env: &Env, key: DataKey, authorized: bool) {
        if authorized {
            env.storage().persistent().set(&key, &true);
            env.storage()
                .persistent()
                .extend_ttl(&key, THRESHOLD, EXTEND_TO);
        } else {
            env.storage().persistent().remove(&key);
        }
    }

    fn property_of(env: &Env, property_id: u64) -> Property {
        let key = DataKey::Property(property_id);
        let property: Property = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(env, Error::UnknownProperty));
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
        property
    }

    /// Persist a property and extend its lifetime. Every mutation goes through
    /// here, so a property in use is renewed each time it is touched.
    fn save(env: &Env, property: &Property) {
        let key = DataKey::Property(property.id);
        env.storage().persistent().set(&key, property);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn transition_from_offering(
        env: &Env,
        caller: Address,
        property_id: u64,
        status: PropertyStatus,
    ) {
        Self::extend_instance(env);
        caller.require_auth();
        Self::require_offering(env, &caller);
        let mut property = Self::property_of(env, property_id);
        if property.status != PropertyStatus::Offering {
            panic_with_error!(env, Error::WrongStatus);
        }
        property.status = status;
        Self::save(env, &property);
        Self::publish_status(env, property_id, status);
    }

    fn publish_status(env: &Env, property_id: u64, status: PropertyStatus) {
        env.events()
            .publish((REGISTRY, symbol_short!("status")), (property_id, status));
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

    fn require_sponsor(env: &Env, sponsor: &Address) {
        sponsor.require_auth();
        let key = DataKey::Sponsor(sponsor.clone());
        if !env.storage().persistent().get(&key).unwrap_or(false) {
            panic_with_error!(env, Error::NotSponsor);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn require_appraiser(env: &Env, appraiser: &Address) {
        appraiser.require_auth();
        let key = DataKey::Appraiser(appraiser.clone());
        if !env.storage().persistent().get(&key).unwrap_or(false) {
            panic_with_error!(env, Error::NotAppraiser);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn require_offering(env: &Env, caller: &Address) {
        let offering: Address = env
            .storage()
            .instance()
            .get(&DataKey::Offering)
            .unwrap_or_else(|| panic_with_error!(env, Error::OfferingNotSet));
        if *caller != offering {
            panic_with_error!(env, Error::NotAuthorized);
        }
    }
}

#[cfg(test)]
mod test;
