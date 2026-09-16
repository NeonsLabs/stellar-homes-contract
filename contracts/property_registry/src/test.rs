#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    BytesN, Env,
};

const TIMELOCK: u64 = 172_800;
const SHARES: i128 = 10_000;
const PRICE: i128 = 100;

struct Setup<'a> {
    env: Env,
    registry: PropertyRegistryContractClient<'a>,
    admin: Address,
    offering: Address,
    sponsor: Address,
    appraiser: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let offering = Address::generate(&env);
    let sponsor = Address::generate(&env);
    let appraiser = Address::generate(&env);

    let id = env.register(PropertyRegistryContract, (admin.clone(), TIMELOCK));
    let registry = PropertyRegistryContractClient::new(&env, &id);
    registry.set_offering(&admin, &offering);
    registry.set_sponsor(&admin, &sponsor, &true);
    registry.set_appraiser(&admin, &appraiser, &true);

    Setup {
        env,
        registry,
        admin,
        offering,
        sponsor,
        appraiser,
    }
}

fn docs(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

/// Register, appraise and open in one step; most tests start from a live offering.
fn offered(s: &Setup) -> u64 {
    let id = s
        .registry
        .register_property(&s.sponsor, &docs(&s.env), &SHARES, &PRICE);
    s.registry.publish_valuation(&s.appraiser, &id, &1_000_000);
    s.registry.open_offering(&s.sponsor, &id);
    id
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.registry.get_admin(), s.admin);
    assert_eq!(s.registry.get_timelock_secs(), TIMELOCK);
    assert_eq!(s.registry.get_next_id(), 1);
    assert_eq!(s.registry.get_offering(), Some(s.offering.clone()));
    assert_eq!(s.registry.get_scheduled_action(), None);
}

#[test]
fn test_register_property_records_the_sponsors_terms() {
    let s = setup();
    let id = s
        .registry
        .register_property(&s.sponsor, &docs(&s.env), &SHARES, &PRICE);

    let property = s.registry.get_property(&id);
    assert_eq!(property.id, 1);
    assert_eq!(property.sponsor, s.sponsor);
    assert_eq!(property.document_hash, docs(&s.env));
    assert_eq!(property.total_shares, SHARES);
    assert_eq!(property.price_per_share, PRICE);
    assert_eq!(property.status, PropertyStatus::Draft);
    assert_eq!(property.valuation, 0);
    assert_eq!(property.appraiser, None);

    // Ids are handed out in order and never reused.
    let second = s
        .registry
        .register_property(&s.sponsor, &docs(&s.env), &SHARES, &PRICE);
    assert_eq!(second, 2);
    assert_eq!(s.registry.get_next_id(), 3);
}

#[test]
fn test_register_property_rejects_invalid_terms() {
    let s = setup();
    let d = docs(&s.env);

    assert!(s
        .registry
        .try_register_property(&s.sponsor, &d, &0, &PRICE)
        .is_err());
    assert!(s
        .registry
        .try_register_property(&s.sponsor, &d, &-1, &PRICE)
        .is_err());
    // Above MAX_TOTAL_SHARES, where the per-share accounting loses precision.
    assert!(s
        .registry
        .try_register_property(&s.sponsor, &d, &(MAX_TOTAL_SHARES + 1), &PRICE)
        .is_err());
    assert!(s
        .registry
        .try_register_property(&s.sponsor, &d, &SHARES, &0)
        .is_err());
}

#[test]
fn test_only_registered_sponsors_can_register() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let d = docs(&s.env);

    assert!(s
        .registry
        .try_register_property(&stranger, &d, &SHARES, &PRICE)
        .is_err());

    // Revocation takes effect on the next invocation.
    s.registry.set_sponsor(&s.admin, &s.sponsor, &false);
    assert!(!s.registry.is_sponsor(&s.sponsor));
    assert!(s
        .registry
        .try_register_property(&s.sponsor, &d, &SHARES, &PRICE)
        .is_err());
}

#[test]
fn test_a_property_cannot_be_offered_before_it_is_appraised() {
    let s = setup();
    let id = s
        .registry
        .register_property(&s.sponsor, &docs(&s.env), &SHARES, &PRICE);

    // The sponsor's own say-so is not enough to reach the market.
    assert!(s.registry.try_open_offering(&s.sponsor, &id).is_err());

    s.registry.publish_valuation(&s.appraiser, &id, &1_250_000);
    s.registry.open_offering(&s.sponsor, &id);
    assert_eq!(s.registry.get_status(&id), PropertyStatus::Offering);
}

#[test]
fn test_valuations_are_independent_and_attributed() {
    let s = setup();
    let id = s
        .registry
        .register_property(&s.sponsor, &docs(&s.env), &SHARES, &PRICE);

    s.env.ledger().set_timestamp(5_000);
    s.registry.publish_valuation(&s.appraiser, &id, &900_000);

    let property = s.registry.get_property(&id);
    assert_eq!(property.valuation, 900_000);
    assert_eq!(property.valued_at, 5_000);
    assert_eq!(property.appraiser, Some(s.appraiser.clone()));

    // A sponsor may not value their own property, even holding both roles.
    s.registry.set_appraiser(&s.admin, &s.sponsor, &true);
    assert!(s
        .registry
        .try_publish_valuation(&s.sponsor, &id, &5_000_000)
        .is_err());

    // Unregistered addresses and nonsense numbers are refused.
    let stranger = Address::generate(&s.env);
    assert!(s
        .registry
        .try_publish_valuation(&stranger, &id, &900_000)
        .is_err());
    assert!(s
        .registry
        .try_publish_valuation(&s.appraiser, &id, &0)
        .is_err());

    // A later appraisal replaces the earlier one.
    s.env.ledger().set_timestamp(9_000);
    s.registry.publish_valuation(&s.appraiser, &id, &1_100_000);
    let property = s.registry.get_property(&id);
    assert_eq!(property.valuation, 1_100_000);
    assert_eq!(property.valued_at, 9_000);
}

#[test]
fn test_only_the_properties_own_sponsor_can_open_it() {
    let s = setup();
    let other_sponsor = Address::generate(&s.env);
    s.registry.set_sponsor(&s.admin, &other_sponsor, &true);

    let id = s
        .registry
        .register_property(&s.sponsor, &docs(&s.env), &SHARES, &PRICE);
    s.registry.publish_valuation(&s.appraiser, &id, &1_000_000);

    assert!(s.registry.try_open_offering(&other_sponsor, &id).is_err());
    s.registry.open_offering(&s.sponsor, &id);
}

#[test]
fn test_status_only_moves_along_the_allowed_path() {
    let s = setup();
    let id = offered(&s);

    // Nobody but the wired Offering contract settles a sale.
    let stranger = Address::generate(&s.env);
    assert!(s.registry.try_mark_owned(&stranger, &id).is_err());
    assert!(s.registry.try_mark_owned(&s.admin, &id).is_err());

    s.registry.mark_owned(&s.offering, &id);
    assert_eq!(s.registry.get_status(&id), PropertyStatus::Owned);

    // An owned property cannot be settled again or reopened.
    assert!(s.registry.try_mark_owned(&s.offering, &id).is_err());
    assert!(s.registry.try_mark_failed(&s.offering, &id).is_err());
    assert!(s.registry.try_open_offering(&s.sponsor, &id).is_err());

    // Retirement is admin-only and one-way.
    assert!(s.registry.try_retire_property(&stranger, &id).is_err());
    s.registry.retire_property(&s.admin, &id);
    assert_eq!(s.registry.get_status(&id), PropertyStatus::Retired);
    assert!(s.registry.try_retire_property(&s.admin, &id).is_err());
}

#[test]
fn test_a_failed_offering_is_terminal() {
    let s = setup();
    let id = offered(&s);

    s.registry.mark_failed(&s.offering, &id);
    assert_eq!(s.registry.get_status(&id), PropertyStatus::Failed);

    // No route back: a failed raise is re-registered as a new property, so a
    // cap table can never be reopened under the same id.
    assert!(s.registry.try_open_offering(&s.sponsor, &id).is_err());
    assert!(s.registry.try_mark_owned(&s.offering, &id).is_err());
    assert!(s.registry.try_retire_property(&s.admin, &id).is_err());
    assert!(s
        .registry
        .try_publish_valuation(&s.appraiser, &id, &10)
        .is_err());
}

#[test]
fn test_unknown_properties_are_rejected() {
    let s = setup();
    assert!(s.registry.try_get_property(&42).is_err());
    assert!(s.registry.try_open_offering(&s.sponsor, &42).is_err());
    assert!(s
        .registry
        .try_publish_valuation(&s.appraiser, &42, &1)
        .is_err());
}

#[test]
fn test_wiring_is_set_once() {
    let s = setup();
    let other = Address::generate(&s.env);
    assert!(s.registry.try_set_offering(&s.admin, &other).is_err());
    assert_eq!(s.registry.get_offering(), Some(s.offering.clone()));
}

#[test]
fn test_role_changes_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);

    assert!(s
        .registry
        .try_set_sponsor(&stranger, &stranger, &true)
        .is_err());
    assert!(s
        .registry
        .try_set_appraiser(&stranger, &stranger, &true)
        .is_err());
    assert!(s.registry.try_set_offering(&stranger, &stranger).is_err());
    assert!(!s.registry.is_sponsor(&stranger));
    assert!(!s.registry.is_appraiser(&stranger));
}

#[test]
fn test_upgrades_wait_out_the_timelock() {
    let s = setup();
    let wasm_hash = BytesN::from_array(&s.env, &[3u8; 32]);
    let action = Action::Upgrade(wasm_hash);

    s.env.ledger().set_timestamp(1_000);
    s.registry.schedule_action(&s.admin, &action);
    let scheduled = s.registry.get_scheduled_action().unwrap();
    assert_eq!(scheduled.eta, 1_000 + TIMELOCK);

    // Only one action may be pending, so a queued upgrade stays visible.
    assert!(s.registry.try_schedule_action(&s.admin, &action).is_err());

    // Not yet.
    s.env.ledger().set_timestamp(1_000 + TIMELOCK - 1);
    assert!(s.registry.try_execute_action(&s.admin).is_err());

    // Cancelling clears the queue.
    s.registry.cancel_action(&s.admin);
    assert_eq!(s.registry.get_scheduled_action(), None);
    assert!(s.registry.try_cancel_action(&s.admin).is_err());
    assert!(s.registry.try_execute_action(&s.admin).is_err());
}

#[test]
fn test_timelocked_actions_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[4u8; 32]));

    assert!(s.registry.try_schedule_action(&stranger, &action).is_err());
    s.registry.schedule_action(&s.admin, &action);
    assert!(s.registry.try_execute_action(&stranger).is_err());
    assert!(s.registry.try_cancel_action(&stranger).is_err());
}

#[test]
fn test_admin_handover_is_two_step() {
    let s = setup();
    let new_admin = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);

    s.registry.propose_admin(&s.admin, &new_admin);
    // Proposing changes nothing on its own.
    assert_eq!(s.registry.get_admin(), s.admin);
    // Only the proposed address can accept, so a mistyped address is harmless.
    assert!(s.registry.try_accept_admin(&stranger).is_err());

    s.registry.accept_admin(&new_admin);
    assert_eq!(s.registry.get_admin(), new_admin);

    // The old admin's powers are gone.
    let sponsor = Address::generate(&s.env);
    assert!(s
        .registry
        .try_set_sponsor(&s.admin, &sponsor, &true)
        .is_err());
    s.registry.set_sponsor(&new_admin, &sponsor, &true);
    assert!(s.registry.is_sponsor(&sponsor));
}
