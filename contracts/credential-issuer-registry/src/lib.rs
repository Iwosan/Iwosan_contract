#![no_std]

//! # Credential Issuer Registry
//!
//! Today the marketplace relies on a single admin `verify_listing` flag. Real
//! regulators already do this work off-chain, so this contract lets them issue
//! verifiable licences on-chain instead:
//!
//! - the admin registers trusted issuers (e.g. NAFDAC, a nursing council, a
//!   herbal practitioners' association),
//! - a registered issuer issues a credential of a given type to a subject with
//!   an expiry date, and can revoke it,
//! - anyone can call `has_valid_credential` to check a licence is issued,
//!   unrevoked and unexpired before letting a listing or hire go ahead.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, Address,
    Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotIssuer = 1,
    NotAuthorized = 2,
    CredentialNotFound = 3,
    InvalidExpiry = 4,
}

/// The licence a subject can hold. One subject may hold several types.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialType {
    DrugSeller,
    NativeMedicinePractitioner,
    Nurse,
    Doctor,
    LabScientist,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Credential {
    pub subject: Address,
    pub credential_type: CredentialType,
    pub issuer: Address,
    pub expires_at: u64,
    pub revoked: bool,
}

#[contractevent(topics = ["issuer_add"])]
pub struct IssuerAdded {
    pub issuer: Address,
}

#[contractevent(topics = ["issuer_rem"])]
pub struct IssuerRemoved {
    pub issuer: Address,
}

#[contractevent(topics = ["issued"])]
pub struct CredentialIssued {
    #[topic]
    pub subject: Address,
    pub credential_type: CredentialType,
    pub issuer: Address,
    pub expires_at: u64,
}

#[contractevent(topics = ["revoked"])]
pub struct CredentialRevoked {
    #[topic]
    pub subject: Address,
    pub credential_type: CredentialType,
}

#[contracttype]
enum DataKey {
    Admin,
    Issuer(Address),
    Credential(Address, CredentialType),
}

#[contract]
pub struct CredentialIssuerRegistry;

#[contractimpl]
impl CredentialIssuerRegistry {
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Admin trusts a body to issue credentials.
    pub fn add_issuer(env: Env, issuer: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::Issuer(issuer.clone()), &true);
        IssuerAdded { issuer }.publish(&env);
    }

    /// Admin withdraws the right to issue. Credentials already issued are kept
    /// and must be revoked individually.
    pub fn remove_issuer(env: Env, issuer: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::Issuer(issuer.clone()));
        IssuerRemoved { issuer }.publish(&env);
    }

    pub fn is_issuer(env: Env, issuer: Address) -> bool {
        env.storage().persistent().has(&DataKey::Issuer(issuer))
    }

    /// A registered issuer issues (or renews) a credential for `subject`.
    pub fn issue(
        env: Env,
        issuer: Address,
        subject: Address,
        credential_type: CredentialType,
        expires_at: u64,
    ) {
        issuer.require_auth();
        if !Self::is_issuer(env.clone(), issuer.clone()) {
            panic_with_error!(&env, Error::NotIssuer);
        }
        if expires_at <= env.ledger().timestamp() {
            panic_with_error!(&env, Error::InvalidExpiry);
        }

        let credential = Credential {
            subject: subject.clone(),
            credential_type,
            issuer: issuer.clone(),
            expires_at,
            revoked: false,
        };
        env.storage().persistent().set(
            &DataKey::Credential(subject.clone(), credential_type),
            &credential,
        );

        CredentialIssued {
            subject,
            credential_type,
            issuer,
            expires_at,
        }
        .publish(&env);
    }

    /// The issuing body or the admin revokes a credential (e.g. a suspended
    /// licence).
    pub fn revoke(env: Env, caller: Address, subject: Address, credential_type: CredentialType) {
        caller.require_auth();
        let key = DataKey::Credential(subject.clone(), credential_type);
        let mut credential: Credential = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::CredentialNotFound));

        let admin = Self::admin(&env);
        if caller != admin && caller != credential.issuer {
            panic_with_error!(&env, Error::NotAuthorized);
        }

        credential.revoked = true;
        env.storage().persistent().set(&key, &credential);
        CredentialRevoked {
            subject,
            credential_type,
        }
        .publish(&env);
    }

    /// `true` only when the credential exists, is not revoked and has not
    /// expired.
    pub fn has_valid_credential(
        env: Env,
        subject: Address,
        credential_type: CredentialType,
    ) -> bool {
        match env
            .storage()
            .persistent()
            .get::<DataKey, Credential>(&DataKey::Credential(subject, credential_type))
        {
            Some(credential) => {
                !credential.revoked && credential.expires_at > env.ledger().timestamp()
            }
            None => false,
        }
    }

    fn admin(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }
}

#[cfg(test)]
mod test;
