#![cfg(test)]

use super::*;
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    Address, BytesN, Env,
};

const PREMIUM: i128 = 1_000;
const PERIOD: u64 = 30 * 24 * 60 * 60;
const ANNUAL_LIMIT: i128 = 20_000;
const NOW: u64 = 1_000;

/// Stands in for `credential-issuer-registry` so tests can grant or withhold
/// a licence without deploying the real issuer workflow.
#[contracttype]
enum MockKey {
    Valid(Address, CredentialType),
}

#[contract]
struct MockRegistry;

#[contractimpl]
impl MockRegistry {
    pub fn set_valid(env: Env, provider: Address, credential_type: CredentialType, valid: bool) {
        env.storage()
            .instance()
            .set(&MockKey::Valid(provider, credential_type), &valid);
    }

    pub fn has_valid_credential(
        env: Env,
        subject: Address,
        credential_type: CredentialType,
    ) -> bool {
        env.storage()
            .instance()
            .get(&MockKey::Valid(subject, credential_type))
            .unwrap_or(false)
    }
}

struct Setup {
    env: Env,
    client: MicroInsurancePoolClient<'static>,
    token: token::Client<'static>,
    token_id: Address,
    provider: Address,
    member: Address,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(NOW);
    let admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let id = env.register(
        MicroInsurancePool,
        (&admin, &token_id, &PREMIUM, &PERIOD, &ANNUAL_LIMIT),
    );
    let client = MicroInsurancePoolClient::new(&env, &id);

    let registry = env.register(MockRegistry, ());
    client.set_credential_registry(&registry);
    let provider = Address::generate(&env);
    MockRegistryClient::new(&env, &registry).set_valid(&provider, &CredentialType::Doctor, &true);

    let member = Address::generate(&env);
    StellarAssetClient::new(&env, &token_id).mint(&member, &(PREMIUM * 12));

    Setup {
        client,
        token: token::Client::new(&env, &token_id),
        token_id,
        env,
        provider,
        member,
    }
}

fn hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[3; 32])
}

/// Subscribes `count` extra members so the pool holds enough for a payout.
fn top_up_pool(s: &Setup, count: u32) {
    let assets = StellarAssetClient::new(&s.env, &s.token_id);
    for _ in 0..count {
        let member = Address::generate(&s.env);
        assets.mint(&member, &PREMIUM);
        s.client.subscribe(&member);
    }
}

#[test]
fn subscribe_activates_coverage_and_collects_the_premium() {
    let s = setup();
    s.client.subscribe(&s.member);

    assert!(s.client.is_covered(&s.member));
    assert_eq!(s.client.coverage_until(&s.member), NOW + PERIOD);
    assert_eq!(s.token.balance(&s.member), PREMIUM * 11);
    assert_eq!(s.token.balance(&s.client.address), PREMIUM);
}

#[test]
fn renewing_extends_coverage_without_losing_remaining_time() {
    let s = setup();
    s.client.subscribe(&s.member);
    s.client.subscribe(&s.member);

    assert_eq!(s.client.coverage_until(&s.member), NOW + PERIOD * 2);
    assert_eq!(s.token.balance(&s.client.address), PREMIUM * 2);
}

#[test]
fn approved_claim_pays_the_provider() {
    let s = setup();
    s.client.subscribe(&s.member);
    top_up_pool(&s, 4);
    let claim = s
        .client
        .submit_claim(&s.provider, &s.member, &5_000, &hash(&s.env));

    s.client.approve_claim(&claim);

    assert_eq!(s.token.balance(&s.provider), 5_000);
    assert_eq!(s.token.balance(&s.member), PREMIUM * 11);
    assert_eq!(s.client.claimed_this_year(&s.member), 5_000);
    assert_eq!(s.client.get_claim(&claim).status, ClaimStatus::Paid);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn claims_for_lapsed_members_fail() {
    let s = setup();
    s.client.subscribe(&s.member);
    s.env.ledger().set_timestamp(NOW + PERIOD + 1);

    s.client
        .submit_claim(&s.provider, &s.member, &5_000, &hash(&s.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn payouts_never_exceed_the_pool_balance() {
    let s = setup();
    s.client.subscribe(&s.member);
    let claim = s
        .client
        .submit_claim(&s.provider, &s.member, &5_000, &hash(&s.env));

    // Only one premium is in the pool.
    s.client.approve_claim(&claim);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn annual_claim_limit_is_enforced() {
    let s = setup();
    s.client.subscribe(&s.member);
    s.client
        .submit_claim(&s.provider, &s.member, &15_000, &hash(&s.env));

    s.client
        .submit_claim(&s.provider, &s.member, &6_000, &hash(&s.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn unlicensed_providers_cannot_submit_claims() {
    let s = setup();
    s.client.subscribe(&s.member);
    let unlicensed = Address::generate(&s.env);

    s.client
        .submit_claim(&unlicensed, &s.member, &5_000, &hash(&s.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn claims_need_a_credential_registry() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(NOW);
    let admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let id = env.register(
        MicroInsurancePool,
        (&admin, &token_id, &PREMIUM, &PERIOD, &ANNUAL_LIMIT),
    );
    let client = MicroInsurancePoolClient::new(&env, &id);

    let member = Address::generate(&env);
    StellarAssetClient::new(&env, &token_id).mint(&member, &PREMIUM);
    let provider = Address::generate(&env);
    client.subscribe(&member);

    client.submit_claim(&provider, &member, &5_000, &hash(&env));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn approved_claims_cannot_be_paid_twice() {
    let s = setup();
    s.client.subscribe(&s.member);
    s.client.subscribe(&s.member);
    let claim = s
        .client
        .submit_claim(&s.provider, &s.member, &1_000, &hash(&s.env));

    s.client.approve_claim(&claim);
    s.client.approve_claim(&claim);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn constructor_rejects_invalid_terms() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    env.register(
        MicroInsurancePool,
        (&admin, &token, &0i128, &PERIOD, &ANNUAL_LIMIT),
    );
}
