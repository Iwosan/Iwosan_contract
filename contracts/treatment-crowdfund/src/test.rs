#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    Address, BytesN, Env,
};

const GOAL: i128 = 10_000;
const DEADLINE: u64 = 10_000;
const NOW: u64 = 1_000;

struct Setup {
    env: Env,
    client: TreatmentCrowdfundClient<'static>,
    token: token::Client<'static>,
    token_id: Address,
    patient: Address,
    hospital: Address,
    campaign: u64,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(NOW);
    let admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin).address();
    let id = env.register(TreatmentCrowdfund, (&token_id,));
    let client = TreatmentCrowdfundClient::new(&env, &id);

    let patient = Address::generate(&env);
    let hospital = Address::generate(&env);
    let campaign = client.create_campaign(
        &patient,
        &hospital,
        &GOAL,
        &DEADLINE,
        &BytesN::from_array(&env, &[7; 32]),
    );

    Setup {
        token: token::Client::new(&env, &token_id),
        token_id,
        env,
        client,
        patient,
        hospital,
        campaign,
    }
}

fn fund(s: &Setup, donor: &Address, amount: i128) {
    StellarAssetClient::new(&s.env, &s.token_id).mint(donor, &amount);
}

#[test]
fn donations_are_released_to_the_hospital_not_the_patient() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, GOAL);

    s.client.donate(&donor, &s.campaign, &6_000);
    s.client.donate(&donor, &s.campaign, &4_000);
    assert_eq!(s.client.get_campaign(&s.campaign).raised, GOAL);

    s.client.release(&s.campaign);

    assert_eq!(s.token.balance(&s.hospital), GOAL);
    assert_eq!(s.token.balance(&s.patient), 0);
    assert_eq!(s.token.balance(&donor), 0);
    let campaign = s.client.get_campaign(&s.campaign);
    assert!(campaign.released);
    assert_eq!(campaign.raised, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn donations_cannot_exceed_the_goal() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, GOAL + 1);

    s.client.donate(&donor, &s.campaign, &(GOAL - 1));
    s.client.donate(&donor, &s.campaign, &2);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn release_before_the_goal_is_met_fails() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, 5_000);
    s.client.donate(&donor, &s.campaign, &5_000);

    s.client.release(&s.campaign);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn release_happens_once() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, GOAL);
    s.client.donate(&donor, &s.campaign, &GOAL);

    s.client.release(&s.campaign);
    assert_eq!(s.token.balance(&s.hospital), GOAL);
    s.client.release(&s.campaign);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn donation_after_the_deadline_fails() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, 1_000);

    s.env.ledger().set_timestamp(DEADLINE);
    s.client.donate(&donor, &s.campaign, &1_000);
}

#[test]
fn donor_refunds_once_after_a_missed_deadline() {
    let s = setup();
    let donor = Address::generate(&s.env);
    let other = Address::generate(&s.env);
    fund(&s, &donor, 3_000);
    fund(&s, &other, 2_000);
    s.client.donate(&donor, &s.campaign, &3_000);
    s.client.donate(&other, &s.campaign, &2_000);

    s.env.ledger().set_timestamp(DEADLINE);
    s.client.refund(&donor, &s.campaign);
    assert_eq!(s.token.balance(&donor), 3_000);
    assert!(s.client.has_refunded(&s.campaign, &donor));
    assert_eq!(s.client.contribution(&s.campaign, &donor), 0);

    // Another donor's money is untouched and still refundable.
    assert_eq!(s.token.balance(&other), 0);
    s.client.refund(&other, &s.campaign);
    assert_eq!(s.token.balance(&other), 2_000);
    assert_eq!(s.token.balance(&s.hospital), 0);
    assert_eq!(s.client.get_campaign(&s.campaign).raised, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn a_donor_cannot_refund_twice() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, 3_000);
    s.client.donate(&donor, &s.campaign, &3_000);

    s.env.ledger().set_timestamp(DEADLINE);
    s.client.refund(&donor, &s.campaign);
    s.client.refund(&donor, &s.campaign);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn refunds_are_not_available_before_the_deadline() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, 3_000);
    s.client.donate(&donor, &s.campaign, &3_000);

    s.client.refund(&donor, &s.campaign);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn refunds_are_closed_once_the_goal_is_met() {
    let s = setup();
    let donor = Address::generate(&s.env);
    fund(&s, &donor, GOAL);
    s.client.donate(&donor, &s.campaign, &GOAL);

    s.env.ledger().set_timestamp(DEADLINE);
    s.client.refund(&donor, &s.campaign);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn non_donor_cannot_refund() {
    let s = setup();
    s.env.ledger().set_timestamp(DEADLINE);
    let stranger = Address::generate(&s.env);
    s.client.refund(&stranger, &s.campaign);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn campaign_needs_a_positive_goal() {
    let s = setup();
    let patient = Address::generate(&s.env);
    let hospital = Address::generate(&s.env);
    s.client.create_campaign(
        &patient,
        &hospital,
        &0,
        &DEADLINE,
        &BytesN::from_array(&s.env, &[1; 32]),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn campaign_needs_a_future_deadline() {
    let s = setup();
    let patient = Address::generate(&s.env);
    let hospital = Address::generate(&s.env);
    s.client.create_campaign(
        &patient,
        &hospital,
        &GOAL,
        &NOW,
        &BytesN::from_array(&s.env, &[1; 32]),
    );
}
