#![cfg(test)]

use super::*;
use sh_lending_pool::{LendingPoolContract, LendingPoolContractClient};
use sh_property_registry::{PropertyRegistryContract, PropertyRegistryContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, BytesN, Env,
};

const TIMELOCK: u64 = 172_800;
const GRACE: u64 = 1_209_600; // 14 days
const MONTH: u64 = 30 * 24 * 60 * 60;
const VALUATION: i128 = 100_000;
const PRINCIPAL: i128 = 50_000;
const TERM: u32 = 120;
const RATE_BPS: u32 = 850; // 8.5%, matching the backend's default

struct Setup<'a> {
    env: Env,
    pool: MortgagePoolContractClient<'a>,
    registry: PropertyRegistryContractClient<'a>,
    lending: LendingPoolContractClient<'a>,
    usdc: token::Client<'a>,
    admin: Address,
    underwriter: Address,
    trustee: Address,
    oracle: Address,
    borrower: Address,
    investor: Address,
    property: u64,
}

fn setup<'a>() -> Setup<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000_000);

    let admin = Address::generate(&env);
    let underwriter = Address::generate(&env);
    let trustee = Address::generate(&env);
    let oracle = Address::generate(&env);
    let borrower = Address::generate(&env);
    let investor = Address::generate(&env);

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let usdc = token::Client::new(&env, &sac.address());
    let minter = token::StellarAssetClient::new(&env, &sac.address());
    minter.mint(&investor, &1_000_000);
    minter.mint(&borrower, &1_000_000);

    let registry_id = env.register(PropertyRegistryContract, (admin.clone(), TIMELOCK));
    let registry = PropertyRegistryContractClient::new(&env, &registry_id);

    let lending_id = env.register(
        LendingPoolContract,
        (admin.clone(), sac.address(), TIMELOCK),
    );
    let lending = LendingPoolContractClient::new(&env, &lending_id);

    let pool_id = env.register(
        MortgagePoolContract,
        (
            admin.clone(),
            registry_id.clone(),
            lending_id.clone(),
            GRACE,
            TIMELOCK,
        ),
    );
    let pool = MortgagePoolContractClient::new(&env, &pool_id);

    registry.set_mortgage_pool(&admin, &pool_id);
    lending.set_mortgage_pool(&admin, &pool_id);
    registry.set_trustee(&admin, &trustee, &true);
    registry.set_oracle(&admin, &oracle, &true);
    pool.set_underwriter(&admin, &underwriter, &true);

    // Capital to lend, and a verified, valued property to lend against.
    lending.deposit(&investor, &500_000);
    let property = registry.submit_property(
        &trustee,
        &BytesN::from_array(&env, &[1u8; 32]),
        &BytesN::from_array(&env, &[2u8; 32]),
    );
    registry.verify_title(&oracle, &property);
    registry.set_valuation(&oracle, &property, &VALUATION);

    Setup {
        env,
        pool,
        registry,
        lending,
        usdc,
        admin,
        underwriter,
        trustee,
        oracle,
        borrower,
        investor,
        property,
    }
}

/// Sign off stage `stage`, ready for its tranche to be drawn.
fn sign_off(s: &Setup, stage: u32) {
    s.registry.submit_milestone_evidence(
        &s.trustee,
        &s.property,
        &stage,
        &BytesN::from_array(&s.env, &[10 + stage as u8; 32]),
    );
    s.registry.verify_milestone(&s.oracle, &s.property, &stage);
}

/// An approved mortgage, nothing drawn yet.
fn approved(s: &Setup) -> u64 {
    let id = s
        .pool
        .apply(&s.borrower, &s.property, &PRINCIPAL, &TERM, &RATE_BPS);
    s.pool.approve(&s.underwriter, &id);
    id
}

/// An approved mortgage with the first tranche drawn.
fn funded(s: &Setup) -> u64 {
    let id = approved(s);
    sign_off(s, 0);
    s.pool.disburse(&id, &0);
    id
}

#[test]
fn test_constructor_sets_configuration_at_deploy() {
    let s = setup();
    assert_eq!(s.pool.get_admin(), s.admin);
    assert_eq!(s.pool.get_grace_secs(), GRACE);
    assert_eq!(s.pool.get_timelock_secs(), TIMELOCK);
    assert_eq!(s.pool.get_seconds_per_month(), MONTH);
}

#[test]
fn test_an_application_records_the_terms_without_committing_anything() {
    let s = setup();
    let id = s
        .pool
        .apply(&s.borrower, &s.property, &PRINCIPAL, &TERM, &RATE_BPS);

    let m = s.pool.get_mortgage(&id);
    assert_eq!(m.borrower, s.borrower);
    assert_eq!(m.principal, PRINCIPAL);
    assert_eq!(m.term_months, TERM);
    assert_eq!(m.rate_bps, RATE_BPS);
    assert_eq!(m.status, MortgageStatus::Applied);
    assert_eq!(m.outstanding, 0);
    assert_eq!(m.disbursed, 0);

    // Nothing is committed until an underwriter approves.
    assert_eq!(s.lending.available(), 500_000);
    assert_eq!(s.pool.mortgage_for_property(&s.property), Some(id));
}

#[test]
fn test_lending_is_capped_at_the_valuation() {
    let s = setup();
    let ceiling = VALUATION * s.pool.get_max_ltv_bps() / 10_000;

    assert!(s
        .pool
        .try_apply(&s.borrower, &s.property, &(ceiling + 1), &TERM, &RATE_BPS)
        .is_err());
    // Exactly at the ceiling is fine.
    let id = s
        .pool
        .apply(&s.borrower, &s.property, &ceiling, &TERM, &RATE_BPS);
    assert_eq!(s.pool.get_mortgage(&id).principal, ceiling);
}

#[test]
fn test_applications_are_refused_on_unverified_property_and_bad_terms() {
    let s = setup();

    // A property whose title has not been checked.
    let draft = s.registry.submit_property(
        &s.trustee,
        &BytesN::from_array(&s.env, &[3u8; 32]),
        &BytesN::from_array(&s.env, &[4u8; 32]),
    );
    assert!(s
        .pool
        .try_apply(&s.borrower, &draft, &1_000, &TERM, &RATE_BPS)
        .is_err());

    assert!(s
        .pool
        .try_apply(&s.borrower, &s.property, &0, &TERM, &RATE_BPS)
        .is_err());
    assert!(s
        .pool
        .try_apply(&s.borrower, &s.property, &PRINCIPAL, &0, &RATE_BPS)
        .is_err());
    // Above the rate ceiling.
    let max = s.pool.get_max_rate_bps();
    assert!(s
        .pool
        .try_apply(&s.borrower, &s.property, &PRINCIPAL, &TERM, &(max + 1))
        .is_err());
}

#[test]
fn test_one_property_backs_one_mortgage() {
    let s = setup();
    let id = approved(&s);

    let other = Address::generate(&s.env);
    assert!(s
        .pool
        .try_apply(&other, &s.property, &1_000, &TERM, &RATE_BPS)
        .is_err());

    // A declined application frees the property again.
    let _ = id;
    let fresh = setup();
    let declined = fresh.pool.apply(
        &fresh.borrower,
        &fresh.property,
        &PRINCIPAL,
        &TERM,
        &RATE_BPS,
    );
    fresh.pool.decline(&fresh.underwriter, &declined);
    assert_eq!(fresh.pool.mortgage_for_property(&fresh.property), None);
    fresh.pool.apply(
        &fresh.borrower,
        &fresh.property,
        &PRINCIPAL,
        &TERM,
        &RATE_BPS,
    );
}

#[test]
fn test_approval_commits_the_whole_facility_before_anything_is_built() {
    let s = setup();
    let id = approved(&s);

    assert_eq!(s.pool.get_mortgage(&id).status, MortgageStatus::Approved);
    // The investor's capital is spoken for, so a borrower whose foundation
    // passes cannot find the money withdrawn.
    assert_eq!(s.lending.available(), 500_000 - PRINCIPAL);
    assert!(s.lending.try_withdraw(&s.investor, &500_000).is_err());
    s.lending.withdraw(&s.investor, &450_000);

    // Nothing has reached anyone yet.
    assert_eq!(s.usdc.balance(&s.trustee), 0);
    assert_eq!(s.pool.get_mortgage(&id).outstanding, 0);
}

#[test]
fn test_approval_is_underwriter_only_and_happens_once() {
    let s = setup();
    let id = s
        .pool
        .apply(&s.borrower, &s.property, &PRINCIPAL, &TERM, &RATE_BPS);
    let stranger = Address::generate(&s.env);

    assert!(s.pool.try_approve(&stranger, &id).is_err());
    assert!(s.pool.try_approve(&s.admin, &id).is_err());
    assert!(s.pool.try_approve(&s.borrower, &id).is_err());

    s.pool.approve(&s.underwriter, &id);
    assert!(s.pool.try_approve(&s.underwriter, &id).is_err());
    assert!(s.pool.try_decline(&s.underwriter, &id).is_err());
}

#[test]
fn test_money_follows_the_building() {
    let s = setup();
    let id = approved(&s);
    let tranche = PRINCIPAL / MILESTONE_COUNT as i128;

    // Nothing is releasable until an inspector signs the stage off.
    assert!(s.pool.try_disburse(&id, &0).is_err());

    sign_off(&s, 0);
    assert_eq!(s.pool.disburse(&id, &0), tranche);

    // The money goes to the trustee running the build, never the borrower.
    assert_eq!(s.usdc.balance(&s.trustee), tranche);
    assert_eq!(s.usdc.balance(&s.borrower), 1_000_000);

    let m = s.pool.get_mortgage(&id);
    assert_eq!(m.status, MortgageStatus::Funded);
    assert_eq!(m.disbursed, tranche);
    assert_eq!(m.outstanding, tranche);
    assert_eq!(
        s.registry.get_status_of(&s.property),
        sh_property_registry::PropertyStatus::Mortgaged
    );

    // A stage draws once, and the next one is still locked.
    assert!(s.pool.try_disburse(&id, &0).is_err());
    assert!(s.pool.try_disburse(&id, &1).is_err());
}

#[test]
fn test_the_whole_facility_is_drawable_across_the_stages() {
    let s = setup();
    let id = approved(&s);

    let mut drawn = 0;
    for stage in 0..MILESTONE_COUNT {
        sign_off(&s, stage);
        drawn += s.pool.disburse(&id, &stage);
    }

    // Rounding does not strand any of the facility.
    assert_eq!(drawn, PRINCIPAL);
    assert_eq!(s.usdc.balance(&s.trustee), PRINCIPAL);
    assert_eq!(s.pool.get_mortgage(&id).disbursed, PRINCIPAL);
    assert_eq!(s.lending.pool_state().total_lent, PRINCIPAL);
    assert_eq!(s.lending.pool_state().total_reserved, 0);
}

#[test]
fn test_disbursement_is_permissionless_but_state_driven() {
    let s = setup();
    let id = approved(&s);
    sign_off(&s, 0);

    // A stranger draws it; the tranche still goes to the trustee.
    let stranger = Address::generate(&s.env);
    let _ = stranger;
    s.pool.disburse(&id, &0);
    assert_eq!(s.usdc.balance(&s.trustee), PRINCIPAL / 5);
}

#[test]
fn test_interest_is_charged_only_on_what_has_been_drawn() {
    let s = setup();
    let id = funded(&s);
    let tranche = PRINCIPAL / 5;

    // One month on the first tranche only.
    s.env.ledger().set_timestamp(1_000_000 + MONTH);
    let (outstanding, interest) = s.pool.current_balance(&id);
    assert_eq!(outstanding, tranche);
    // 10,000 * 8.5% / 12 = 70 (truncated).
    assert_eq!(interest, tranche * RATE_BPS as i128 / (10_000 * 12));

    // The undrawn four fifths cost nothing.
    assert!(interest < PRINCIPAL * RATE_BPS as i128 / (10_000 * 12));
}

#[test]
fn test_interest_accrues_in_whole_months_and_never_twice() {
    let s = setup();
    let id = funded(&s);
    let monthly = (PRINCIPAL / 5) * RATE_BPS as i128 / (10_000 * 12);

    // Part of a month costs nothing yet.
    s.env.ledger().set_timestamp(1_000_000 + MONTH - 1);
    assert_eq!(s.pool.current_balance(&id).1, 0);

    s.env.ledger().set_timestamp(1_000_000 + MONTH);
    assert_eq!(s.pool.current_balance(&id).1, monthly);

    // Reading it repeatedly does not charge it repeatedly.
    assert_eq!(s.pool.current_balance(&id).1, monthly);

    s.env.ledger().set_timestamp(1_000_000 + 3 * MONTH);
    // Three months costs slightly more than three truncated single months,
    // because the carry gives back what each division rounded away.
    assert!(s.pool.current_balance(&id).1 >= monthly * 3);
}

#[test]
fn test_interest_does_not_depend_on_how_often_it_is_charged() {
    // A borrower who pokes the contract every month must not end up owing less
    // than one who leaves it alone, so accrual has to be path-independent.
    let stepped = setup();
    let a = funded(&stepped);
    for month in 1..=6u64 {
        stepped
            .env
            .ledger()
            .set_timestamp(1_000_000 + month * MONTH);
        // Reading the balance settles the accrual for that month.
        stepped.pool.current_balance(&a);
    }
    let charged_monthly = stepped.pool.current_balance(&a).1;

    let untouched = setup();
    let b = funded(&untouched);
    untouched.env.ledger().set_timestamp(1_000_000 + 6 * MONTH);
    let charged_at_once = untouched.pool.current_balance(&b).1;

    assert_eq!(charged_monthly, charged_at_once);
}

#[test]
fn test_a_payment_clears_interest_before_it_touches_principal() {
    let s = setup();
    let id = funded(&s);
    let tranche = PRINCIPAL / 5;
    s.env.ledger().set_timestamp(1_000_000 + MONTH);

    let interest = s.pool.current_balance(&id).1;
    let due = s.pool.amount_due(&id);
    // The instalment is the month's interest plus a slice of principal.
    assert_eq!(due, interest + PRINCIPAL / TERM as i128);

    s.pool.repay(&id, &due);

    let m = s.pool.get_mortgage(&id);
    assert_eq!(m.interest_accrued, 0);
    assert_eq!(m.interest_paid, interest);
    assert_eq!(m.outstanding, tranche - PRINCIPAL / TERM as i128);
    assert_eq!(m.payments_made, 1);
    assert_eq!(m.status, MortgageStatus::Repaying);

    // The investor earns the interest.
    assert_eq!(s.lending.claimable_interest(&s.investor), interest);
}

#[test]
fn test_a_payment_short_of_the_instalment_is_refused() {
    let s = setup();
    let id = funded(&s);
    s.env.ledger().set_timestamp(1_000_000 + MONTH);

    let due = s.pool.amount_due(&id);
    assert!(s.pool.try_repay(&id, &(due - 1)).is_err());
    assert!(s.pool.try_repay(&id, &0).is_err());
    s.pool.repay(&id, &due);
}

#[test]
fn test_paying_more_than_due_shortens_the_loan() {
    let s = setup();
    let id = funded(&s);
    s.env.ledger().set_timestamp(1_000_000 + MONTH);

    let before = s.pool.get_mortgage(&id).outstanding;
    let due = s.pool.amount_due(&id);
    s.pool.repay(&id, &(due + 5_000));

    let m = s.pool.get_mortgage(&id);
    // Everything above the instalment went to principal.
    assert_eq!(m.outstanding, before - (due - m.interest_paid) - 5_000);
}

#[test]
fn test_a_loan_can_be_paid_off_outright_at_any_time() {
    let s = setup();
    let id = funded(&s);
    s.env.ledger().set_timestamp(1_000_000 + MONTH);

    let payoff = s.pool.payoff_amount(&id);
    s.pool.repay(&id, &payoff);

    let m = s.pool.get_mortgage(&id);
    assert_eq!(m.status, MortgageStatus::PaidOff);
    assert_eq!(m.outstanding, 0);
    assert_eq!(m.interest_accrued, 0);

    // The property is released and the undrawn facility returned to the pool.
    assert_eq!(
        s.registry.get_status_of(&s.property),
        sh_property_registry::PropertyStatus::Repaid
    );
    assert_eq!(s.pool.mortgage_for_property(&s.property), None);
    assert_eq!(s.lending.pool_state().total_reserved, 0);
    assert_eq!(s.lending.pool_state().total_lent, 0);

    // A closed loan takes no more money.
    assert!(s.pool.try_repay(&id, &100).is_err());
}

#[test]
fn test_overpaying_a_payoff_takes_only_what_is_owed() {
    let s = setup();
    let id = funded(&s);
    s.env.ledger().set_timestamp(1_000_000 + MONTH);

    let payoff = s.pool.payoff_amount(&id);
    let before = s.usdc.balance(&s.borrower);
    s.pool.repay(&id, &(payoff * 2));

    // Only the payoff was taken.
    assert_eq!(s.usdc.balance(&s.borrower), before - payoff);
    assert_eq!(s.pool.get_mortgage(&id).status, MortgageStatus::PaidOff);
}

#[test]
fn test_only_the_borrower_repays() {
    let s = setup();
    let id = funded(&s);
    s.env.ledger().set_timestamp(1_000_000 + MONTH);

    // With auths mocked the call succeeds, so assert the money comes from the
    // borrower's account and nobody else's.
    let due = s.pool.amount_due(&id);
    let stranger = Address::generate(&s.env);
    let before = s.usdc.balance(&s.borrower);
    s.pool.repay(&id, &due);
    assert_eq!(s.usdc.balance(&s.borrower), before - due);
    assert_eq!(s.usdc.balance(&stranger), 0);
}

#[test]
fn test_a_loan_defaults_only_after_grace_has_run_out() {
    let s = setup();
    let id = funded(&s);

    // Current.
    assert!(!s.pool.is_defaultable(&id));
    assert!(s.pool.try_mark_default(&id).is_err());

    // Late, but inside grace.
    s.env.ledger().set_timestamp(1_000_000 + MONTH + GRACE);
    assert!(!s.pool.is_defaultable(&id));
    assert!(s.pool.try_mark_default(&id).is_err());

    // Past grace: anyone may write it off.
    s.env.ledger().set_timestamp(1_000_000 + MONTH + GRACE + 1);
    assert!(s.pool.is_defaultable(&id));
    s.pool.mark_default(&id);

    let m = s.pool.get_mortgage(&id);
    assert_eq!(m.status, MortgageStatus::Defaulted);
    assert_eq!(
        s.registry.get_status_of(&s.property),
        sh_property_registry::PropertyStatus::Defaulted
    );

    // The drawn balance is written off; the undrawn facility goes back.
    let state = s.lending.pool_state();
    assert_eq!(state.total_written_off, PRINCIPAL / 5);
    assert_eq!(state.total_reserved, 0);
    assert_eq!(state.total_lent, 0);

    // And it cannot be defaulted or repaid twice.
    assert!(s.pool.try_mark_default(&id).is_err());
    assert!(s.pool.try_repay(&id, &1_000).is_err());
}

#[test]
fn test_paying_on_time_keeps_a_loan_out_of_default() {
    let s = setup();
    let id = funded(&s);

    for month in 1..4u64 {
        s.env.ledger().set_timestamp(1_000_000 + month * MONTH);
        let due = s.pool.amount_due(&id);
        s.pool.repay(&id, &due);
        assert!(!s.pool.is_defaultable(&id));
    }
    assert_eq!(s.pool.get_mortgage(&id).payments_made, 3);
}

#[test]
fn test_an_undrawn_loan_cannot_default() {
    let s = setup();
    let id = approved(&s);

    // Nothing has been released, so no instalment has fallen due.
    s.env.ledger().set_timestamp(1_000_000 + 12 * MONTH);
    assert!(!s.pool.is_defaultable(&id));
    assert!(s.pool.try_mark_default(&id).is_err());
}

#[test]
fn test_the_pool_is_repaid_what_the_borrower_pays() {
    let s = setup();
    let id = funded(&s);
    let tranche = PRINCIPAL / 5;

    s.env.ledger().set_timestamp(1_000_000 + MONTH);
    let payoff = s.pool.payoff_amount(&id);
    let interest = s.pool.current_balance(&id).1;
    s.pool.repay(&id, &payoff);

    let state = s.lending.pool_state();
    // Principal came back to capital; interest is the investors' yield.
    assert_eq!(state.total_lent, 0);
    assert_eq!(state.total_capital, 500_000 - tranche + tranche);
    assert_eq!(state.total_interest, interest);
    assert_eq!(s.lending.claim_interest(&s.investor), interest);
}

#[test]
fn test_grace_changes_wait_out_the_timelock() {
    let s = setup();
    let action = Action::SetGraceSecs(GRACE * 2);

    s.pool.schedule_action(&s.admin, &action);
    assert_eq!(s.pool.get_grace_secs(), GRACE);
    assert!(s.pool.try_execute_action(&s.admin).is_err());

    s.env.ledger().set_timestamp(1_000_000 + TIMELOCK);
    s.pool.execute_action(&s.admin);
    assert_eq!(s.pool.get_grace_secs(), GRACE * 2);

    // A zero grace period is refused on the way in, not left to fail later.
    assert!(s
        .pool
        .try_schedule_action(&s.admin, &Action::SetGraceSecs(0))
        .is_err());
}

#[test]
fn test_upgrade_and_admin_handover_are_admin_only() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let action = Action::Upgrade(BytesN::from_array(&s.env, &[7u8; 32]));

    assert!(s.pool.try_schedule_action(&stranger, &action).is_err());
    assert!(s.pool.try_execute_action(&stranger).is_err());
    assert!(s.pool.try_cancel_action(&stranger).is_err());
    assert!(s.pool.try_propose_admin(&stranger, &stranger).is_err());
    assert!(s
        .pool
        .try_set_underwriter(&stranger, &stranger, &true)
        .is_err());
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

#[test]
fn test_unknown_mortgages_are_rejected() {
    let s = setup();
    assert!(s.pool.try_get_mortgage(&99).is_err());
    assert!(s.pool.try_repay(&99, &100).is_err());
    assert!(s.pool.try_disburse(&99, &0).is_err());
    assert!(s.pool.try_mark_default(&99).is_err());
}
