#![cfg(test)]

use super::*;
use sh_share_ledger::{ShareLedgerContract, ShareLedgerContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, BytesN, Env,
};

const TIMELOCK: u64 = 172_800;
const PROPERTY: u64 = 1;

struct Setup<'a> {
    env: Env,
    income: IncomeDistributorContractClient<'a>,
    shares: ShareLedgerContractClient<'a>,
    usdc: token::Client<'a>,
    admin: Address,
    offering: Address,
    sponsor: Address,
    alice: Address,
    bob: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let offering = Address::generate(&env);
    let sponsor = Address::generate(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let usdc = token::Client::new(&env, &sac.address());
    token::StellarAssetClient::new(&env, &sac.address()).mint(&sponsor, &1_000_000);

    let ledger_id = env.register(ShareLedgerContract, (admin.clone(), TIMELOCK));
    let shares = ShareLedgerContractClient::new(&env, &ledger_id);

    let income_id = env.register(
        IncomeDistributorContract,
        (admin.clone(), ledger_id.clone(), sac.address(), TIMELOCK),
    );
    let income = IncomeDistributorContractClient::new(&env, &income_id);

    shares.set_offering(&admin, &offering);
    shares.set_distributor(&admin, &income_id);

    Setup {
        env,
        income,
        shares,
        usdc,
        admin,
        offering,
        sponsor,
        alice,
        bob,
    }
}

/// Put a cap table in place: Alice holds 3/4, Bob 1/4.
fn issued(s: &Setup) {
    s.shares.issue(&s.offering, &PROPERTY, &s.alice, &750);
    s.shares.issue(&s.offering, &PROPERTY, &s.bob, &250);
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.income.get_admin(), s.admin);
    assert_eq!(s.income.get_timelock_secs(), TIMELOCK);
    assert_eq!(s.income.get_settlement_token(), s.usdc.address);
    assert_eq!(s.income.total_deposited(&PROPERTY), 0);
    assert_eq!(s.income.get_scheduled_action(), None);
}

#[test]
fn test_rent_is_escrowed_and_claimed_pro_rata() {
    let s = setup();
    issued(&s);

    s.income.deposit_income(&PROPERTY, &s.sponsor, &4_000);

    // The money is held here until holders come for it.
    assert_eq!(s.usdc.balance(&s.income.address), 4_000);
    assert_eq!(s.usdc.balance(&s.sponsor), 996_000);
    assert_eq!(s.income.total_deposited(&PROPERTY), 4_000);
    assert_eq!(s.income.unclaimed(&PROPERTY), 4_000);

    assert_eq!(s.income.claimable(&PROPERTY, &s.alice), 3_000);
    assert_eq!(s.income.claimable(&PROPERTY, &s.bob), 1_000);

    assert_eq!(s.income.claim(&PROPERTY, &s.alice), 3_000);
    assert_eq!(s.usdc.balance(&s.alice), 3_000);
    assert_eq!(s.usdc.balance(&s.income.address), 1_000);

    assert_eq!(s.income.claim(&PROPERTY, &s.bob), 1_000);
    assert_eq!(s.usdc.balance(&s.bob), 1_000);

    // Everything deposited has now been collected.
    assert_eq!(s.usdc.balance(&s.income.address), 0);
    assert_eq!(s.income.total_claimed(&PROPERTY), 4_000);
    assert_eq!(s.income.unclaimed(&PROPERTY), 0);
}

#[test]
fn test_income_cannot_be_claimed_twice() {
    let s = setup();
    issued(&s);
    s.income.deposit_income(&PROPERTY, &s.sponsor, &4_000);

    s.income.claim(&PROPERTY, &s.alice);
    // The ledger zeroed the position, so there is nothing left to take.
    assert!(s.income.try_claim(&PROPERTY, &s.alice).is_err());
    assert_eq!(s.usdc.balance(&s.alice), 3_000);
}

#[test]
fn test_someone_who_holds_nothing_can_claim_nothing() {
    let s = setup();
    issued(&s);
    s.income.deposit_income(&PROPERTY, &s.sponsor, &4_000);

    let stranger = Address::generate(&s.env);
    assert_eq!(s.income.claimable(&PROPERTY, &stranger), 0);
    assert!(s.income.try_claim(&PROPERTY, &stranger).is_err());
}

#[test]
fn test_deposits_pile_up_and_are_collected_in_one_go() {
    let s = setup();
    issued(&s);

    // Twelve months of rent, never collected.
    for _ in 0..12 {
        s.income.deposit_income(&PROPERTY, &s.sponsor, &1_000);
    }
    assert_eq!(s.income.total_deposited(&PROPERTY), 12_000);

    assert_eq!(s.income.claim(&PROPERTY, &s.alice), 9_000);
    assert_eq!(s.income.claim(&PROPERTY, &s.bob), 3_000);
    assert_eq!(s.usdc.balance(&s.income.address), 0);
}

#[test]
fn test_a_deposit_with_nobody_to_pay_moves_no_money() {
    let s = setup();
    // No shares issued for this property yet.
    assert!(s
        .income
        .try_deposit_income(&PROPERTY, &s.sponsor, &1_000)
        .is_err());

    // The sponsor's balance is untouched: the ledger refused before the
    // transfer, so the money never left.
    assert_eq!(s.usdc.balance(&s.sponsor), 1_000_000);
    assert_eq!(s.usdc.balance(&s.income.address), 0);
    assert_eq!(s.income.total_deposited(&PROPERTY), 0);
}

#[test]
fn test_deposits_are_open_to_anyone_but_must_be_positive() {
    let s = setup();
    issued(&s);

    // A managing agent, not the sponsor, pays the rent in.
    let agent = Address::generate(&s.env);
    token::StellarAssetClient::new(&s.env, &s.usdc.address).mint(&agent, &5_000);
    s.income.deposit_income(&PROPERTY, &agent, &5_000);
    assert_eq!(s.income.claimable(&PROPERTY, &s.alice), 3_750);

    assert!(s.income.try_deposit_income(&PROPERTY, &agent, &0).is_err());
    assert!(s.income.try_deposit_income(&PROPERTY, &agent, &-1).is_err());
}

#[test]
fn test_income_follows_the_cap_table_through_a_sale() {
    let s = setup();
    issued(&s);

    s.income.deposit_income(&PROPERTY, &s.sponsor, &4_000);
    // Alice sells her whole stake to Bob without claiming first.
    s.shares.transfer(&PROPERTY, &s.alice, &s.bob, &750);

    s.income.deposit_income(&PROPERTY, &s.sponsor, &4_000);

    // Alice keeps her share of the first month only; Bob has all of the second
    // plus his quarter of the first.
    assert_eq!(s.income.claim(&PROPERTY, &s.alice), 3_000);
    assert_eq!(s.income.claim(&PROPERTY, &s.bob), 5_000);
    assert_eq!(s.usdc.balance(&s.income.address), 0);
}

#[test]
fn test_the_contract_always_holds_enough_to_cover_what_it_owes() {
    let s = setup();
    // Three equal holders and rent that never divides by three.
    let carol = Address::generate(&s.env);
    s.shares.issue(&s.offering, &PROPERTY, &s.alice, &1);
    s.shares.issue(&s.offering, &PROPERTY, &s.bob, &1);
    s.shares.issue(&s.offering, &PROPERTY, &carol, &1);

    for _ in 0..10 {
        s.income.deposit_income(&PROPERTY, &s.sponsor, &7);
    }

    let held_before = s.usdc.balance(&s.income.address);
    assert_eq!(held_before, 70);

    let paid = s.income.claim(&PROPERTY, &s.alice)
        + s.income.claim(&PROPERTY, &s.bob)
        + s.income.claim(&PROPERTY, &carol);

    // It can never pay out more than it holds; what it cannot divide stays put
    // for the next deposit rather than going missing.
    assert!(paid <= held_before);
    assert_eq!(s.usdc.balance(&s.income.address), held_before - paid);
    assert_eq!(s.income.unclaimed(&PROPERTY), held_before - paid);
}

#[test]
fn test_there_is_no_admin_route_to_the_money() {
    let s = setup();
    issued(&s);
    s.income.deposit_income(&PROPERTY, &s.sponsor, &4_000);

    // The admin holds no shares, so even the admin claims nothing.
    assert_eq!(s.income.claimable(&PROPERTY, &s.admin), 0);
    assert!(s.income.try_claim(&PROPERTY, &s.admin).is_err());
    assert_eq!(s.usdc.balance(&s.income.address), 4_000);
}

#[test]
fn test_upgrades_wait_out_the_timelock() {
    let s = setup();
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[2u8; 32]));

    s.env.ledger().set_timestamp(500);
    s.income.schedule_action(&s.admin, &action);
    assert_eq!(s.income.get_scheduled_action().unwrap().eta, 500 + TIMELOCK);

    assert!(s.income.try_schedule_action(&s.admin, &action).is_err());
    s.env.ledger().set_timestamp(500 + TIMELOCK - 1);
    assert!(s.income.try_execute_action(&s.admin).is_err());

    s.income.cancel_action(&s.admin);
    assert_eq!(s.income.get_scheduled_action(), None);
    assert!(s.income.try_cancel_action(&s.admin).is_err());
}

#[test]
fn test_upgrade_and_admin_handover_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[5u8; 32]));

    assert!(s.income.try_schedule_action(&stranger, &action).is_err());
    assert!(s.income.try_propose_admin(&stranger, &stranger).is_err());
    assert!(s.income.try_accept_admin(&stranger).is_err());
}

#[test]
fn test_admin_handover_is_two_step() {
    let s = setup();
    let new_admin = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);

    s.income.propose_admin(&s.admin, &new_admin);
    assert_eq!(s.income.get_admin(), s.admin);
    assert!(s.income.try_accept_admin(&stranger).is_err());

    s.income.accept_admin(&new_admin);
    assert_eq!(s.income.get_admin(), new_admin);
    assert!(s.income.try_propose_admin(&s.admin, &s.admin).is_err());
}
