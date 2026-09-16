#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    BytesN, Env,
};

const TIMELOCK: u64 = 172_800;

struct Setup<'a> {
    env: Env,
    registry: PropertyRegistryContractClient<'a>,
    admin: Address,
    pool: Address,
    trustee: Address,
    oracle: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let trustee = Address::generate(&env);
    let oracle = Address::generate(&env);

    let id = env.register(PropertyRegistryContract, (admin.clone(), TIMELOCK));
    let registry = PropertyRegistryContractClient::new(&env, &id);
    registry.set_mortgage_pool(&admin, &pool);
    registry.set_trustee(&admin, &trustee, &true);
    registry.set_oracle(&admin, &oracle, &true);

    Setup {
        env,
        registry,
        admin,
        pool,
        trustee,
        oracle,
    }
}

fn hash(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

/// Submit a property and clear its title, the usual starting point.
fn verified_property(s: &Setup) -> u64 {
    let id = s
        .registry
        .submit_property(&s.trustee, &hash(&s.env, 1), &hash(&s.env, 2));
    s.registry.verify_title(&s.oracle, &id);
    s.registry.set_valuation(&s.oracle, &id, &100_000);
    id
}

/// Sign off every stage up to and including `through`.
fn verify_through(s: &Setup, property: u64, through: u32) {
    for stage in 0..=through {
        s.registry.submit_milestone_evidence(
            &s.trustee,
            &property,
            &stage,
            &hash(&s.env, 10 + stage as u8),
        );
        s.registry.verify_milestone(&s.oracle, &property, &stage);
    }
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.registry.get_admin(), s.admin);
    assert_eq!(s.registry.get_timelock_secs(), TIMELOCK);
    assert_eq!(s.registry.get_next_id(), 1);
    assert_eq!(s.registry.get_mortgage_pool(), Some(s.pool.clone()));
    assert_eq!(s.registry.get_milestone_count(), MILESTONE_COUNT);
}

#[test]
fn test_submitting_a_property_creates_its_build_schedule() {
    let s = setup();
    let id = s
        .registry
        .submit_property(&s.trustee, &hash(&s.env, 1), &hash(&s.env, 2));

    let property = s.registry.get_property(&id);
    assert_eq!(property.id, 1);
    assert_eq!(property.trustee, s.trustee);
    assert_eq!(property.title_hash, hash(&s.env, 1));
    assert_eq!(property.survey_doc_hash, hash(&s.env, 2));
    assert_eq!(property.status, PropertyStatus::Pending);
    assert_eq!(property.usdc_value, 0);

    // All five stages exist before anyone lends against the build.
    for stage in 0..MILESTONE_COUNT {
        let milestone = s.registry.get_milestone(&id, &stage);
        assert_eq!(milestone.stage, stage);
        assert!(!milestone.verified);
        assert!(!milestone.released);
    }
    // And no sixth.
    assert!(s.registry.try_get_milestone(&id, &MILESTONE_COUNT).is_err());
}

#[test]
fn test_only_registered_trustees_can_submit() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    assert!(s
        .registry
        .try_submit_property(&stranger, &hash(&s.env, 1), &hash(&s.env, 2))
        .is_err());

    // Revocation takes effect on the next invocation.
    s.registry.set_trustee(&s.admin, &s.trustee, &false);
    assert!(!s.registry.is_trustee(&s.trustee));
    assert!(s
        .registry
        .try_submit_property(&s.trustee, &hash(&s.env, 1), &hash(&s.env, 2))
        .is_err());
}

#[test]
fn test_a_valuation_needs_a_verified_title_first() {
    let s = setup();
    let id = s
        .registry
        .submit_property(&s.trustee, &hash(&s.env, 1), &hash(&s.env, 2));

    // A valuation on an unchecked title is worth nothing.
    assert!(s
        .registry
        .try_set_valuation(&s.oracle, &id, &100_000)
        .is_err());

    s.registry.verify_title(&s.oracle, &id);
    assert_eq!(s.registry.get_status_of(&id), PropertyStatus::Verified);

    s.registry.set_valuation(&s.oracle, &id, &100_000);
    let property = s.registry.get_property(&id);
    assert_eq!(property.usdc_value, 100_000);
    assert_eq!(property.valued_by, Some(s.oracle.clone()));
    assert_eq!(property.verified_by, Some(s.oracle.clone()));

    // A title is checked once.
    assert!(s.registry.try_verify_title(&s.oracle, &id).is_err());
    // Nonsense valuations are refused.
    assert!(s.registry.try_set_valuation(&s.oracle, &id, &0).is_err());
    assert!(s.registry.try_set_valuation(&s.oracle, &id, &-1).is_err());
}

#[test]
fn test_a_trustee_cannot_verify_their_own_property() {
    let s = setup();
    // Even holding both roles.
    s.registry.set_oracle(&s.admin, &s.trustee, &true);
    let id = s
        .registry
        .submit_property(&s.trustee, &hash(&s.env, 1), &hash(&s.env, 2));

    assert!(s.registry.try_verify_title(&s.trustee, &id).is_err());
    s.registry.verify_title(&s.oracle, &id);
    assert!(s
        .registry
        .try_set_valuation(&s.trustee, &id, &100_000)
        .is_err());

    s.registry
        .submit_milestone_evidence(&s.trustee, &id, &0, &hash(&s.env, 9));
    assert!(s
        .registry
        .try_verify_milestone(&s.trustee, &id, &0)
        .is_err());
}

#[test]
fn test_a_milestone_needs_evidence_before_it_is_signed_off() {
    let s = setup();
    let id = verified_property(&s);

    assert!(s.registry.try_verify_milestone(&s.oracle, &id, &0).is_err());

    s.registry
        .submit_milestone_evidence(&s.trustee, &id, &0, &hash(&s.env, 7));
    s.registry.verify_milestone(&s.oracle, &id, &0);

    let milestone = s.registry.get_milestone(&id, &0);
    assert!(milestone.verified);
    assert_eq!(milestone.evidence_hash, hash(&s.env, 7));
    assert_eq!(milestone.verified_by, Some(s.oracle.clone()));

    // Signed off once, and the evidence is then frozen.
    assert!(s.registry.try_verify_milestone(&s.oracle, &id, &0).is_err());
    assert!(s
        .registry
        .try_submit_milestone_evidence(&s.trustee, &id, &0, &hash(&s.env, 8))
        .is_err());
}

#[test]
fn test_evidence_can_be_corrected_until_it_is_verified() {
    let s = setup();
    let id = verified_property(&s);

    s.registry
        .submit_milestone_evidence(&s.trustee, &id, &0, &hash(&s.env, 7));
    // A rejected submission can be replaced.
    s.registry
        .submit_milestone_evidence(&s.trustee, &id, &0, &hash(&s.env, 8));
    assert_eq!(
        s.registry.get_milestone(&id, &0).evidence_hash,
        hash(&s.env, 8)
    );
}

#[test]
fn test_stages_are_signed_off_in_order() {
    let s = setup();
    let id = verified_property(&s);

    // Evidence for the roof exists, but the foundation has not passed.
    s.registry
        .submit_milestone_evidence(&s.trustee, &id, &2, &hash(&s.env, 12));
    assert!(s.registry.try_verify_milestone(&s.oracle, &id, &2).is_err());

    verify_through(&s, id, 1);
    // Now the roof can be inspected.
    s.registry.verify_milestone(&s.oracle, &id, &2);
    assert_eq!(s.registry.verified_stage_count(&id), 3);
}

#[test]
fn test_only_the_properties_own_trustee_submits_its_evidence() {
    let s = setup();
    let other = Address::generate(&s.env);
    s.registry.set_trustee(&s.admin, &other, &true);
    let id = verified_property(&s);

    assert!(s
        .registry
        .try_submit_milestone_evidence(&other, &id, &0, &hash(&s.env, 7))
        .is_err());
}

#[test]
fn test_release_is_recorded_once_and_only_by_the_pool() {
    let s = setup();
    let id = verified_property(&s);
    verify_through(&s, id, 0);

    assert!(s.registry.is_releasable(&id, &0));
    // Not signed off yet.
    assert!(!s.registry.is_releasable(&id, &1));

    let stranger = Address::generate(&s.env);
    assert!(s.registry.try_mark_released(&stranger, &id, &0).is_err());
    assert!(s.registry.try_mark_released(&s.admin, &id, &0).is_err());
    // An unverified stage cannot be released.
    assert!(s.registry.try_mark_released(&s.pool, &id, &1).is_err());

    s.registry.mark_released(&s.pool, &id, &0);
    assert!(s.registry.get_milestone(&id, &0).released);
    // A stage funds once.
    assert!(!s.registry.is_releasable(&id, &0));
    assert!(s.registry.try_mark_released(&s.pool, &id, &0).is_err());
}

#[test]
fn test_financed_status_only_moves_along_the_allowed_path() {
    let s = setup();
    let id = verified_property(&s);
    let stranger = Address::generate(&s.env);

    // Only the wired pool may move a financed property.
    assert!(s.registry.try_mark_mortgaged(&stranger, &id).is_err());
    assert!(s.registry.try_mark_mortgaged(&s.admin, &id).is_err());
    // Cannot skip straight to repaid.
    assert!(s.registry.try_mark_repaid(&s.pool, &id).is_err());

    s.registry.mark_mortgaged(&s.pool, &id);
    assert_eq!(s.registry.get_status_of(&id), PropertyStatus::Mortgaged);
    // Not twice.
    assert!(s.registry.try_mark_mortgaged(&s.pool, &id).is_err());

    s.registry.mark_repaid(&s.pool, &id);
    assert_eq!(s.registry.get_status_of(&id), PropertyStatus::Repaid);
    // Terminal.
    assert!(s.registry.try_mark_defaulted(&s.pool, &id).is_err());
}

#[test]
fn test_lending_terms_report_what_the_pool_needs() {
    let s = setup();
    let id = s
        .registry
        .submit_property(&s.trustee, &hash(&s.env, 1), &hash(&s.env, 2));

    let (trustee, valuation, verified) = s.registry.lending_terms(&id);
    assert_eq!(trustee, s.trustee);
    assert_eq!(valuation, 0);
    assert!(!verified);

    s.registry.verify_title(&s.oracle, &id);
    s.registry.set_valuation(&s.oracle, &id, &250_000);
    let (_, valuation, verified) = s.registry.lending_terms(&id);
    assert_eq!(valuation, 250_000);
    assert!(verified);

    // Once financed, the property is no longer available to lend against.
    s.registry.mark_mortgaged(&s.pool, &id);
    let (_, _, verified) = s.registry.lending_terms(&id);
    assert!(!verified);
}

#[test]
fn test_unknown_properties_and_stages_are_rejected() {
    let s = setup();
    assert!(s.registry.try_get_property(&99).is_err());
    assert!(s.registry.try_verify_title(&s.oracle, &99).is_err());
    assert!(s.registry.try_get_milestone(&99, &0).is_err());

    let id = verified_property(&s);
    assert!(s.registry.try_get_milestone(&id, &MILESTONE_COUNT).is_err());
    assert!(s
        .registry
        .try_submit_milestone_evidence(&s.trustee, &id, &MILESTONE_COUNT, &hash(&s.env, 1))
        .is_err());
}

#[test]
fn test_wiring_is_set_once() {
    let s = setup();
    let other = Address::generate(&s.env);
    assert!(s.registry.try_set_mortgage_pool(&s.admin, &other).is_err());
    assert_eq!(s.registry.get_mortgage_pool(), Some(s.pool.clone()));
}

#[test]
fn test_role_changes_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    assert!(s
        .registry
        .try_set_trustee(&stranger, &stranger, &true)
        .is_err());
    assert!(s
        .registry
        .try_set_oracle(&stranger, &stranger, &true)
        .is_err());
    assert!(!s.registry.is_trustee(&stranger));
    assert!(!s.registry.is_oracle(&stranger));
}

#[test]
fn test_upgrades_wait_out_the_timelock() {
    let s = setup();
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[3u8; 32]));

    s.env.ledger().set_timestamp(1_000);
    s.registry.schedule_action(&s.admin, &action);
    assert_eq!(
        s.registry.get_scheduled_action().unwrap().eta,
        1_000 + TIMELOCK
    );
    // One at a time, so a queued upgrade stays visible.
    assert!(s.registry.try_schedule_action(&s.admin, &action).is_err());

    s.env.ledger().set_timestamp(1_000 + TIMELOCK - 1);
    assert!(s.registry.try_execute_action(&s.admin).is_err());

    s.registry.cancel_action(&s.admin);
    assert_eq!(s.registry.get_scheduled_action(), None);
    assert!(s.registry.try_cancel_action(&s.admin).is_err());
}

#[test]
fn test_admin_handover_is_two_step() {
    let s = setup();
    let new_admin = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);

    s.registry.propose_admin(&s.admin, &new_admin);
    assert_eq!(s.registry.get_admin(), s.admin);
    assert!(s.registry.try_accept_admin(&stranger).is_err());

    s.registry.accept_admin(&new_admin);
    assert_eq!(s.registry.get_admin(), new_admin);

    let someone = Address::generate(&s.env);
    assert!(s
        .registry
        .try_set_trustee(&s.admin, &someone, &true)
        .is_err());
    s.registry.set_trustee(&new_admin, &someone, &true);
    assert!(s.registry.is_trustee(&someone));
}
