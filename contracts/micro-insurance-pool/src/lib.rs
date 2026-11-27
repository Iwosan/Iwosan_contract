#![no_std]

//! # Micro Insurance Pool
//!
//! Small monthly premiums in one token (e.g. USDC) fund a pool that pays
//! treatment claims:
//!
//! - anyone can `subscribe` for one period by paying the premium; coverage
//!   stays active for a fixed number of seconds and can be renewed early
//!   without losing the remaining time,
//! - a verified provider (one holding a `credential-issuer-registry` licence)
//!   submits a claim against a covered member with a hash of the evidence,
//! - the admin approves the claim and the pool pays the provider directly, so
//!   members never handle the money,
//! - each member has an annual claim limit, and a payout can never take the
//!   pool below zero.

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype,
    panic_with_error, token, Address, BytesN, Env,
};

/// Seconds in a 365-day year, used to bucket claims for the annual limit.
const YEAR_SECS: u64 = 365 * 24 * 60 * 60;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotCovered = 1,
    InsufficientPool = 2,
    AnnualLimitExceeded = 3,
    ClaimNotFound = 4,
    ClaimClosed = 5,
    InvalidAmount = 6,
    NotVerified = 7,
    RegistryNotSet = 8,
    InvalidConfig = 9,
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

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimStatus {
    Pending,
    Paid,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Claim {
    pub id: u64,
    pub member: Address,
    /// Verified provider that is paid when the claim is approved.
    pub provider: Address,
    pub amount: i128,
    /// Hash of the off-chain evidence (invoice, treatment record, etc.).
    pub evidence_hash: BytesN<32>,
    pub submitted_at: u64,
    pub status: ClaimStatus,
}

#[contracttype]
enum DataKey {
    Admin,
    Token,
    Premium,
    PeriodSecs,
    AnnualLimit,
    CredentialRegistry,
    /// Ledger timestamp up to which `member` is covered.
    Coverage(Address),
    /// Claim amount already reserved by `member` in a given year bucket.
    Claimed(Address, u64),
    NextClaimId,
    Claim(u64),
}

#[contractevent(topics = ["subscribed"])]
pub struct Subscribed {
    #[topic]
    pub member: Address,
    pub amount: i128,
    pub coverage_until: u64,
}

#[contractevent(topics = ["claim_new"])]
pub struct ClaimSubmitted {
    #[topic]
    pub claim_id: u64,
    pub member: Address,
    pub provider: Address,
    pub amount: i128,
}

#[contractevent(topics = ["claim_paid"])]
pub struct ClaimPaid {
    #[topic]
    pub claim_id: u64,
    pub member: Address,
    pub provider: Address,
    pub amount: i128,
}

#[contract]
pub struct MicroInsurancePool;

#[contractimpl]
impl MicroInsurancePool {
    /// Sets the admin, the token premiums and payouts use, the premium per
    /// period, the length of a coverage period and the per-member annual
    /// claim limit.
    pub fn __constructor(
        env: Env,
        admin: Address,
        token: Address,
        premium: i128,
        period_secs: u64,
        annual_limit: i128,
    ) {
        if premium <= 0 || period_secs == 0 || annual_limit <= 0 {
            panic_with_error!(&env, Error::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Premium, &premium);
        env.storage()
            .instance()
            .set(&DataKey::PeriodSecs, &period_secs);
        env.storage()
            .instance()
            .set(&DataKey::AnnualLimit, &annual_limit);
    }

    /// Admin points the pool at a deployed `credential-issuer-registry`, which
    /// is how providers prove they are licensed.
    pub fn set_credential_registry(env: Env, registry: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .instance()
            .set(&DataKey::CredentialRegistry, &registry);
    }

    pub fn credential_registry(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::CredentialRegistry)
    }

    /// Pays one premium. Renewing before the current period ends adds the new
    /// period on top of the remaining time instead of discarding it.
    pub fn subscribe(env: Env, member: Address) {
        member.require_auth();
        let now = env.ledger().timestamp();
        let premium = Self::premium_amount(&env);
        let current = Self::coverage_until(env.clone(), member.clone());
        let coverage_until = current.max(now) + Self::period_secs_value(&env);

        env.storage()
            .persistent()
            .set(&DataKey::Coverage(member.clone()), &coverage_until);
        Self::token(&env).transfer(&member, env.current_contract_address(), &premium);
        Subscribed {
            member,
            amount: premium,
            coverage_until,
        }
        .publish(&env);
    }

    /// A licensed provider submits a claim for a member whose coverage is
    /// active. The amount is reserved against the member's annual limit.
    pub fn submit_claim(
        env: Env,
        provider: Address,
        member: Address,
        amount: i128,
        evidence_hash: BytesN<32>,
    ) -> u64 {
        provider.require_auth();
        let Some(registry) = Self::credential_registry(env.clone()) else {
            panic_with_error!(&env, Error::RegistryNotSet);
        };
        if !Self::provider_is_verified(&env, &registry, &provider) {
            panic_with_error!(&env, Error::NotVerified);
        }
        if !Self::is_covered(env.clone(), member.clone()) {
            panic_with_error!(&env, Error::NotCovered);
        }
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let now = env.ledger().timestamp();
        let year = now / YEAR_SECS;
        let claimed_key = DataKey::Claimed(member.clone(), year);
        let claimed: i128 = env.storage().persistent().get(&claimed_key).unwrap_or(0);
        let requested = claimed
            .checked_add(amount)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        if requested > Self::annual_limit_value(&env) {
            panic_with_error!(&env, Error::AnnualLimitExceeded);
        }
        // Reserve the amount before the claim can be approved.
        env.storage().persistent().set(&claimed_key, &requested);

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextClaimId)
            .unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::NextClaimId, &(id + 1));

        let claim = Claim {
            id,
            member: member.clone(),
            provider: provider.clone(),
            amount,
            evidence_hash,
            submitted_at: now,
            status: ClaimStatus::Pending,
        };
        env.storage().persistent().set(&DataKey::Claim(id), &claim);
        ClaimSubmitted {
            claim_id: id,
            member,
            provider,
            amount,
        }
        .publish(&env);
        id
    }

    /// Admin approves a claim; the provider is paid directly from the pool.
    /// Approval fails if the pool does not hold enough to cover the claim.
    pub fn approve_claim(env: Env, claim_id: u64) {
        Self::admin(&env).require_auth();
        let mut claim = Self::get_claim(env.clone(), claim_id);
        if claim.status != ClaimStatus::Pending {
            panic_with_error!(&env, Error::ClaimClosed);
        }
        if claim.amount > Self::token(&env).balance(&env.current_contract_address()) {
            panic_with_error!(&env, Error::InsufficientPool);
        }

        // Close the claim before the transfer so it cannot be paid twice.
        claim.status = ClaimStatus::Paid;
        env.storage()
            .persistent()
            .set(&DataKey::Claim(claim_id), &claim);
        Self::token(&env).transfer(
            &env.current_contract_address(),
            &claim.provider,
            &claim.amount,
        );
        ClaimPaid {
            claim_id,
            member: claim.member,
            provider: claim.provider,
            amount: claim.amount,
        }
        .publish(&env);
    }

    pub fn get_claim(env: Env, claim_id: u64) -> Claim {
        env.storage()
            .persistent()
            .get(&DataKey::Claim(claim_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::ClaimNotFound))
    }

    /// Ledger timestamp up to which `member` is covered (0 if never subscribed).
    pub fn coverage_until(env: Env, member: Address) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::Coverage(member))
            .unwrap_or(0)
    }

    pub fn is_covered(env: Env, member: Address) -> bool {
        Self::coverage_until(env.clone(), member) >= env.ledger().timestamp()
    }

    /// Amount reserved by `member` in the current annual bucket.
    pub fn claimed_this_year(env: Env, member: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Claimed(
                member,
                env.ledger().timestamp() / YEAR_SECS,
            ))
            .unwrap_or(0)
    }

    pub fn premium(env: Env) -> i128 {
        Self::premium_amount(&env)
    }

    pub fn annual_limit(env: Env) -> i128 {
        Self::annual_limit_value(&env)
    }

    /// Providers need a doctor, nurse or lab-scientist licence.
    fn provider_is_verified(env: &Env, registry: &Address, provider: &Address) -> bool {
        let registry = CredentialRegistryClient::new(env, registry);
        [
            CredentialType::Doctor,
            CredentialType::Nurse,
            CredentialType::LabScientist,
        ]
        .iter()
        .any(|credential| registry.has_valid_credential(provider, credential))
    }

    fn admin(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("admin not set")
    }

    fn premium_amount(env: &Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::Premium)
            .expect("premium not set")
    }

    fn annual_limit_value(env: &Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::AnnualLimit)
            .expect("annual limit not set")
    }

    fn period_secs_value(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::PeriodSecs)
            .expect("period not set")
    }

    fn token(env: &Env) -> token::Client<'_> {
        token::Client::new(
            env,
            &env.storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::Token)
                .expect("token not set"),
        )
    }
}

#[cfg(test)]
mod test;
