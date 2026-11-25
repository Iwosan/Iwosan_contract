#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    vec, Address, Env,
};

const CONTRIBUTION: i128 = 5_000;
const PERIOD: u64 = 30 * 24 * 60 * 60;

struct Setup {
    env: Env,
    client: HealthSavingsPoolClient<'static>,
    token: token::Client<'static>,
    members: Vec<Address>,
    pool: u64,
}

/// A pool of three members, each funded with enough for several rounds.
fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin).address();
    let id = env.register(HealthSavingsPool, ());
    let client = HealthSavingsPoolClient::new(&env, &id);

    let members = vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    for member in members.iter() {
        StellarAssetClient::new(&env, &token_id).mint(&member, &(CONTRIBUTION * 5));
    }
    let organiser = Address::generate(&env);
    let pool = client.create_pool(&organiser, &token_id, &CONTRIBUTION, &PERIOD, &members);

    Setup {
        token: token::Client::new(&env, &token_id),
        env,
        client,
        members,
        pool,
    }
}

fn everyone_contributes(s: &Setup) {
    for member in s.members.iter() {
        s.client.contribute(&s.pool, &member);
    }
}

fn balance(s: &Setup, index: u32) -> i128 {
    s.token.balance(&s.members.get_unchecked(index))
}

#[test]
fn full_rotation_pays_each_member_once_in_order() {
    let s = setup();

    for round in 0..3 {
        assert_eq!(
            s.client.next_recipient(&s.pool),
            s.members.get_unchecked(round)
        );
        everyone_contributes(&s);
        s.client.payout(&s.pool);
    }

    // Everyone paid 3 contributions and received one pot of 3 contributions.
    for index in 0..3 {
        assert_eq!(balance(&s, index), CONTRIBUTION * 5);
    }
    let pool = s.client.get_pool(&s.pool);
    assert_eq!(pool.round, 3);
    assert_eq!(pool.pot, 0);
    // The rotation starts again with the first member.
    assert_eq!(s.client.next_recipient(&s.pool), s.members.get_unchecked(0));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn member_cannot_contribute_twice_in_a_round() {
    let s = setup();
    let member = s.members.get_unchecked(0);

    s.client.contribute(&s.pool, &member);
    s.client.contribute(&s.pool, &member);
}

#[test]
fn member_contributes_again_in_the_next_round() {
    let s = setup();
    let member = s.members.get_unchecked(1);
    everyone_contributes(&s);
    s.client.payout(&s.pool);

    assert!(!s.client.has_contributed(&s.pool, &member));
    s.client.contribute(&s.pool, &member);
    assert!(s.client.has_contributed(&s.pool, &member));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_member_cannot_contribute() {
    let s = setup();
    let stranger = Address::generate(&s.env);

    s.client.contribute(&s.pool, &stranger);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn payout_waits_for_everyone_during_the_period() {
    let s = setup();
    s.client.contribute(&s.pool, &s.members.get_unchecked(0));

    s.client.payout(&s.pool);
}

#[test]
fn payout_after_the_period_pays_what_was_collected() {
    let s = setup();
    s.client.contribute(&s.pool, &s.members.get_unchecked(1));
    s.client.contribute(&s.pool, &s.members.get_unchecked(2));
    s.env.ledger().set_timestamp(1_000 + PERIOD);

    s.client.payout(&s.pool);

    assert_eq!(balance(&s, 0), CONTRIBUTION * 5 + CONTRIBUTION * 2);
    assert_eq!(s.client.get_pool(&s.pool).round, 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn duplicate_members_are_rejected() {
    let s = setup();
    let member = s.members.get_unchecked(0);
    let members = vec![&s.env, member.clone(), member];

    s.client.create_pool(
        &Address::generate(&s.env),
        &s.token.address,
        &CONTRIBUTION,
        &PERIOD,
        &members,
    );
}

#[test]
fn emergency_withdrawal_is_paid_after_majority_approval() {
    let s = setup();
    everyone_contributes(&s);
    let patient = s.members.get_unchecked(2);

    let request = s.client.request_emergency(&s.pool, &patient, &8_000);
    // One of three approvals (the requester's own) is not a majority.
    assert!(!s.client.get_request(&s.pool, &request).paid);
    assert_eq!(balance(&s, 2), CONTRIBUTION * 4);

    s.client
        .approve_emergency(&s.pool, &request, &s.members.get_unchecked(0));

    assert!(s.client.get_request(&s.pool, &request).paid);
    assert_eq!(balance(&s, 2), CONTRIBUTION * 4 + 8_000);
    assert_eq!(s.client.get_pool(&s.pool).pot, CONTRIBUTION * 3 - 8_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn member_cannot_approve_twice() {
    let s = setup();
    everyone_contributes(&s);
    let patient = s.members.get_unchecked(2);
    let request = s.client.request_emergency(&s.pool, &patient, &1_000);

    s.client.approve_emergency(&s.pool, &request, &patient);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn emergency_cannot_exceed_the_pot() {
    let s = setup();
    s.client.contribute(&s.pool, &s.members.get_unchecked(0));

    s.client
        .request_emergency(&s.pool, &s.members.get_unchecked(0), &(CONTRIBUTION + 1));
}
