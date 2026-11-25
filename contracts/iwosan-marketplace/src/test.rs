#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, BytesN, Env};

fn setup() -> (Env, IwosanMarketplaceClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let id = env.register(IwosanMarketplace, (&admin, &token));
    let client = IwosanMarketplaceClient::new(&env, &id);
    (env, client, admin, token)
}

#[test]
fn equipment_sale_releases_escrow_to_seller() {
    let (env, client, _admin, token) = setup();
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&buyer, &1_000);

    let listing = client.create_listing(
        &seller,
        &ListingKind::Equipment,
        &TradeMode::Sale,
        &String::from_str(&env, "Oxygen concentrator"),
        &BytesN::from_array(&env, &[0; 32]),
        &300,
        &2,
    );
    let order = client.place_order(&buyer, &listing, &2);
    client.confirm_order(&order);

    let balance = token::Client::new(&env, &token);
    assert_eq!(balance.balance(&seller), 600);
    assert_eq!(balance.balance(&buyer), 400);
    assert_eq!(client.get_listing(&listing).stock, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn unverified_native_drug_cannot_be_ordered() {
    let (env, client, _admin, token) = setup();
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&buyer, &1_000);

    let listing = client.create_listing(
        &seller,
        &ListingKind::NativeDrug,
        &TradeMode::Sale,
        &String::from_str(&env, "Agbo herbal mix"),
        &BytesN::from_array(&env, &[0; 32]),
        &50,
        &10,
    );
    client.place_order(&buyer, &listing, &1);
}

#[test]
fn verified_personnel_hire_can_be_refunded() {
    let (env, client, _admin, token) = setup();
    let nurse = Address::generate(&env);
    let clinic = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&clinic, &500);

    let listing = client.create_listing(
        &nurse,
        &ListingKind::Personnel,
        &TradeMode::Hire,
        &String::from_str(&env, "Registered nurse, night shift"),
        &BytesN::from_array(&env, &[0; 32]),
        &100,
        &0,
    );
    client.verify_listing(&listing);
    let order = client.place_order(&clinic, &listing, &3);
    client.refund_order(&order);

    assert_eq!(token::Client::new(&env, &token).balance(&clinic), 500);
    assert_eq!(client.get_order(&order).status, OrderStatus::Refunded);
}

mod credentials {
    use super::*;
    use credential_issuer_registry::{
        CredentialIssuerRegistry, CredentialIssuerRegistryClient,
        CredentialType as RegistryCredential,
    };
    use soroban_sdk::testutils::Ledger as _;

    struct Setup {
        env: Env,
        market: IwosanMarketplaceClient<'static>,
        registry: CredentialIssuerRegistryClient<'static>,
        issuer: Address,
        seller: Address,
        buyer: Address,
    }

    /// Marketplace wired to a credential registry, with the admin-verify
    /// fallback switched off so only credentials count.
    fn setup() -> Setup {
        let (env, market, admin, token) = super::setup();
        env.ledger().set_timestamp(1_000);
        let registry_id = env.register(CredentialIssuerRegistry, (&admin,));
        let registry = CredentialIssuerRegistryClient::new(&env, &registry_id);
        let issuer = Address::generate(&env);
        registry.add_issuer(&issuer);
        market.set_credential_registry(&registry_id);
        market.set_verify_fallback(&false);

        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        StellarAssetClient::new(&env, &token).mint(&buyer, &1_000);
        Setup {
            env,
            market,
            registry,
            issuer,
            seller,
            buyer,
        }
    }

    fn drug_listing(s: &Setup) -> u64 {
        s.market.create_listing(
            &s.seller,
            &ListingKind::OrthodoxDrug,
            &TradeMode::Sale,
            &String::from_str(&s.env, "Amoxicillin 500mg"),
            &BytesN::from_array(&s.env, &[0; 32]),
            &20,
            &10,
        )
    }

    #[test]
    fn credentialed_seller_can_be_ordered_from() {
        let s = setup();
        s.registry.issue(
            &s.issuer,
            &s.seller,
            &RegistryCredential::DrugSeller,
            &5_000,
        );
        let listing = drug_listing(&s);

        let order = s.market.place_order(&s.buyer, &listing, &2);

        assert_eq!(s.market.get_order(&order).amount, 40);
    }

    #[test]
    fn any_licensed_profession_can_be_hired() {
        let s = setup();
        s.registry
            .issue(&s.issuer, &s.seller, &RegistryCredential::Doctor, &5_000);
        let listing = s.market.create_listing(
            &s.seller,
            &ListingKind::Personnel,
            &TradeMode::Hire,
            &String::from_str(&s.env, "General practitioner, day shift"),
            &BytesN::from_array(&s.env, &[0; 32]),
            &100,
            &0,
        );

        s.market.place_order(&s.buyer, &listing, &1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn revoked_credential_blocks_orders() {
        let s = setup();
        s.registry.issue(
            &s.issuer,
            &s.seller,
            &RegistryCredential::DrugSeller,
            &5_000,
        );
        let listing = drug_listing(&s);
        s.registry
            .revoke(&s.issuer, &s.seller, &RegistryCredential::DrugSeller);

        s.market.place_order(&s.buyer, &listing, &1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn expired_credential_blocks_orders() {
        let s = setup();
        s.registry.issue(
            &s.issuer,
            &s.seller,
            &RegistryCredential::DrugSeller,
            &5_000,
        );
        let listing = drug_listing(&s);
        s.env.ledger().set_timestamp(5_001);

        s.market.place_order(&s.buyer, &listing, &1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn wrong_credential_type_blocks_orders() {
        let s = setup();
        // A nursing licence does not allow selling drugs.
        s.registry
            .issue(&s.issuer, &s.seller, &RegistryCredential::Nurse, &5_000);
        let listing = drug_listing(&s);

        s.market.place_order(&s.buyer, &listing, &1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn admin_verification_is_ignored_when_fallback_is_off() {
        let s = setup();
        let listing = drug_listing(&s);
        s.market.verify_listing(&listing);

        s.market.place_order(&s.buyer, &listing, &1);
    }

    #[test]
    fn admin_verification_works_while_fallback_is_on() {
        let s = setup();
        s.market.set_verify_fallback(&true);
        let listing = drug_listing(&s);
        s.market.verify_listing(&listing);

        s.market.place_order(&s.buyer, &listing, &1);
        assert!(s.market.verify_fallback());
    }
}
