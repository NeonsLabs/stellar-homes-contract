#![no_std]
//! PropertyRegistry — what a property is, who holds it in trust, and how far
//! the build has got.
//!
//! The registry holds no money and lends nothing. It is the notary: it records
//! the hash of a title deed and survey, whether a land-registry oracle has
//! checked that title, what a licensed surveyor valued the property at, and
//! which construction milestones an inspector has signed off.
//!
//! Those signatures are what the MortgagePool reads before it releases a
//! tranche of somebody's money, which is why they live in a contract that
//! cannot itself move funds. A compromised registry can lie about a building;
//! it cannot spend against one.
//!
//! ## Why the paperwork is only a hash
//!
//! A title deed names people and places. Publishing one on a public ledger
//! would expose the borrower, the seller and the plot to anyone who cared to
//! look, permanently. The registry stores only a 32-byte digest, so the
//! documents can be held off-chain, disclosed to the parties who need them,
//! and still be proved unaltered by anyone holding a copy.

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

/// Construction stages, fixed at five to match the build schedule the platform
/// underwrites against: foundation, walls, roofing, finishing, handover.
///
/// Fixed rather than configurable because a tranche is released per stage. A
/// property that could declare its own stage count could declare one stage and
/// draw the whole principal on a poured foundation.
pub const MILESTONE_COUNT: u32 = 5;

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
    NotTrustee = 9,
    NotOracle = 10,
    WrongStatus = 11,
    InvalidValuation = 12,
    InvalidStage = 13,
    NoEvidence = 14,
    AlreadyVerified = 15,
    OutOfOrder = 16,
    MortgagePoolNotSet = 17,
}

/// Where a property sits in its life.
///
/// `Pending -> Verified` is the land-registry check. `Verified -> Mortgaged`
/// happens when a mortgage against it is funded, and from there it ends at
/// `Repaid` or `Defaulted`. The names match the backend's property status
/// exactly, so the two never have to be translated.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PropertyStatus {
    /// Submitted by a trustee, title not yet checked.
    Pending,
    /// The land registry oracle has confirmed the title.
    Verified,
    /// A mortgage against this property has been funded.
    Mortgaged,
    /// The mortgage was paid off.
    Repaid,
    /// The mortgage defaulted.
    Defaulted,
}

/// A property as the protocol knows it.
///
/// `title_hash` digests the deed; `survey_doc_hash` digests the surveyor's
/// report. Neither document is stored.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Property {
    pub id: u64,
    /// The party holding the property in trust for the borrower.
    pub trustee: Address,
    pub title_hash: BytesN<32>,
    pub survey_doc_hash: BytesN<32>,
    /// Surveyor's valuation in the settlement asset. Zero until valued.
    pub usdc_value: i128,
    pub status: PropertyStatus,
    /// The oracle that verified the title, for traceability.
    pub verified_by: Option<Address>,
    /// The oracle that published the valuation.
    pub valued_by: Option<Address>,
}

/// One construction stage.
///
/// `released` is written by the MortgagePool when it pays the tranche out, so
/// a stage can never fund twice even if the registry is called again.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub stage: u32,
    pub evidence_hash: BytesN<32>,
    pub verified: bool,
    pub released: bool,
    /// The inspector who signed this stage off.
    pub verified_by: Option<Address>,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    TimelockSecs,
    Scheduled,
    /// The MortgagePool, the only caller allowed to mark a stage released or
    /// move a property into or out of `Mortgaged`.
    MortgagePool,
    NextId,
    Trustee(Address),
    Oracle(Address),
    Property(u64),
    Milestone(u64, u32),
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

    /// Register the MortgagePool, the only caller allowed to record that a
    /// tranche was released or to move a property's status once it is financed.
    /// Can be set only once: rewiring it later would let the admin point the
    /// registry at a contract that marks any stage funded, so a change goes
    /// through an upgrade and its timelock instead.
    pub fn set_mortgage_pool(env: Env, admin: Address, pool: Address) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        if env.storage().instance().has(&DataKey::MortgagePool) {
            panic_with_error!(&env, Error::AlreadySet);
        }
        env.storage().instance().set(&DataKey::MortgagePool, &pool);
    }

    /// Authorize or revoke a trustee — the party that submits a property and
    /// holds it in trust. Revocation takes effect on the next invocation.
    pub fn set_trustee(env: Env, admin: Address, trustee: Address, authorized: bool) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        Self::set_role(&env, DataKey::Trustee(trustee.clone()), authorized);
        env.events()
            .publish((REGISTRY, symbol_short!("trustee")), (trustee, authorized));
    }

    /// Authorize or revoke an oracle — the land registry check, the surveyor's
    /// valuation and the milestone inspection all run through this role.
    pub fn set_oracle(env: Env, admin: Address, oracle: Address, authorized: bool) {
        Self::extend_instance(&env);
        Self::require_admin(&env, &admin);
        Self::set_role(&env, DataKey::Oracle(oracle.clone()), authorized);
        env.events()
            .publish((REGISTRY, symbol_short!("oracle")), (oracle, authorized));
    }

    /// Schedule an upgrade. It can execute only once the timelock set at
    /// deployment has elapsed, and can be cancelled at any time before that.
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

    // --- Trustee operations ---

    /// Submit a property for title verification. Returns its id.
    ///
    /// The five construction stages are created here, empty, so the schedule a
    /// mortgage is underwritten against exists before anyone lends against it.
    pub fn submit_property(
        env: Env,
        trustee: Address,
        title_hash: BytesN<32>,
        survey_doc_hash: BytesN<32>,
    ) -> u64 {
        Self::extend_instance(&env);
        Self::require_trustee(&env, &trustee);

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let property = Property {
            id,
            trustee: trustee.clone(),
            title_hash,
            survey_doc_hash,
            usdc_value: 0,
            status: PropertyStatus::Pending,
            verified_by: None,
            valued_by: None,
        };
        Self::save(&env, &property);

        for stage in 0..MILESTONE_COUNT {
            Self::save_milestone(
                &env,
                id,
                &Milestone {
                    stage,
                    evidence_hash: BytesN::from_array(&env, &[0u8; 32]),
                    verified: false,
                    released: false,
                    verified_by: None,
                },
            );
        }

        env.events()
            .publish((REGISTRY, symbol_short!("submitted")), (id, trustee));
        id
    }

    /// Submit evidence that a construction stage is complete — photographs,
    /// receipts, an engineer's report — as the digest of the evidence bundle.
    ///
    /// Only the property's own trustee may do this, and only for a stage that
    /// has not already been signed off. Evidence may be replaced until it is
    /// verified, so a rejected submission can be corrected.
    pub fn submit_milestone_evidence(
        env: Env,
        trustee: Address,
        property_id: u64,
        stage: u32,
        evidence_hash: BytesN<32>,
    ) {
        Self::extend_instance(&env);
        Self::require_trustee(&env, &trustee);
        let property = Self::property_of(&env, property_id);
        if property.trustee != trustee {
            panic_with_error!(&env, Error::NotTrustee);
        }

        let mut milestone = Self::milestone_of(&env, property_id, stage);
        if milestone.verified {
            panic_with_error!(&env, Error::AlreadyVerified);
        }
        milestone.evidence_hash = evidence_hash.clone();
        Self::save_milestone(&env, property_id, &milestone);

        env.events().publish(
            (REGISTRY, symbol_short!("evidence")),
            (property_id, stage, evidence_hash),
        );
    }

    // --- Oracle operations ---

    /// Confirm the title against the land registry.
    pub fn verify_title(env: Env, oracle: Address, property_id: u64) {
        Self::extend_instance(&env);
        Self::require_oracle(&env, &oracle);
        let mut property = Self::property_of(&env, property_id);
        if property.status != PropertyStatus::Pending {
            panic_with_error!(&env, Error::WrongStatus);
        }
        // A trustee cannot wave their own title through, even if the admin has
        // granted them both roles.
        if property.trustee == oracle {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        property.status = PropertyStatus::Verified;
        property.verified_by = Some(oracle.clone());
        Self::save(&env, &property);

        env.events()
            .publish((REGISTRY, symbol_short!("title")), (property_id, oracle));
        Self::publish_status(&env, property_id, PropertyStatus::Verified);
    }

    /// Record the surveyor's valuation. The property must have a confirmed
    /// title first — a valuation on an unverified title is worth nothing, and
    /// the mortgage's loan-to-value is computed against this number.
    pub fn set_valuation(env: Env, oracle: Address, property_id: u64, usdc_value: i128) {
        Self::extend_instance(&env);
        Self::require_oracle(&env, &oracle);
        if usdc_value <= 0 {
            panic_with_error!(&env, Error::InvalidValuation);
        }
        let mut property = Self::property_of(&env, property_id);
        if property.trustee == oracle {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        // Verified or already financed; a valuation may be refreshed during the
        // build, but never before the title is confirmed.
        if property.status == PropertyStatus::Pending {
            panic_with_error!(&env, Error::WrongStatus);
        }
        property.usdc_value = usdc_value;
        property.valued_by = Some(oracle.clone());
        Self::save(&env, &property);

        env.events().publish(
            (REGISTRY, symbol_short!("valuation")),
            (property_id, oracle, usdc_value),
        );
    }

    /// Sign off a construction stage, which is what lets the MortgagePool
    /// release that stage's tranche.
    ///
    /// Stages are signed off in order. Skipping one would let a builder draw
    /// the roofing tranche on an unfinished foundation, so an out-of-order
    /// inspection is refused even if the evidence looks good.
    pub fn verify_milestone(env: Env, oracle: Address, property_id: u64, stage: u32) {
        Self::extend_instance(&env);
        Self::require_oracle(&env, &oracle);
        let property = Self::property_of(&env, property_id);
        if property.trustee == oracle {
            panic_with_error!(&env, Error::NotAuthorized);
        }

        let mut milestone = Self::milestone_of(&env, property_id, stage);
        if milestone.verified {
            panic_with_error!(&env, Error::AlreadyVerified);
        }
        // An all-zero digest is what submit_property writes, and means nothing
        // has been submitted for this stage.
        if milestone.evidence_hash == BytesN::from_array(&env, &[0u8; 32]) {
            panic_with_error!(&env, Error::NoEvidence);
        }
        if stage > 0 && !Self::milestone_of(&env, property_id, stage - 1).verified {
            panic_with_error!(&env, Error::OutOfOrder);
        }

        milestone.verified = true;
        milestone.verified_by = Some(oracle.clone());
        Self::save_milestone(&env, property_id, &milestone);

        env.events().publish(
            (REGISTRY, symbol_short!("verified")),
            (property_id, stage, oracle),
        );
    }

    // --- MortgagePool callbacks ---

    /// Record that a stage's tranche has been paid out. MortgagePool only.
    ///
    /// The pool checks `verified` before paying and calls this after, so the
    /// registry is what stops the same stage funding twice.
    pub fn mark_released(env: Env, caller: Address, property_id: u64, stage: u32) {
        Self::extend_instance(&env);
        caller.require_auth();
        Self::require_pool(&env, &caller);

        let mut milestone = Self::milestone_of(&env, property_id, stage);
        if !milestone.verified {
            panic_with_error!(&env, Error::WrongStatus);
        }
        if milestone.released {
            panic_with_error!(&env, Error::AlreadyVerified);
        }
        milestone.released = true;
        Self::save_milestone(&env, property_id, &milestone);

        env.events()
            .publish((REGISTRY, symbol_short!("released")), (property_id, stage));
    }

    /// Record that a mortgage against this property has been funded.
    /// MortgagePool only.
    ///
    /// The pool owns these three transitions because it is the only thing that
    /// knows whether a loan funded, closed or defaulted. They are separate
    /// calls rather than one taking a status, so the pool never has to name the
    /// registry's enum and the two contracts share no types across the wire.
    pub fn mark_mortgaged(env: Env, caller: Address, property_id: u64) {
        Self::transition(&env, caller, property_id, PropertyStatus::Mortgaged);
    }

    /// Record that the mortgage was paid off. MortgagePool only.
    pub fn mark_repaid(env: Env, caller: Address, property_id: u64) {
        Self::transition(&env, caller, property_id, PropertyStatus::Repaid);
    }

    /// Record that the mortgage defaulted. MortgagePool only.
    pub fn mark_defaulted(env: Env, caller: Address, property_id: u64) {
        Self::transition(&env, caller, property_id, PropertyStatus::Defaulted);
    }

    // --- Getters ---

    pub fn get_property(env: Env, property_id: u64) -> Property {
        Self::property_of(&env, property_id)
    }

    pub fn get_status_of(env: Env, property_id: u64) -> PropertyStatus {
        Self::property_of(&env, property_id).status
    }

    pub fn get_milestone(env: Env, property_id: u64, stage: u32) -> Milestone {
        Self::milestone_of(&env, property_id, stage)
    }

    /// What the MortgagePool needs to decide whether it may lend against a
    /// property: `(trustee, usdc_value, is_verified)`.
    ///
    /// Returned as plain values rather than a [`Property`] so the pool can call
    /// across without linking this contract's types into its own wasm. Widening
    /// what it returns widens that interface, so keep it to what lending needs.
    pub fn lending_terms(env: Env, property_id: u64) -> (Address, i128, bool) {
        let property = Self::property_of(&env, property_id);
        (
            property.trustee,
            property.usdc_value,
            property.status == PropertyStatus::Verified,
        )
    }

    /// Whether a stage is signed off and not yet paid out — the single
    /// condition the MortgagePool needs before releasing a tranche.
    pub fn is_releasable(env: Env, property_id: u64, stage: u32) -> bool {
        let milestone = Self::milestone_of(&env, property_id, stage);
        milestone.verified && !milestone.released
    }

    /// How many stages have been signed off, for progress display.
    pub fn verified_stage_count(env: Env, property_id: u64) -> u32 {
        let mut count = 0;
        for stage in 0..MILESTONE_COUNT {
            if Self::milestone_of(&env, property_id, stage).verified {
                count += 1;
            }
        }
        count
    }

    pub fn is_trustee(env: Env, trustee: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Trustee(trustee))
            .unwrap_or(false)
    }

    pub fn is_oracle(env: Env, oracle: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Oracle(oracle))
            .unwrap_or(false)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_mortgage_pool(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::MortgagePool)
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

    pub fn get_milestone_count(_env: Env) -> u32 {
        MILESTONE_COUNT
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

    fn milestone_of(env: &Env, property_id: u64, stage: u32) -> Milestone {
        if stage >= MILESTONE_COUNT {
            panic_with_error!(env, Error::InvalidStage);
        }
        let key = DataKey::Milestone(property_id, stage);
        let milestone: Milestone = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(env, Error::UnknownProperty));
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
        milestone
    }

    fn save_milestone(env: &Env, property_id: u64, milestone: &Milestone) {
        let key = DataKey::Milestone(property_id, milestone.stage);
        env.storage().persistent().set(&key, milestone);
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    /// The one path that moves a financed property's status, so the legal
    /// transitions are stated in a single place.
    fn transition(env: &Env, caller: Address, property_id: u64, status: PropertyStatus) {
        Self::extend_instance(env);
        caller.require_auth();
        Self::require_pool(env, &caller);

        let mut property = Self::property_of(env, property_id);
        let allowed = match status {
            PropertyStatus::Mortgaged => property.status == PropertyStatus::Verified,
            PropertyStatus::Repaid | PropertyStatus::Defaulted => {
                property.status == PropertyStatus::Mortgaged
            }
            // Pending and Verified are the registry's own to set.
            _ => false,
        };
        if !allowed {
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

    fn require_trustee(env: &Env, trustee: &Address) {
        trustee.require_auth();
        let key = DataKey::Trustee(trustee.clone());
        if !env.storage().persistent().get(&key).unwrap_or(false) {
            panic_with_error!(env, Error::NotTrustee);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
    }

    fn require_oracle(env: &Env, oracle: &Address) {
        oracle.require_auth();
        let key = DataKey::Oracle(oracle.clone());
        if !env.storage().persistent().get(&key).unwrap_or(false) {
            panic_with_error!(env, Error::NotOracle);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, THRESHOLD, EXTEND_TO);
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
