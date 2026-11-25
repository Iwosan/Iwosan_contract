#![no_std]

//! # Drug Batch Registry
//!
//! Counterfeit drugs are the biggest trust problem in Nigerian drug markets.
//! This contract gives every pack a traceable life on-chain:
//!
//! - the admin registers the manufacturers that may create batches,
//! - a manufacturer registers each batch with its product details, expiry and
//!   regulatory (e.g. NAFDAC) number,
//! - every custody handoff (manufacturer -> distributor -> pharmacy) is recorded,
//! - a batch can be recalled by its manufacturer or the admin, after which it
//!   can no longer change hands.
//!
//! Buyers and the API can check `is_recalled` and replay `get_history` to reject
//! counterfeit or recalled packs before money moves.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, Address,
    BytesN, Env, String, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    BatchExists = 1,
    BatchNotFound = 2,
    NotManufacturer = 3,
    NotHolder = 4,
    AlreadyRecalled = 5,
    NotAuthorized = 6,
    InvalidExpiry = 7,
    BatchRecalled = 8,
}

/// The kind of drug in a batch. Native medicine is traditional/herbal.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrugKind {
    NativeDrug,
    OrthodoxDrug,
}

/// One custody handoff. `from` is `None` for the initial registration.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CustodyEvent {
    pub from: Option<Address>,
    pub to: Address,
    pub at: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct DrugBatch {
    pub batch_id: BytesN<32>,
    pub manufacturer: Address,
    pub product: String,
    pub kind: DrugKind,
    pub expiry: u64,
    pub regulatory_number: String,
    /// Current custodian of the batch.
    pub holder: Address,
    pub recalled: bool,
}

#[contractevent(topics = ["mfr_added"])]
pub struct ManufacturerAdded {
    pub manufacturer: Address,
}

#[contractevent(topics = ["mfr_removed"])]
pub struct ManufacturerRemoved {
    pub manufacturer: Address,
}

#[contractevent(topics = ["registered"])]
pub struct BatchRegistered {
    #[topic]
    pub batch_id: BytesN<32>,
    pub manufacturer: Address,
}

#[contractevent(topics = ["custody"])]
pub struct CustodyChanged {
    #[topic]
    pub batch_id: BytesN<32>,
    pub from: Address,
    pub to: Address,
}

#[contractevent(topics = ["recalled"])]
pub struct BatchRecalled {
    #[topic]
    pub batch_id: BytesN<32>,
    pub caller: Address,
    pub reason_hash: BytesN<32>,
}

#[contracttype]
enum DataKey {
    Admin,
    Manufacturer(Address),
    Batch(BytesN<32>),
    History(BytesN<32>),
}

#[contract]
pub struct DrugBatchRegistry;

#[contractimpl]
impl DrugBatchRegistry {
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Admin allows a manufacturer to register batches.
    pub fn add_manufacturer(env: Env, manufacturer: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::Manufacturer(manufacturer.clone()), &true);
        ManufacturerAdded { manufacturer }.publish(&env);
    }

    /// Admin stops a manufacturer from registering new batches. Batches that
    /// already exist are unaffected.
    pub fn remove_manufacturer(env: Env, manufacturer: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::Manufacturer(manufacturer.clone()));
        ManufacturerRemoved { manufacturer }.publish(&env);
    }

    pub fn is_manufacturer(env: Env, manufacturer: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Manufacturer(manufacturer))
    }

    /// A registered manufacturer registers a new batch. The manufacturer is the
    /// first custodian of the batch.
    pub fn register_batch(
        env: Env,
        manufacturer: Address,
        batch_id: BytesN<32>,
        product: String,
        kind: DrugKind,
        expiry: u64,
        regulatory_number: String,
    ) {
        manufacturer.require_auth();
        if !Self::is_manufacturer(env.clone(), manufacturer.clone()) {
            panic_with_error!(&env, Error::NotManufacturer);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::Batch(batch_id.clone()))
        {
            panic_with_error!(&env, Error::BatchExists);
        }
        if expiry <= env.ledger().timestamp() {
            panic_with_error!(&env, Error::InvalidExpiry);
        }

        let batch = DrugBatch {
            batch_id: batch_id.clone(),
            manufacturer: manufacturer.clone(),
            product,
            kind,
            expiry,
            regulatory_number,
            holder: manufacturer.clone(),
            recalled: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Batch(batch_id.clone()), &batch);

        let mut history = Vec::new(&env);
        history.push_back(CustodyEvent {
            from: None,
            to: manufacturer.clone(),
            at: env.ledger().timestamp(),
        });
        env.storage()
            .persistent()
            .set(&DataKey::History(batch_id.clone()), &history);

        BatchRegistered {
            batch_id,
            manufacturer,
        }
        .publish(&env);
    }

    /// The current holder hands the batch to `to`. Recalled batches cannot move.
    pub fn transfer_custody(env: Env, from: Address, to: Address, batch_id: BytesN<32>) {
        from.require_auth();
        let mut batch = Self::get_batch(env.clone(), batch_id.clone());
        if batch.recalled {
            panic_with_error!(&env, Error::BatchRecalled);
        }
        if batch.holder != from {
            panic_with_error!(&env, Error::NotHolder);
        }

        batch.holder = to.clone();
        env.storage()
            .persistent()
            .set(&DataKey::Batch(batch_id.clone()), &batch);

        let mut history = Self::history(&env, batch_id.clone());
        history.push_back(CustodyEvent {
            from: Some(from.clone()),
            to: to.clone(),
            at: env.ledger().timestamp(),
        });
        env.storage()
            .persistent()
            .set(&DataKey::History(batch_id.clone()), &history);

        CustodyChanged { batch_id, from, to }.publish(&env);
    }

    /// Recall a batch. Only its manufacturer or the admin may do it, and only
    /// once. `reason_hash` is a hash of the off-chain recall notice.
    pub fn recall_batch(env: Env, caller: Address, batch_id: BytesN<32>, reason_hash: BytesN<32>) {
        caller.require_auth();
        let mut batch = Self::get_batch(env.clone(), batch_id.clone());
        let admin = Self::admin(&env);
        if caller != batch.manufacturer && caller != admin {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        if batch.recalled {
            panic_with_error!(&env, Error::AlreadyRecalled);
        }

        batch.recalled = true;
        env.storage()
            .persistent()
            .set(&DataKey::Batch(batch_id.clone()), &batch);

        BatchRecalled {
            batch_id,
            caller,
            reason_hash,
        }
        .publish(&env);
    }

    pub fn is_recalled(env: Env, batch_id: BytesN<32>) -> bool {
        Self::get_batch(env, batch_id).recalled
    }

    pub fn get_batch(env: Env, batch_id: BytesN<32>) -> DrugBatch {
        env.storage()
            .persistent()
            .get(&DataKey::Batch(batch_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::BatchNotFound))
    }

    /// Every custody handoff for a batch, oldest first. The first entry is the
    /// manufacturer taking initial custody on registration.
    pub fn get_history(env: Env, batch_id: BytesN<32>) -> Vec<CustodyEvent> {
        Self::get_batch(env.clone(), batch_id.clone());
        Self::history(&env, batch_id)
    }

    fn history(env: &Env, batch_id: BytesN<32>) -> Vec<CustodyEvent> {
        env.storage()
            .persistent()
            .get(&DataKey::History(batch_id))
            .unwrap_or_else(|| Vec::new(env))
    }

    fn admin(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }
}

#[cfg(test)]
mod test;
