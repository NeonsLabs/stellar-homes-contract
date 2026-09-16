#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    BytesN, Env,
};

const TIMELOCK: u64 = 172_800;
const PROPERTY: u64 = 1;
const OTHER_PROPERTY: u64 = 2;

struct Setup<'a> {
    env: Env,
    ledger: ShareLedgerContractClient<'a>,
    admin: Address,
    offering: Address,
    distributor: Address,
    alice: Address,
    bob: Address,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let offering = Address::generate(&env);
    let distributor = Address::generate(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);

    let id = env.register(ShareLedgerContract, (admin.clone(), TIMELOCK));
    let ledger = ShareLedgerContractClient::new(&env, &id);
    ledger.set_offering(&admin, &offering);
    ledger.set_distributor(&admin, &distributor);

    Setup {
        env,
        ledger,
        admin,
        offering,
        distributor,
        alice,
        bob,
    }
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.ledger.get_admin(), s.admin);
    assert_eq!(s.ledger.get_timelock_secs(), TIMELOCK);
    assert_eq!(s.ledger.get_offering(), Some(s.offering.clone()));
    assert_eq!(s.ledger.get_distributor(), Some(s.distributor.clone()));
    assert_eq!(s.ledger.total_issued(&PROPERTY), 0);
}

#[test]
fn test_only_the_offering_can_issue_shares() {
    let s = setup();
    let stranger = Address::generate(&s.env);

    assert!(s
        .ledger
        .try_issue(&stranger, &PROPERTY, &s.alice, &100)
        .is_err());
    assert!(s
        .ledger
        .try_issue(&s.admin, &PROPERTY, &s.alice, &100)
        .is_err());
    assert!(s
        .ledger
        .try_issue(&s.distributor, &PROPERTY, &s.alice, &100)
        .is_err());

    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &100);
    assert_eq!(s.ledger.balance_of(&PROPERTY, &s.alice), 100);
    assert_eq!(s.ledger.total_issued(&PROPERTY), 100);

    // Zero and negative issuance is rejected.
    assert!(s
        .ledger
        .try_issue(&s.offering, &PROPERTY, &s.alice, &0)
        .is_err());
    assert!(s
        .ledger
        .try_issue(&s.offering, &PROPERTY, &s.alice, &-5)
        .is_err());
}

#[test]
fn test_properties_are_isolated() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &100);
    s.ledger.issue(&s.offering, &OTHER_PROPERTY, &s.bob, &400);

    assert_eq!(s.ledger.balance_of(&PROPERTY, &s.bob), 0);
    assert_eq!(s.ledger.balance_of(&OTHER_PROPERTY, &s.alice), 0);
    assert_eq!(s.ledger.total_issued(&PROPERTY), 100);
    assert_eq!(s.ledger.total_issued(&OTHER_PROPERTY), 400);

    // Income on one property never reaches the other's holders.
    s.ledger.accrue(&s.distributor, &PROPERTY, &1_000);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 1_000);
    assert_eq!(s.ledger.accrued_of(&OTHER_PROPERTY, &s.bob), 0);
}

#[test]
fn test_transfer_moves_shares_and_needs_the_sellers_signature() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &1_000);

    s.ledger.transfer(&PROPERTY, &s.alice, &s.bob, &300);
    assert_eq!(s.ledger.balance_of(&PROPERTY, &s.alice), 700);
    assert_eq!(s.ledger.balance_of(&PROPERTY, &s.bob), 300);
    // Transfers move shares between holders; they never create or destroy any.
    assert_eq!(s.ledger.total_issued(&PROPERTY), 1_000);

    // Cannot send shares nobody holds.
    assert!(s
        .ledger
        .try_transfer(&PROPERTY, &s.alice, &s.bob, &701)
        .is_err());
    assert!(s
        .ledger
        .try_transfer(&PROPERTY, &s.alice, &s.bob, &0)
        .is_err());
    // A self-transfer would settle the same position twice.
    assert!(s
        .ledger
        .try_transfer(&PROPERTY, &s.alice, &s.alice, &1)
        .is_err());
}

#[test]
fn test_income_is_split_in_proportion_to_holdings() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &750);
    s.ledger.issue(&s.offering, &PROPERTY, &s.bob, &250);

    s.ledger.accrue(&s.distributor, &PROPERTY, &4_000);

    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 3_000);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.bob), 1_000);

    // Claiming pays out once and leaves nothing behind.
    assert_eq!(
        s.ledger.take_accrued(&s.distributor, &PROPERTY, &s.alice),
        3_000
    );
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 0);
    assert_eq!(
        s.ledger.take_accrued(&s.distributor, &PROPERTY, &s.alice),
        0
    );

    // Bob's entitlement is untouched by Alice claiming hers.
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.bob), 1_000);
}

#[test]
fn test_shares_bought_after_a_deposit_earn_nothing_from_it() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &1_000);
    s.ledger.accrue(&s.distributor, &PROPERTY, &5_000);

    // Bob buys in after the rent landed.
    s.ledger.transfer(&PROPERTY, &s.alice, &s.bob, &500);

    // Alice keeps the whole of the first deposit; Bob gets none of it.
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 5_000);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.bob), 0);

    // The next month's rent is split by the new cap table.
    s.ledger.accrue(&s.distributor, &PROPERTY, &5_000);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 7_500);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.bob), 2_500);
}

#[test]
fn test_selling_shares_does_not_forfeit_income_already_earned() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &1_000);
    s.ledger.accrue(&s.distributor, &PROPERTY, &3_000);

    // Alice sells out entirely without claiming first.
    s.ledger.transfer(&PROPERTY, &s.alice, &s.bob, &1_000);
    assert_eq!(s.ledger.balance_of(&PROPERTY, &s.alice), 0);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 3_000);

    // Later rent belongs entirely to Bob, but Alice can still collect hers.
    s.ledger.accrue(&s.distributor, &PROPERTY, &1_000);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.bob), 1_000);
    assert_eq!(
        s.ledger.take_accrued(&s.distributor, &PROPERTY, &s.alice),
        3_000
    );
}

#[test]
fn test_income_cannot_be_farmed_by_churning_shares() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &600);
    s.ledger.issue(&s.offering, &PROPERTY, &s.bob, &400);
    s.ledger.accrue(&s.distributor, &PROPERTY, &10_000);

    // Passing the same shares back and forth must not mint entitlement.
    for _ in 0..5 {
        s.ledger.transfer(&PROPERTY, &s.alice, &s.bob, &600);
        s.ledger.transfer(&PROPERTY, &s.bob, &s.alice, &600);
    }

    let alice = s.ledger.accrued_of(&PROPERTY, &s.alice);
    let bob = s.ledger.accrued_of(&PROPERTY, &s.bob);
    assert_eq!(alice, 6_000);
    assert_eq!(bob, 4_000);
    // Nothing was created: the two claims still sum to what was deposited.
    assert_eq!(alice + bob, 10_000);
}

#[test]
fn test_payouts_never_exceed_what_was_deposited() {
    let s = setup();
    // 3 holders and 7 units of rent: nothing divides evenly.
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &1);
    s.ledger.issue(&s.offering, &PROPERTY, &s.bob, &1);
    let carol = Address::generate(&s.env);
    s.ledger.issue(&s.offering, &PROPERTY, &carol, &1);

    let mut deposited = 0i128;
    for _ in 0..12 {
        s.ledger.accrue(&s.distributor, &PROPERTY, &7);
        deposited += 7;
    }

    let paid = s.ledger.take_accrued(&s.distributor, &PROPERTY, &s.alice)
        + s.ledger.take_accrued(&s.distributor, &PROPERTY, &s.bob)
        + s.ledger.take_accrued(&s.distributor, &PROPERTY, &carol);

    // The contract may owe slightly less than it holds, never more: it can
    // only pay out money the distributor actually received.
    assert!(
        paid <= deposited,
        "paid {paid} exceeds deposited {deposited}"
    );
    // The carry keeps truncation from compounding, so the shortfall stays
    // under one unit per holder rather than growing with each deposit.
    assert!(deposited - paid < 3, "stranded {} units", deposited - paid);
}

#[test]
fn test_the_carry_holds_indivisible_rent_until_it_can_be_paid() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &3);

    // One unit across three shares does not divide; it is kept, not dropped.
    s.ledger.accrue(&s.distributor, &PROPERTY, &1);
    assert!(s.ledger.carry_of_property(&PROPERTY) > 0);

    // Enough deposits later, the carry has been absorbed and Alice — who holds
    // every share — is owed the entire amount deposited.
    for _ in 0..2 {
        s.ledger.accrue(&s.distributor, &PROPERTY, &1);
    }
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 3);
    assert_eq!(s.ledger.carry_of_property(&PROPERTY), 0);
}

#[test]
fn test_income_is_refused_before_any_shares_exist() {
    let s = setup();
    // Rent arriving before an offering settles has nobody to belong to.
    assert!(s
        .ledger
        .try_accrue(&s.distributor, &PROPERTY, &1_000)
        .is_err());

    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &10);
    s.ledger.accrue(&s.distributor, &PROPERTY, &1_000);
    assert_eq!(s.ledger.accrued_of(&PROPERTY, &s.alice), 1_000);
}

#[test]
fn test_only_the_distributor_touches_income() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &100);

    assert!(s.ledger.try_accrue(&stranger, &PROPERTY, &100).is_err());
    assert!(s.ledger.try_accrue(&s.admin, &PROPERTY, &100).is_err());
    assert!(s.ledger.try_accrue(&s.offering, &PROPERTY, &100).is_err());
    assert!(s.ledger.try_accrue(&s.distributor, &PROPERTY, &0).is_err());

    assert!(s
        .ledger
        .try_take_accrued(&stranger, &PROPERTY, &s.alice)
        .is_err());
    assert!(s
        .ledger
        .try_take_accrued(&s.alice, &PROPERTY, &s.alice)
        .is_err());
}

#[test]
fn test_wiring_is_set_once() {
    let s = setup();
    let other = Address::generate(&s.env);
    assert!(s.ledger.try_set_offering(&s.admin, &other).is_err());
    assert!(s.ledger.try_set_distributor(&s.admin, &other).is_err());
    assert_eq!(s.ledger.get_offering(), Some(s.offering.clone()));
    assert_eq!(s.ledger.get_distributor(), Some(s.distributor.clone()));
}

#[test]
fn test_positions_are_extended_by_use() {
    let s = setup();
    s.ledger.issue(&s.offering, &PROPERTY, &s.alice, &100);

    let position = s.ledger.position_of_holder(&PROPERTY, &s.alice);
    assert_eq!(position.balance, 100);
    assert_eq!(position.credited, 0);
    assert_eq!(position.reward_debt, 0);

    s.ledger.accrue(&s.distributor, &PROPERTY, &500);
    s.ledger.transfer(&PROPERTY, &s.alice, &s.bob, &50);

    let position = s.ledger.position_of_holder(&PROPERTY, &s.alice);
    assert_eq!(position.balance, 50);
    assert_eq!(position.credited, 500);
    // Re-anchored against the smaller balance.
    assert_eq!(
        position.reward_debt,
        50 * s.ledger.acc_per_share(&PROPERTY) / SCALE
    );
}

#[test]
fn test_upgrades_wait_out_the_timelock() {
    let s = setup();
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[9u8; 32]));

    s.env.ledger().set_timestamp(2_000);
    s.ledger.schedule_action(&s.admin, &action);
    assert_eq!(
        s.ledger.get_scheduled_action().unwrap().eta,
        2_000 + TIMELOCK
    );

    assert!(s.ledger.try_schedule_action(&s.admin, &action).is_err());
    s.env.ledger().set_timestamp(2_000 + TIMELOCK - 1);
    assert!(s.ledger.try_execute_action(&s.admin).is_err());

    s.ledger.cancel_action(&s.admin);
    assert_eq!(s.ledger.get_scheduled_action(), None);
}

#[test]
fn test_upgrade_and_admin_handover_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[1u8; 32]));

    assert!(s.ledger.try_schedule_action(&stranger, &action).is_err());
    assert!(s.ledger.try_propose_admin(&stranger, &stranger).is_err());
    assert!(s.ledger.try_accept_admin(&stranger).is_err());
}

#[test]
fn test_admin_handover_is_two_step() {
    let s = setup();
    let new_admin = Address::generate(&s.env);
    let stranger = Address::generate(&s.env);

    s.ledger.propose_admin(&s.admin, &new_admin);
    assert_eq!(s.ledger.get_admin(), s.admin);
    assert!(s.ledger.try_accept_admin(&stranger).is_err());

    s.ledger.accept_admin(&new_admin);
    assert_eq!(s.ledger.get_admin(), new_admin);
    assert!(s.ledger.try_propose_admin(&s.admin, &s.admin).is_err());
}
