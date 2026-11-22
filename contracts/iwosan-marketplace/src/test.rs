#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, BytesN, Env};

fn setup() -> (Env, IwosanMarketplaceClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
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
