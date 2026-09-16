#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, BytesN, Env,
};

const TIMELOCK: u64 = 172_800;

struct Setup<'a> {
    env: Env,
    pool: LendingPoolContractClient<'a>,
    usdc: token::Client<'a>,
    admin: Address,
    mortgage_pool: Address,
    alice: Address,
    bob: Address,
    builder: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let mortgage_pool = Address::generate(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    let builder = Address::generate(&env);

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let usdc = token::Client::new(&env, &sac.address());
    let minter = token::StellarAssetClient::new(&env, &sac.address());
    minter.mint(&alice, &1_000_000);
    minter.mint(&bob, &1_000_000);
    // The borrower needs funds to repay with.
    minter.mint(&mortgage_pool, &1_000_000);

    let id = env.register(
        LendingPoolContract,
        (admin.clone(), sac.address(), TIMELOCK),
    );
    let pool = LendingPoolContractClient::new(&env, &id);
    pool.set_mortgage_pool(&admin, &mortgage_pool);

    Setup {
        env,
        pool,
        usdc,
        admin,
        mortgage_pool,
        alice,
        bob,
        builder,
    }
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.pool.get_admin(), s.admin);
    assert_eq!(s.pool.get_timelock_secs(), TIMELOCK);
    assert_eq!(s.pool.get_settlement_token(), s.usdc.address);
    assert_eq!(s.pool.get_mortgage_pool(), Some(s.mortgage_pool.clone()));
    assert_eq!(s.pool.available(), 0);
}

#[test]
fn test_deposit_and_withdraw() {
    let s = setup();

    s.pool.deposit(&s.alice, &100_000);
    assert_eq!(s.pool.shares_of(&s.alice), 100_000);
    assert_eq!(s.pool.available(), 100_000);
    assert_eq!(s.usdc.balance(&s.pool.address), 100_000);
    assert_eq!(s.usdc.balance(&s.alice), 900_000);

    s.pool.withdraw(&s.alice, &40_000);
    assert_eq!(s.pool.shares_of(&s.alice), 60_000);
    assert_eq!(s.usdc.balance(&s.alice), 940_000);

    assert!(s.pool.try_withdraw(&s.alice, &60_001).is_err());
    assert!(s.pool.try_deposit(&s.alice, &0).is_err());
    assert!(s.pool.try_deposit(&s.alice, &-1).is_err());
}

#[test]
fn test_committed_capital_cannot_be_withdrawn() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);

    s.pool.reserve(&s.mortgage_pool, &80_000);
    assert_eq!(s.pool.available(), 20_000);

    // Alice still holds all her shares, but the committed part is spoken for.
    assert_eq!(s.pool.shares_of(&s.alice), 100_000);
    assert!(s.pool.try_withdraw(&s.alice, &20_001).is_err());
    s.pool.withdraw(&s.alice, &20_000);

    // The pool still holds the committed money, ready for the next tranche.
    assert_eq!(s.usdc.balance(&s.pool.address), 80_000);
}

#[test]
fn test_a_pool_cannot_commit_what_it_does_not_have() {
    let s = setup();
    s.pool.deposit(&s.alice, &50_000);

    assert!(s.pool.try_reserve(&s.mortgage_pool, &50_001).is_err());
    s.pool.reserve(&s.mortgage_pool, &50_000);
    assert!(s.pool.try_reserve(&s.mortgage_pool, &1).is_err());

    // Releasing a commitment makes it available again.
    s.pool.unreserve(&s.mortgage_pool, &20_000);
    assert_eq!(s.pool.available(), 20_000);
    assert!(s.pool.try_unreserve(&s.mortgage_pool, &30_001).is_err());
}

#[test]
fn test_tranches_are_paid_from_committed_capital_only() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);

    // Nothing committed yet.
    assert!(s
        .pool
        .try_disburse(&s.mortgage_pool, &s.builder, &10_000)
        .is_err());

    s.pool.reserve(&s.mortgage_pool, &50_000);
    s.pool.disburse(&s.mortgage_pool, &s.builder, &20_000);

    assert_eq!(s.usdc.balance(&s.builder), 20_000);
    let state = s.pool.pool_state();
    assert_eq!(state.total_capital, 80_000);
    assert_eq!(state.total_reserved, 30_000);
    assert_eq!(state.total_lent, 20_000);
    // Uncommitted capital is untouched by the draw.
    assert_eq!(s.pool.available(), 50_000);

    assert!(s
        .pool
        .try_disburse(&s.mortgage_pool, &s.builder, &30_001)
        .is_err());
}

#[test]
fn test_only_the_mortgage_pool_moves_capital() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);
    let stranger = Address::generate(&s.env);

    assert!(s.pool.try_reserve(&stranger, &1_000).is_err());
    assert!(s.pool.try_reserve(&s.admin, &1_000).is_err());
    assert!(s.pool.try_unreserve(&stranger, &1_000).is_err());
    assert!(s.pool.try_disburse(&stranger, &stranger, &1_000).is_err());
    assert!(s.pool.try_write_off(&stranger, &1_000).is_err());
    assert!(s
        .pool
        .try_repay(&stranger, &s.mortgage_pool, &100, &10)
        .is_err());
}

#[test]
fn test_repayment_returns_principal_and_pays_interest_out() {
    let s = setup();
    s.pool.deposit(&s.alice, &75_000);
    s.pool.deposit(&s.bob, &25_000);

    s.pool.reserve(&s.mortgage_pool, &40_000);
    s.pool.disburse(&s.mortgage_pool, &s.builder, &40_000);
    assert_eq!(s.pool.pool_state().total_lent, 40_000);

    // The borrower repays 10,000 of principal and 4,000 of interest.
    s.pool
        .repay(&s.mortgage_pool, &s.mortgage_pool, &10_000, &4_000);

    let state = s.pool.pool_state();
    assert_eq!(state.total_lent, 30_000);
    assert_eq!(state.total_capital, 70_000);
    assert_eq!(state.total_interest, 4_000);

    // Interest splits by shareholding, three quarters to one quarter.
    assert_eq!(s.pool.claimable_interest(&s.alice), 3_000);
    assert_eq!(s.pool.claimable_interest(&s.bob), 1_000);

    assert_eq!(s.pool.claim_interest(&s.alice), 3_000);
    assert_eq!(s.usdc.balance(&s.alice), 925_000 + 3_000);
    // Claimed once.
    assert_eq!(s.pool.claimable_interest(&s.alice), 0);
    assert!(s.pool.try_claim_interest(&s.alice).is_err());
}

#[test]
fn test_interest_earned_before_a_deposit_is_not_shared_with_it() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);
    s.pool.reserve(&s.mortgage_pool, &50_000);
    s.pool.disburse(&s.mortgage_pool, &s.builder, &50_000);

    s.pool.repay(&s.mortgage_pool, &s.mortgage_pool, &0, &1_000);
    assert_eq!(s.pool.claimable_interest(&s.alice), 1_000);

    // Bob arrives after that interest was earned.
    s.pool.deposit(&s.bob, &100_000);
    assert_eq!(s.pool.claimable_interest(&s.bob), 0);
    assert_eq!(s.pool.claimable_interest(&s.alice), 1_000);

    // The next payment splits evenly between them.
    s.pool.repay(&s.mortgage_pool, &s.mortgage_pool, &0, &2_000);
    assert_eq!(s.pool.claimable_interest(&s.alice), 2_000);
    assert_eq!(s.pool.claimable_interest(&s.bob), 1_000);
}

#[test]
fn test_withdrawing_does_not_forfeit_interest_already_earned() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);
    s.pool.repay(&s.mortgage_pool, &s.mortgage_pool, &0, &5_000);

    // Alice pulls all her capital out without claiming first.
    s.pool.withdraw(&s.alice, &100_000);
    assert_eq!(s.pool.shares_of(&s.alice), 0);
    assert_eq!(s.pool.claimable_interest(&s.alice), 5_000);
    assert_eq!(s.pool.claim_interest(&s.alice), 5_000);
}

#[test]
fn test_interest_cannot_be_farmed_by_churning_deposits() {
    let s = setup();
    s.pool.deposit(&s.alice, &60_000);
    s.pool.deposit(&s.bob, &40_000);
    s.pool
        .repay(&s.mortgage_pool, &s.mortgage_pool, &0, &10_000);

    // Moving the same capital in and out must not mint entitlement.
    for _ in 0..5 {
        s.pool.withdraw(&s.alice, &60_000);
        s.pool.deposit(&s.alice, &60_000);
    }

    let alice = s.pool.claimable_interest(&s.alice);
    let bob = s.pool.claimable_interest(&s.bob);
    assert_eq!(alice, 6_000);
    assert_eq!(bob, 4_000);
    assert_eq!(alice + bob, 10_000);
}

#[test]
fn test_the_pool_always_holds_enough_to_cover_what_it_owes() {
    let s = setup();
    // Three equal investors and interest that never divides by three.
    let carol = Address::generate(&s.env);
    token::StellarAssetClient::new(&s.env, &s.usdc.address).mint(&carol, &1_000);
    s.pool.deposit(&s.alice, &1);
    s.pool.deposit(&s.bob, &1);
    s.pool.deposit(&carol, &1);

    for _ in 0..10 {
        s.pool.repay(&s.mortgage_pool, &s.mortgage_pool, &0, &7);
    }
    let held = s.usdc.balance(&s.pool.address);

    let paid = s.pool.claim_interest(&s.alice)
        + s.pool.claim_interest(&s.bob)
        + s.pool.claim_interest(&carol);

    // It can never pay out more than it holds; what does not divide is carried
    // into the next payment rather than going missing.
    assert!(paid <= 70, "paid {paid} exceeds the 70 received");
    assert_eq!(s.usdc.balance(&s.pool.address), held - paid);
}

#[test]
fn test_a_default_falls_on_capital_not_on_interest_already_earned() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);
    s.pool.reserve(&s.mortgage_pool, &60_000);
    s.pool.disburse(&s.mortgage_pool, &s.builder, &30_000);
    s.pool.repay(&s.mortgage_pool, &s.mortgage_pool, &0, &2_000);

    // The borrower walks away with 30,000 drawn; the undrawn half is released.
    s.pool.unreserve(&s.mortgage_pool, &30_000);
    s.pool.write_off(&s.mortgage_pool, &30_000);

    let state = s.pool.pool_state();
    assert_eq!(state.total_lent, 0);
    assert_eq!(state.total_written_off, 30_000);
    assert_eq!(state.total_reserved, 0);

    // Interest already earned is still Alice's.
    assert_eq!(s.pool.claimable_interest(&s.alice), 2_000);
    assert_eq!(s.pool.claim_interest(&s.alice), 2_000);
}

#[test]
fn test_interest_with_no_investors_stays_in_the_pool() {
    let s = setup();
    // Nobody has deposited, so there is nobody to credit.
    s.pool.repay(&s.mortgage_pool, &s.mortgage_pool, &0, &500);
    assert_eq!(s.pool.pool_state().total_capital, 500);
    assert_eq!(s.usdc.balance(&s.pool.address), 500);
}

#[test]
fn test_repayments_must_carry_something() {
    let s = setup();
    assert!(s
        .pool
        .try_repay(&s.mortgage_pool, &s.mortgage_pool, &0, &0)
        .is_err());
    assert!(s
        .pool
        .try_repay(&s.mortgage_pool, &s.mortgage_pool, &-1, &5)
        .is_err());
}

#[test]
fn test_wiring_is_set_once() {
    let s = setup();
    let other = Address::generate(&s.env);
    assert!(s.pool.try_set_mortgage_pool(&s.admin, &other).is_err());
    assert_eq!(s.pool.get_mortgage_pool(), Some(s.mortgage_pool.clone()));
}

#[test]
fn test_there_is_no_admin_route_to_the_capital() {
    let s = setup();
    s.pool.deposit(&s.alice, &100_000);

    // The admin holds no shares, so there is nothing to claim or withdraw.
    assert_eq!(s.pool.shares_of(&s.admin), 0);
    assert!(s.pool.try_withdraw(&s.admin, &1).is_err());
    assert!(s.pool.try_claim_interest(&s.admin).is_err());
    assert_eq!(s.usdc.balance(&s.pool.address), 100_000);
}

#[test]
fn test_upgrades_wait_out_the_timelock() {
    let s = setup();
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[6u8; 32]));

    s.env.ledger().set_timestamp(900);
    s.pool.schedule_action(&s.admin, &action);
    assert_eq!(s.pool.get_scheduled_action().unwrap().eta, 900 + TIMELOCK);
    assert!(s.pool.try_schedule_action(&s.admin, &action).is_err());

    s.env.ledger().set_timestamp(900 + TIMELOCK - 1);
    assert!(s.pool.try_execute_action(&s.admin).is_err());

    s.pool.cancel_action(&s.admin);
    assert_eq!(s.pool.get_scheduled_action(), None);
}

#[test]
fn test_admin_handover_is_two_step() {
    let s = setup();
    let new_admin = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);

    s.pool.propose_admin(&s.admin, &new_admin);
    assert_eq!(s.pool.get_admin(), s.admin);
    assert!(s.pool.try_accept_admin(&stranger).is_err());

    s.pool.accept_admin(&new_admin);
    assert_eq!(s.pool.get_admin(), new_admin);
    assert!(s.pool.try_propose_admin(&s.admin, &s.admin).is_err());
}
