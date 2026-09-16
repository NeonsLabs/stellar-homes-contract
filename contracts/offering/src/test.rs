#![cfg(test)]

use super::*;
use sh_property_registry::{PropertyRegistryContract, PropertyRegistryContractClient};
use sh_share_ledger::{ShareLedgerContract, ShareLedgerContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, BytesN, Env,
};

const TIMELOCK: u64 = 172_800;
const DURATION: u64 = 604_800;
const TOTAL_SHARES: i128 = 1_000;
const PRICE: i128 = 100;
const MIN_SHARES: i128 = 400;
const FEE_BPS: u32 = 250;

struct Setup<'a> {
    env: Env,
    offering: OfferingContractClient<'a>,
    registry: PropertyRegistryContractClient<'a>,
    shares: ShareLedgerContractClient<'a>,
    usdc: token::Client<'a>,
    admin: Address,
    sponsor: Address,
    treasury: Address,
    alice: Address,
    bob: Address,
    property: u64,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let sponsor = Address::generate(&env);
    let appraiser = Address::generate(&env);
    let treasury = Address::generate(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let usdc = token::Client::new(&env, &sac.address());
    let minter = token::StellarAssetClient::new(&env, &sac.address());
    minter.mint(&alice, &1_000_000);
    minter.mint(&bob, &1_000_000);

    let registry_id = env.register(PropertyRegistryContract, (admin.clone(), TIMELOCK));
    let registry = PropertyRegistryContractClient::new(&env, &registry_id);

    let ledger_id = env.register(ShareLedgerContract, (admin.clone(), TIMELOCK));
    let shares = ShareLedgerContractClient::new(&env, &ledger_id);

    let offering_id = env.register(
        OfferingContract,
        (
            admin.clone(),
            registry_id.clone(),
            ledger_id.clone(),
            sac.address(),
            treasury.clone(),
            FEE_BPS,
            TIMELOCK,
        ),
    );
    let offering = OfferingContractClient::new(&env, &offering_id);

    registry.set_offering(&admin, &offering_id);
    shares.set_offering(&admin, &offering_id);
    registry.set_sponsor(&admin, &sponsor, &true);
    registry.set_appraiser(&admin, &appraiser, &true);

    // A property valued, cleared and ready to sell.
    let property = registry.register_property(
        &sponsor,
        &BytesN::from_array(&env, &[1u8; 32]),
        &TOTAL_SHARES,
        &PRICE,
    );
    registry.publish_valuation(&appraiser, &property, &100_000);
    registry.open_offering(&sponsor, &property);

    Setup {
        env,
        offering,
        registry,
        shares,
        usdc,
        admin,
        sponsor,
        treasury,
        alice,
        bob,
        property,
    }
}

/// Open the sale with the default terms.
fn open(s: &Setup) {
    s.offering
        .open(&s.sponsor, &s.property, &MIN_SHARES, &DURATION);
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.offering.get_admin(), s.admin);
    assert_eq!(s.offering.get_fee_bps(), FEE_BPS);
    assert_eq!(s.offering.get_treasury(), s.treasury);
    assert_eq!(s.offering.get_timelock_secs(), TIMELOCK);
    assert!(s.offering.get_fee_bps() <= s.offering.get_max_fee_bps());
}

#[test]
fn test_open_copies_the_terms_the_registry_cleared() {
    let s = setup();
    s.env.ledger().set_timestamp(1_000);
    let sale = s
        .offering
        .open(&s.sponsor, &s.property, &MIN_SHARES, &DURATION);

    assert_eq!(sale.property, s.property);
    assert_eq!(sale.sponsor, s.sponsor);
    assert_eq!(sale.total_shares, TOTAL_SHARES);
    assert_eq!(sale.price_per_share, PRICE);
    assert_eq!(sale.min_shares, MIN_SHARES);
    assert_eq!(sale.subscribed, 0);
    assert_eq!(sale.deadline, 1_000 + DURATION);
    assert_eq!(sale.status, SaleStatus::Open);
}

#[test]
fn test_only_the_registered_sponsor_of_a_cleared_property_can_open() {
    let s = setup();
    let other = Address::generate(&s.env);
    s.registry.set_sponsor(&s.admin, &other, &true);

    // Wrong sponsor.
    assert!(s
        .offering
        .try_open(&other, &s.property, &MIN_SHARES, &DURATION)
        .is_err());

    // A property the registry has not cleared for offering.
    let draft = s.registry.register_property(
        &s.sponsor,
        &BytesN::from_array(&s.env, &[2u8; 32]),
        &TOTAL_SHARES,
        &PRICE,
    );
    assert!(s
        .offering
        .try_open(&s.sponsor, &draft, &MIN_SHARES, &DURATION)
        .is_err());

    // Nonsense terms.
    assert!(s
        .offering
        .try_open(&s.sponsor, &s.property, &0, &DURATION)
        .is_err());
    assert!(s
        .offering
        .try_open(&s.sponsor, &s.property, &(TOTAL_SHARES + 1), &DURATION)
        .is_err());
    assert!(s
        .offering
        .try_open(&s.sponsor, &s.property, &MIN_SHARES, &0)
        .is_err());

    open(&s);
    // One sale per property, ever.
    assert!(s
        .offering
        .try_open(&s.sponsor, &s.property, &MIN_SHARES, &DURATION)
        .is_err());
}

#[test]
fn test_subscriptions_are_escrowed_not_forwarded() {
    let s = setup();
    open(&s);

    s.offering.subscribe(&s.property, &s.alice, &300);

    // The money is in the offering contract, not with the sponsor.
    assert_eq!(s.usdc.balance(&s.offering.address), 30_000);
    assert_eq!(s.usdc.balance(&s.alice), 970_000);
    assert_eq!(s.usdc.balance(&s.sponsor), 0);
    assert_eq!(s.offering.get_subscription(&s.property, &s.alice), 300);
    assert_eq!(s.offering.get_sale(&s.property).subscribed, 300);

    // Nothing is issued while the sale is open.
    assert_eq!(s.shares.total_issued(&s.property), 0);
}

#[test]
fn test_a_sale_cannot_be_oversubscribed() {
    let s = setup();
    open(&s);

    s.offering.subscribe(&s.property, &s.alice, &600);
    assert!(s
        .offering
        .try_subscribe(&s.property, &s.bob, &(TOTAL_SHARES - 600 + 1))
        .is_err());

    // Exactly the remainder is fine.
    s.offering.subscribe(&s.property, &s.bob, &400);
    assert_eq!(s.offering.get_sale(&s.property).subscribed, TOTAL_SHARES);

    assert!(s.offering.try_subscribe(&s.property, &s.alice, &1).is_err());
    assert!(s.offering.try_subscribe(&s.property, &s.alice, &0).is_err());
}

#[test]
fn test_an_investor_can_back_out_while_the_sale_is_open() {
    let s = setup();
    open(&s);

    s.offering.subscribe(&s.property, &s.alice, &500);
    s.offering
        .withdraw_subscription(&s.property, &s.alice, &200);

    assert_eq!(s.offering.get_subscription(&s.property, &s.alice), 300);
    assert_eq!(s.offering.get_sale(&s.property).subscribed, 300);
    assert_eq!(s.usdc.balance(&s.alice), 970_000);
    assert_eq!(s.usdc.balance(&s.offering.address), 30_000);

    // Cannot take out more than was put in.
    assert!(s
        .offering
        .try_withdraw_subscription(&s.property, &s.alice, &301)
        .is_err());

    // Once the deadline passes the sale is decided by its subscriptions.
    s.env.ledger().set_timestamp(DURATION + 1);
    assert!(s
        .offering
        .try_withdraw_subscription(&s.property, &s.alice, &1)
        .is_err());
}

#[test]
fn test_a_sale_that_sells_out_closes_immediately() {
    let s = setup();
    open(&s);

    s.offering.subscribe(&s.property, &s.alice, &600);
    assert!(!s.offering.is_closable(&s.property));
    // Not sold out, deadline not reached.
    assert!(s.offering.try_close(&s.property).is_err());

    s.offering.subscribe(&s.property, &s.bob, &400);
    assert!(s.offering.is_closable(&s.property));
    assert_eq!(s.offering.close(&s.property), SaleStatus::Settled);
}

#[test]
fn test_settlement_pays_the_sponsor_and_the_treasury() {
    let s = setup();
    open(&s);

    s.offering.subscribe(&s.property, &s.alice, &600);
    s.offering.subscribe(&s.property, &s.bob, &400);
    s.offering.close(&s.property);

    // 1000 shares at 100 is 100_000; the 2.5% fee is 2_500.
    assert_eq!(s.usdc.balance(&s.treasury), 2_500);
    assert_eq!(s.usdc.balance(&s.sponsor), 97_500);
    assert_eq!(s.usdc.balance(&s.offering.address), 0);

    // And the registry now records the property as owned by its shareholders.
    assert_eq!(
        s.registry.get_status(&s.property),
        sh_property_registry::PropertyStatus::Owned
    );
}

#[test]
fn test_a_partly_sold_raise_settles_at_what_it_sold() {
    let s = setup();
    open(&s);

    // Above the 400-share soft cap but short of the 1000 on offer.
    s.offering.subscribe(&s.property, &s.alice, &500);
    s.env.ledger().set_timestamp(DURATION + 1);
    assert_eq!(s.offering.close(&s.property), SaleStatus::Settled);

    // The sponsor is paid for 500 shares only.
    let proceeds = 500 * PRICE;
    let fee = proceeds * FEE_BPS as i128 / 10_000;
    assert_eq!(s.usdc.balance(&s.treasury), fee);
    assert_eq!(s.usdc.balance(&s.sponsor), proceeds - fee);

    // And only those 500 shares ever exist, so income divides over them.
    s.offering.claim_shares(&s.property, &s.alice);
    assert_eq!(s.shares.total_issued(&s.property), 500);
}

#[test]
fn test_shares_are_claimed_once_and_only_after_settlement() {
    let s = setup();
    open(&s);
    s.offering.subscribe(&s.property, &s.alice, &600);
    s.offering.subscribe(&s.property, &s.bob, &400);

    // Not while the sale is open.
    assert!(s.offering.try_claim_shares(&s.property, &s.alice).is_err());

    s.offering.close(&s.property);

    assert_eq!(s.offering.claim_shares(&s.property, &s.alice), 600);
    assert_eq!(s.shares.balance_of(&s.property, &s.alice), 600);
    // The subscription is spent, so a second claim finds nothing.
    assert!(s.offering.try_claim_shares(&s.property, &s.alice).is_err());
    assert_eq!(s.shares.balance_of(&s.property, &s.alice), 600);

    assert_eq!(s.offering.claim_shares(&s.property, &s.bob), 400);
    assert_eq!(s.shares.total_issued(&s.property), TOTAL_SHARES);

    // Somebody who never subscribed gets nothing.
    let stranger = Address::generate(&s.env);
    assert!(s.offering.try_claim_shares(&s.property, &stranger).is_err());
}

#[test]
fn test_a_raise_below_its_soft_cap_is_unwound_in_full() {
    let s = setup();
    open(&s);

    s.offering.subscribe(&s.property, &s.alice, &200);
    s.offering.subscribe(&s.property, &s.bob, &100);
    assert_eq!(s.usdc.balance(&s.offering.address), 30_000);

    s.env.ledger().set_timestamp(DURATION + 1);
    assert_eq!(s.offering.close(&s.property), SaleStatus::Failed);
    assert_eq!(
        s.registry.get_status(&s.property),
        sh_property_registry::PropertyStatus::Failed
    );

    // The sponsor is paid nothing and no shares exist.
    assert_eq!(s.usdc.balance(&s.sponsor), 0);
    assert_eq!(s.usdc.balance(&s.treasury), 0);
    assert_eq!(s.shares.total_issued(&s.property), 0);
    assert!(s.offering.try_claim_shares(&s.property, &s.alice).is_err());

    // Every subscriber is made whole, to the last unit.
    assert_eq!(s.offering.refund(&s.property, &s.alice), 20_000);
    assert_eq!(s.offering.refund(&s.property, &s.bob), 10_000);
    assert_eq!(s.usdc.balance(&s.alice), 1_000_000);
    assert_eq!(s.usdc.balance(&s.bob), 1_000_000);
    assert_eq!(s.usdc.balance(&s.offering.address), 0);

    // Refunds are taken once.
    assert!(s.offering.try_refund(&s.property, &s.alice).is_err());
}

#[test]
fn test_a_raise_with_no_subscribers_still_closes() {
    let s = setup();
    open(&s);

    s.env.ledger().set_timestamp(DURATION + 1);
    assert_eq!(s.offering.close(&s.property), SaleStatus::Failed);
    assert_eq!(s.usdc.balance(&s.offering.address), 0);
}

#[test]
fn test_closing_is_permissionless_but_decided_by_the_sales_own_state() {
    let s = setup();
    open(&s);
    s.offering.subscribe(&s.property, &s.alice, &500);

    // A stranger closes it, and the outcome is the one the state dictates.
    let stranger = Address::generate(&s.env);
    s.env.ledger().set_timestamp(DURATION + 1);
    let _ = stranger;
    assert_eq!(s.offering.close(&s.property), SaleStatus::Settled);

    // A sale closes once.
    assert!(s.offering.try_close(&s.property).is_err());
    assert!(!s.offering.is_closable(&s.property));
}

#[test]
fn test_a_closed_sale_takes_no_more_money() {
    let s = setup();
    open(&s);
    s.offering.subscribe(&s.property, &s.alice, &500);
    s.env.ledger().set_timestamp(DURATION + 1);
    s.offering.close(&s.property);

    assert!(s.offering.try_subscribe(&s.property, &s.bob, &100).is_err());
    assert!(s
        .offering
        .try_withdraw_subscription(&s.property, &s.alice, &100)
        .is_err());
    // Settled, not failed: there is nothing to refund.
    assert!(s.offering.try_refund(&s.property, &s.alice).is_err());
}

#[test]
fn test_subscriptions_stop_at_the_deadline() {
    let s = setup();
    open(&s);

    s.env.ledger().set_timestamp(DURATION);
    assert!(s
        .offering
        .try_subscribe(&s.property, &s.alice, &100)
        .is_err());
}

#[test]
fn test_unknown_sales_are_rejected() {
    let s = setup();
    assert!(s.offering.try_get_sale(&99).is_err());
    assert!(s.offering.try_subscribe(&99, &s.alice, &1).is_err());
    assert!(s.offering.try_close(&99).is_err());
}

#[test]
fn test_the_fee_is_capped_in_code_not_configuration() {
    let s = setup();
    let max = s.offering.get_max_fee_bps();

    // Rejected on the way into the queue, so an impossible fee never waits.
    assert!(s
        .offering
        .try_schedule_action(&s.admin, &Action::SetFeeBps(max + 1))
        .is_err());

    s.offering
        .schedule_action(&s.admin, &Action::SetFeeBps(max));
    s.env.ledger().set_timestamp(TIMELOCK);
    s.offering.execute_action(&s.admin);
    assert_eq!(s.offering.get_fee_bps(), max);
}

#[test]
fn test_fee_and_treasury_changes_wait_out_the_timelock() {
    let s = setup();
    let new_treasury = Address::generate(&s.env);

    s.env.ledger().set_timestamp(1_000);
    s.offering
        .schedule_action(&s.admin, &Action::SetTreasury(new_treasury.clone()));
    assert_eq!(
        s.offering.get_scheduled_action().unwrap().eta,
        1_000 + TIMELOCK
    );
    // Nothing changes while it waits.
    assert_eq!(s.offering.get_treasury(), s.treasury);
    assert!(s.offering.try_execute_action(&s.admin).is_err());

    s.env.ledger().set_timestamp(1_000 + TIMELOCK);
    s.offering.execute_action(&s.admin);
    assert_eq!(s.offering.get_treasury(), new_treasury);
    assert_eq!(s.offering.get_scheduled_action(), None);
}

#[test]
fn test_upgrade_and_admin_handover_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[8u8; 32]));

    assert!(s.offering.try_schedule_action(&stranger, &action).is_err());
    assert!(s.offering.try_execute_action(&stranger).is_err());
    assert!(s.offering.try_cancel_action(&stranger).is_err());
    assert!(s.offering.try_propose_admin(&stranger, &stranger).is_err());
}

#[test]
fn test_admin_handover_is_two_step() {
    let s = setup();
    let new_admin = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);

    s.offering.propose_admin(&s.admin, &new_admin);
    assert_eq!(s.offering.get_admin(), s.admin);
    assert!(s.offering.try_accept_admin(&stranger).is_err());

    s.offering.accept_admin(&new_admin);
    assert_eq!(s.offering.get_admin(), new_admin);
    assert!(s.offering.try_propose_admin(&s.admin, &s.admin).is_err());
}
