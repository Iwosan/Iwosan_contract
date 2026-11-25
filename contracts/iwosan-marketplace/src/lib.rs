#![no_std]
#![allow(clippy::too_many_arguments)]

//! # Iwosan Marketplace Contract
//!
//! Escrow-backed marketplace for healthcare goods and services on Stellar:
//! - sale and rental of medical equipment,
//! - orthodox (pharmaceutical) and native (traditional/herbal) drugs,
//! - hiring of medical personnel.
//!
//! Buyers pay into escrow; funds are released to the seller only when the buyer
//! confirms delivery, or refunded by the seller. The admin (e.g. a regulator or
//! the Iwosan verification desk) can resolve disputes. Drug and personnel
//! listings must be trusted before they can be ordered, so unverified
//! medicines and unlicensed practitioners cannot be traded. A seller is trusted
//! when they hold a valid licence in the `credential-issuer-registry` contract
//! (issued by NAFDAC, a nursing council, etc.), or, while the admin keeps the
//! fallback switched on, when the admin has verified the listing directly.

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype,
    panic_with_error, token, Address, BytesN, Env, String,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    ListingNotFound = 1,
    OrderNotFound = 2,
    InvalidAmount = 3,
    ListingInactive = 4,
    NotVerified = 5,
    InsufficientStock = 6,
    InvalidMode = 7,
    OrderClosed = 8,
}

/// What is being offered.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListingKind {
    Equipment,
    OrthodoxDrug,
    NativeDrug,
    Personnel,
}

/// How it is offered. `units` on an order means items for `Sale`, days for
/// `Rental` and days/shifts for `Hire`.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TradeMode {
    Sale,
    Rental,
    Hire,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderStatus {
    Escrowed,
    Released,
    Refunded,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Listing {
    pub id: u64,
    pub seller: Address,
    pub kind: ListingKind,
    pub mode: TradeMode,
    pub title: String,
    /// Hash of off-chain details (photos, NAFDAC number, licence, etc.).
    pub details_hash: BytesN<32>,
    /// Price per unit (item, day or shift) in the payment token.
    pub unit_price: i128,
    /// Items in stock for `Sale`; ignored for `Rental` and `Hire`.
    pub stock: u32,
    pub verified: bool,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Order {
    pub id: u64,
    pub listing_id: u64,
    pub buyer: Address,
    pub seller: Address,
    pub units: u32,
    pub amount: i128,
    pub status: OrderStatus,
}

#[contracttype]
enum DataKey {
    Admin,
    Token,
    NextListingId,
    NextOrderId,
    Listing(u64),
    Order(u64),
    CredentialRegistry,
    VerifyFallback,
}

/// Mirrors `CredentialType` in `contracts/credential-issuer-registry`. Enum
/// variants are encoded by name, so the two definitions must stay in sync.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialType {
    DrugSeller,
    NativeMedicinePractitioner,
    Nurse,
    Doctor,
    LabScientist,
}

/// The part of the credential registry this contract calls.
#[contractclient(name = "CredentialRegistryClient")]
pub trait CredentialRegistry {
    fn has_valid_credential(env: Env, subject: Address, credential_type: CredentialType) -> bool;
}

// Events keep the same topics and data as the original `publish` calls, so
// indexers (e.g. the Iwosan API) read them unchanged.

#[contractevent(topics = ["listed"], data_format = "single-value")]
pub struct Listed {
    #[topic]
    pub id: u64,
    pub seller: Address,
}

#[contractevent(topics = ["verified"], data_format = "single-value")]
pub struct Verified {
    #[topic]
    pub id: u64,
}

#[contractevent(topics = ["ordered"], data_format = "single-value")]
pub struct Ordered {
    #[topic]
    pub id: u64,
    pub amount: i128,
}

#[contractevent(topics = ["settled"], data_format = "single-value")]
pub struct Settled {
    #[topic]
    pub id: u64,
    pub release: bool,
}

#[contract]
pub struct IwosanMarketplace;

#[contractimpl]
impl IwosanMarketplace {
    /// Sets the admin and the payment token (e.g. a USDC or XLM SAC address).
    pub fn __constructor(env: Env, admin: Address, token: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
    }

    pub fn create_listing(
        env: Env,
        seller: Address,
        kind: ListingKind,
        mode: TradeMode,
        title: String,
        details_hash: BytesN<32>,
        unit_price: i128,
        stock: u32,
    ) -> u64 {
        seller.require_auth();
        if unit_price <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        // Personnel are hired, never sold or rented; goods are never "hired".
        let valid_mode = match kind {
            ListingKind::Personnel => mode == TradeMode::Hire,
            ListingKind::Equipment => mode != TradeMode::Hire,
            ListingKind::OrthodoxDrug | ListingKind::NativeDrug => mode == TradeMode::Sale,
        };
        if !valid_mode {
            panic_with_error!(&env, Error::InvalidMode);
        }

        let id = Self::next_id(&env, DataKey::NextListingId);
        let listing = Listing {
            id,
            seller,
            kind,
            mode,
            title,
            details_hash,
            unit_price,
            stock,
            // Equipment can trade immediately; drugs and personnel need review.
            verified: kind == ListingKind::Equipment,
            active: true,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Listing(id), &listing);
        Listed {
            id,
            seller: listing.seller,
        }
        .publish(&env);
        id
    }

    /// Admin marks a drug or personnel listing as verified (e.g. NAFDAC number
    /// or practising licence checked).
    pub fn verify_listing(env: Env, listing_id: u64) {
        Self::admin(&env).require_auth();
        let mut listing = Self::get_listing(env.clone(), listing_id);
        listing.verified = true;
        env.storage()
            .persistent()
            .set(&DataKey::Listing(listing_id), &listing);
        Verified { id: listing_id }.publish(&env);
    }

    /// Admin points the marketplace at a deployed `credential-issuer-registry`.
    pub fn set_credential_registry(env: Env, registry: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .instance()
            .set(&DataKey::CredentialRegistry, &registry);
    }

    pub fn credential_registry(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::CredentialRegistry)
    }

    /// Admin switches the `verify_listing` fallback on or off. While it is on
    /// (the default), an admin-verified drug or personnel listing can be
    /// ordered even without a registry credential. Switch it off once issuers
    /// are onboarded so only credentialed sellers can trade.
    pub fn set_verify_fallback(env: Env, enabled: bool) {
        Self::admin(&env).require_auth();
        env.storage()
            .instance()
            .set(&DataKey::VerifyFallback, &enabled);
    }

    pub fn verify_fallback(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::VerifyFallback)
            .unwrap_or(true)
    }

    pub fn deactivate_listing(env: Env, listing_id: u64) {
        let mut listing = Self::get_listing(env.clone(), listing_id);
        listing.seller.require_auth();
        listing.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Listing(listing_id), &listing);
    }

    /// Buyer pays `unit_price * units` into escrow.
    pub fn place_order(env: Env, buyer: Address, listing_id: u64, units: u32) -> u64 {
        buyer.require_auth();
        if units == 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let mut listing = Self::get_listing(env.clone(), listing_id);
        if !listing.active {
            panic_with_error!(&env, Error::ListingInactive);
        }
        if !Self::seller_is_trusted(&env, &listing) {
            panic_with_error!(&env, Error::NotVerified);
        }
        if listing.mode == TradeMode::Sale {
            if listing.stock < units {
                panic_with_error!(&env, Error::InsufficientStock);
            }
            listing.stock -= units;
            env.storage()
                .persistent()
                .set(&DataKey::Listing(listing_id), &listing);
        }

        let amount = listing
            .unit_price
            .checked_mul(units as i128)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        let id = Self::next_id(&env, DataKey::NextOrderId);
        let order = Order {
            id,
            listing_id,
            buyer: buyer.clone(),
            seller: listing.seller,
            units,
            amount,
            status: OrderStatus::Escrowed,
        };
        // Persist the order before moving funds.
        env.storage().persistent().set(&DataKey::Order(id), &order);
        Self::token(&env).transfer(&buyer, env.current_contract_address(), &amount);
        Ordered { id, amount }.publish(&env);
        id
    }

    /// Buyer confirms delivery / service; escrow is paid to the seller.
    pub fn confirm_order(env: Env, order_id: u64) {
        let order = Self::get_order(env.clone(), order_id);
        order.buyer.require_auth();
        Self::settle(&env, order, true);
    }

    /// Seller cancels the order; escrow goes back to the buyer.
    pub fn refund_order(env: Env, order_id: u64) {
        let order = Self::get_order(env.clone(), order_id);
        order.seller.require_auth();
        Self::settle(&env, order, false);
    }

    /// Admin resolves a dispute in favour of the seller (`release = true`) or
    /// the buyer (`release = false`).
    pub fn resolve_dispute(env: Env, order_id: u64, release: bool) {
        Self::admin(&env).require_auth();
        let order = Self::get_order(env.clone(), order_id);
        Self::settle(&env, order, release);
    }

    pub fn get_listing(env: Env, listing_id: u64) -> Listing {
        env.storage()
            .persistent()
            .get(&DataKey::Listing(listing_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::ListingNotFound))
    }

    pub fn get_order(env: Env, order_id: u64) -> Order {
        env.storage()
            .persistent()
            .get(&DataKey::Order(order_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::OrderNotFound))
    }

    fn settle(env: &Env, mut order: Order, release: bool) {
        if order.status != OrderStatus::Escrowed {
            panic_with_error!(env, Error::OrderClosed);
        }
        let to = if release {
            order.status = OrderStatus::Released;
            order.seller.clone()
        } else {
            order.status = OrderStatus::Refunded;
            order.buyer.clone()
        };
        // Close the order before the transfer so it cannot be settled twice.
        env.storage()
            .persistent()
            .set(&DataKey::Order(order.id), &order);
        Self::token(env).transfer(&env.current_contract_address(), &to, &order.amount);
        Settled {
            id: order.id,
            release,
        }
        .publish(env);
    }

    /// Equipment trades once listed. Drugs and personnel need the seller to
    /// hold a matching registry credential, or an admin-verified listing while
    /// the fallback is on.
    fn seller_is_trusted(env: &Env, listing: &Listing) -> bool {
        let accepted: &[CredentialType] = match listing.kind {
            ListingKind::Equipment => return listing.verified,
            ListingKind::OrthodoxDrug => &[CredentialType::DrugSeller],
            ListingKind::NativeDrug => &[CredentialType::NativeMedicinePractitioner],
            ListingKind::Personnel => &[
                CredentialType::Nurse,
                CredentialType::Doctor,
                CredentialType::LabScientist,
            ],
        };

        if let Some(registry) = Self::credential_registry(env.clone()) {
            let registry = CredentialRegistryClient::new(env, &registry);
            if accepted
                .iter()
                .any(|credential| registry.has_valid_credential(&listing.seller, credential))
            {
                return true;
            }
        }

        listing.verified && Self::verify_fallback(env.clone())
    }

    fn next_id(env: &Env, key: DataKey) -> u64 {
        let id: u64 = env.storage().instance().get(&key).unwrap_or(1);
        env.storage().instance().set(&key, &(id + 1));
        id
    }

    fn admin(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    fn token(env: &Env) -> token::Client<'_> {
        let address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(env, &address)
    }
}

#[cfg(test)]
mod test;
