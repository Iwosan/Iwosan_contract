#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, Env, Symbol, TryFromVal, Vec,
};

fn setup() -> (Env, CredentialIssuerRegistryClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let admin = Address::generate(&env);
    let id = env.register(CredentialIssuerRegistry, (&admin,));
    let client = CredentialIssuerRegistryClient::new(&env, &id);
    (env, client, admin)
}

#[test]
fn registered_issuer_can_issue_a_valid_credential() {
    let (env, client, _admin) = setup();
    let nafdac = Address::generate(&env);
    let seller = Address::generate(&env);
    client.add_issuer(&nafdac);

    client.issue(
        &nafdac,
        &seller,
        &CredentialType::DrugSeller,
        &(env.ledger().timestamp() + 1_000),
    );

    assert!(client.has_valid_credential(&seller, &CredentialType::DrugSeller));
}

#[test]
fn credential_types_are_independent() {
    let (env, client, _admin) = setup();
    let council = Address::generate(&env);
    let nurse = Address::generate(&env);
    client.add_issuer(&council);

    client.issue(
        &council,
        &nurse,
        &CredentialType::Nurse,
        &(env.ledger().timestamp() + 1_000),
    );

    assert!(client.has_valid_credential(&nurse, &CredentialType::Nurse));
    assert!(!client.has_valid_credential(&nurse, &CredentialType::Doctor));
}

#[test]
fn expired_credentials_are_invalid() {
    let (env, client, _admin) = setup();
    let nafdac = Address::generate(&env);
    let seller = Address::generate(&env);
    client.add_issuer(&nafdac);
    let expires_at = env.ledger().timestamp() + 500;
    client.issue(&nafdac, &seller, &CredentialType::DrugSeller, &expires_at);

    env.ledger().set_timestamp(expires_at);

    assert!(!client.has_valid_credential(&seller, &CredentialType::DrugSeller));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn credentials_cannot_be_issued_already_expired() {
    let (env, client, _admin) = setup();
    let nafdac = Address::generate(&env);
    let seller = Address::generate(&env);
    client.add_issuer(&nafdac);

    client.issue(
        &nafdac,
        &seller,
        &CredentialType::DrugSeller,
        &(env.ledger().timestamp() - 1),
    );
}

#[test]
fn revoked_credentials_are_invalid() {
    let (env, client, _admin) = setup();
    let council = Address::generate(&env);
    let doctor = Address::generate(&env);
    client.add_issuer(&council);
    client.issue(
        &council,
        &doctor,
        &CredentialType::Doctor,
        &(env.ledger().timestamp() + 1_000),
    );

    client.revoke(&council, &doctor, &CredentialType::Doctor);

    assert!(!client.has_valid_credential(&doctor, &CredentialType::Doctor));
}

#[test]
fn admin_can_revoke_a_credential() {
    let (env, client, admin) = setup();
    let nafdac = Address::generate(&env);
    let seller = Address::generate(&env);
    client.add_issuer(&nafdac);
    client.issue(
        &nafdac,
        &seller,
        &CredentialType::DrugSeller,
        &(env.ledger().timestamp() + 1_000),
    );

    client.revoke(&admin, &seller, &CredentialType::DrugSeller);

    assert!(!client.has_valid_credential(&seller, &CredentialType::DrugSeller));
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn unrelated_callers_cannot_revoke() {
    let (env, client, _admin) = setup();
    let nafdac = Address::generate(&env);
    let other_body = Address::generate(&env);
    let seller = Address::generate(&env);
    client.add_issuer(&nafdac);
    client.add_issuer(&other_body);
    client.issue(
        &nafdac,
        &seller,
        &CredentialType::DrugSeller,
        &(env.ledger().timestamp() + 1_000),
    );

    client.revoke(&other_body, &seller, &CredentialType::DrugSeller);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn only_registered_issuers_can_issue() {
    let (env, client, _admin) = setup();
    let impostor = Address::generate(&env);
    let nurse = Address::generate(&env);

    client.issue(
        &impostor,
        &nurse,
        &CredentialType::Nurse,
        &(env.ledger().timestamp() + 1_000),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn removed_issuers_cannot_issue() {
    let (env, client, _admin) = setup();
    let council = Address::generate(&env);
    let nurse = Address::generate(&env);
    client.add_issuer(&council);
    client.remove_issuer(&council);

    client.issue(
        &council,
        &nurse,
        &CredentialType::Nurse,
        &(env.ledger().timestamp() + 1_000),
    );
}

#[test]
fn removing_an_issuer_keeps_existing_credentials() {
    let (env, client, _admin) = setup();
    let council = Address::generate(&env);
    let nurse = Address::generate(&env);
    client.add_issuer(&council);
    client.issue(
        &council,
        &nurse,
        &CredentialType::Nurse,
        &(env.ledger().timestamp() + 1_000),
    );

    client.remove_issuer(&council);

    assert!(!client.is_issuer(&council));
    assert!(client.has_valid_credential(&nurse, &CredentialType::Nurse));
}

fn event_names(env: &Env) -> Vec<Symbol> {
    let mut names = Vec::new(env);
    for (_, topics, _) in env.events().all().iter() {
        if let Some(topic) = topics.get(0) {
            if let Ok(name) = Symbol::try_from_val(env, &topic) {
                names.push_back(name);
            }
        }
    }
    names
}

#[test]
fn issuing_emits_an_issued_event() {
    let (env, client, _admin) = setup();
    let council = Address::generate(&env);
    let nurse = Address::generate(&env);
    client.add_issuer(&council);
    client.issue(
        &council,
        &nurse,
        &CredentialType::Nurse,
        &(env.ledger().timestamp() + 1_000),
    );

    assert!(event_names(&env).contains(Symbol::new(&env, "issued")));
}

#[test]
fn revoking_emits_a_revoked_event() {
    let (env, client, _admin) = setup();
    let council = Address::generate(&env);
    let nurse = Address::generate(&env);
    client.add_issuer(&council);
    client.issue(
        &council,
        &nurse,
        &CredentialType::Nurse,
        &(env.ledger().timestamp() + 1_000),
    );
    client.revoke(&council, &nurse, &CredentialType::Nurse);

    assert!(event_names(&env).contains(Symbol::new(&env, "revoked")));
}

#[test]
fn unknown_subjects_have_no_valid_credential() {
    let (env, client, _admin) = setup();
    let stranger = Address::generate(&env);

    assert!(!client.has_valid_credential(&stranger, &CredentialType::LabScientist));
}
