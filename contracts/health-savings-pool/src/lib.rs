#![no_std]

//! # Health Savings Pool (ajo/esusu)
//!
//! A rotating community savings pool for health costs, modelled on ajo/esusu:
//!
//! - an organiser creates a pool with a fixed contribution, a period length and
//!   an ordered list of members,
//! - every member contributes once per round,
//! - when everyone has contributed (or the period has ended), `payout` sends the
//!   whole pot to the next member in rotation, in the order they were listed,
//! - a member facing a medical emergency can ask to withdraw from the pot early;
//!   the money is paid once a majority of members approve.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, token,
    Address, Env, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    PoolNotFound = 1,
    InvalidConfig = 2,
    NotMember = 3,
    AlreadyContributed = 4,
    RoundNotComplete = 5,
    RequestNotFound = 6,
    AlreadyApproved = 7,
    InsufficientPot = 8,
    RequestClosed = 9,
    InvalidAmount = 10,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Pool {
    pub id: u64,
    pub organiser: Address,
    pub token: Address,
    pub contribution: i128,
    pub period_secs: u64,
    /// Payout order: round `r` pays `members[r % members.len()]`.
    pub members: Vec<Address>,
    /// Ledger timestamp when the pool was created.
    pub start: u64,
    /// Current round, starting at 0.
    pub round: u32,
    /// Contributions collected in the current round that are not yet paid out.
    pub pot: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct EmergencyRequest {
    pub id: u32,
    pub member: Address,
    pub amount: i128,
    pub approvals: u32,
    pub paid: bool,
}

#[contracttype]
enum DataKey {
    NextPoolId,
    Pool(u64),
    /// Set when `member` has contributed in `round` of pool `u64`.
    Contributed(u64, u32, Address),
    NextRequestId(u64),
    Request(u64, u32),
    Approved(u64, u32, Address),
}

#[contractevent(topics = ["pool_new"])]
pub struct PoolCreated {
    #[topic]
    pub pool_id: u64,
    pub organiser: Address,
    pub contribution: i128,
    pub members: u32,
}

#[contractevent(topics = ["contrib"])]
pub struct Contributed {
    #[topic]
    pub pool_id: u64,
    pub member: Address,
    pub round: u32,
    pub amount: i128,
}

#[contractevent(topics = ["payout"])]
pub struct PaidOut {
    #[topic]
    pub pool_id: u64,
    pub member: Address,
    pub round: u32,
    pub amount: i128,
}

#[contractevent(topics = ["emerg_req"])]
pub struct EmergencyRequested {
    #[topic]
    pub pool_id: u64,
    pub request_id: u32,
    pub member: Address,
    pub amount: i128,
}

#[contractevent(topics = ["emerg_paid"])]
pub struct EmergencyPaid {
    #[topic]
    pub pool_id: u64,
    pub request_id: u32,
    pub member: Address,
    pub amount: i128,
}

#[contract]
pub struct HealthSavingsPool;

#[contractimpl]
impl HealthSavingsPool {
    /// Creates a pool. Members are paid out in the order given.
    pub fn create_pool(
        env: Env,
        organiser: Address,
        token: Address,
        contribution: i128,
        period_secs: u64,
        members: Vec<Address>,
    ) -> u64 {
        organiser.require_auth();
        if contribution <= 0 || period_secs == 0 || members.len() < 2 {
            panic_with_error!(&env, Error::InvalidConfig);
        }
        for (index, member) in members.iter().enumerate() {
            if members.first_index_of(&member) != Some(index as u32) {
                panic_with_error!(&env, Error::InvalidConfig);
            }
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextPoolId)
            .unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::NextPoolId, &(id + 1));

        let pool = Pool {
            id,
            organiser: organiser.clone(),
            token,
            contribution,
            period_secs,
            members: members.clone(),
            start: env.ledger().timestamp(),
            round: 0,
            pot: 0,
        };
        Self::save(&env, &pool);
        PoolCreated {
            pool_id: id,
            organiser,
            contribution,
            members: members.len(),
        }
        .publish(&env);
        id
    }

    /// A member pays their contribution for the current round.
    pub fn contribute(env: Env, pool_id: u64, member: Address) {
        member.require_auth();
        let mut pool = Self::get_pool(env.clone(), pool_id);
        Self::require_member(&env, &pool, &member);

        let key = DataKey::Contributed(pool_id, pool.round, member.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::AlreadyContributed);
        }

        // Record the contribution before moving funds.
        env.storage().persistent().set(&key, &true);
        pool.pot += pool.contribution;
        Self::save(&env, &pool);
        token::Client::new(&env, &pool.token).transfer(
            &member,
            env.current_contract_address(),
            &pool.contribution,
        );

        Contributed {
            pool_id,
            member,
            round: pool.round,
            amount: pool.contribution,
        }
        .publish(&env);
    }

    /// Pays the pot to this round's member and starts the next round. Anyone
    /// can call it once every member has contributed, or once the round's
    /// period has ended (then whatever was collected is paid).
    pub fn payout(env: Env, pool_id: u64) {
        let mut pool = Self::get_pool(env.clone(), pool_id);
        let everyone_paid = pool
            .members
            .iter()
            .all(|m| Self::contributed(&env, pool_id, pool.round, &m));
        let round_end = pool.start + pool.period_secs * (pool.round as u64 + 1);
        if !everyone_paid && env.ledger().timestamp() < round_end {
            panic_with_error!(&env, Error::RoundNotComplete);
        }

        let recipient = Self::recipient(&pool);
        let amount = pool.pot;
        let round = pool.round;
        pool.pot = 0;
        pool.round += 1;
        Self::save(&env, &pool);
        if amount > 0 {
            token::Client::new(&env, &pool.token).transfer(
                &env.current_contract_address(),
                &recipient,
                &amount,
            );
        }

        PaidOut {
            pool_id,
            member: recipient,
            round,
            amount,
        }
        .publish(&env);
    }

    /// A member asks to withdraw `amount` from the current pot for a medical
    /// emergency. The request counts as the member's own approval.
    pub fn request_emergency(env: Env, pool_id: u64, member: Address, amount: i128) -> u32 {
        member.require_auth();
        let pool = Self::get_pool(env.clone(), pool_id);
        Self::require_member(&env, &pool, &member);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if amount > pool.pot {
            panic_with_error!(&env, Error::InsufficientPot);
        }

        let id: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::NextRequestId(pool_id))
            .unwrap_or(1);
        env.storage()
            .persistent()
            .set(&DataKey::NextRequestId(pool_id), &(id + 1));

        let request = EmergencyRequest {
            id,
            member: member.clone(),
            amount,
            approvals: 0,
            paid: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Request(pool_id, id), &request);
        EmergencyRequested {
            pool_id,
            request_id: id,
            member: member.clone(),
            amount,
        }
        .publish(&env);

        Self::record_approval(&env, pool, id, member);
        id
    }

    /// A member approves an emergency request. When more than half of the
    /// members have approved, the amount is paid from the pot.
    pub fn approve_emergency(env: Env, pool_id: u64, request_id: u32, member: Address) {
        member.require_auth();
        let pool = Self::get_pool(env.clone(), pool_id);
        Self::require_member(&env, &pool, &member);
        Self::record_approval(&env, pool, request_id, member);
    }

    pub fn get_pool(env: Env, pool_id: u64) -> Pool {
        env.storage()
            .persistent()
            .get(&DataKey::Pool(pool_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PoolNotFound))
    }

    pub fn get_request(env: Env, pool_id: u64, request_id: u32) -> EmergencyRequest {
        env.storage()
            .persistent()
            .get(&DataKey::Request(pool_id, request_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::RequestNotFound))
    }

    /// Whether `member` has contributed in the pool's current round.
    pub fn has_contributed(env: Env, pool_id: u64, member: Address) -> bool {
        let pool = Self::get_pool(env.clone(), pool_id);
        Self::contributed(&env, pool_id, pool.round, &member)
    }

    /// The member the current round will pay out to.
    pub fn next_recipient(env: Env, pool_id: u64) -> Address {
        Self::recipient(&Self::get_pool(env, pool_id))
    }

    /// Adds `member`'s approval (auth and membership already checked) and pays
    /// the request once a majority has approved.
    fn record_approval(env: &Env, mut pool: Pool, request_id: u32, member: Address) {
        let pool_id = pool.id;
        let mut request = Self::get_request(env.clone(), pool_id, request_id);
        if request.paid {
            panic_with_error!(env, Error::RequestClosed);
        }
        let approval = DataKey::Approved(pool_id, request_id, member);
        if env.storage().persistent().has(&approval) {
            panic_with_error!(env, Error::AlreadyApproved);
        }
        env.storage().persistent().set(&approval, &true);
        request.approvals += 1;

        let majority = request.approvals * 2 > pool.members.len();
        if majority {
            // The pot may have been paid out since the request was made.
            if request.amount > pool.pot {
                panic_with_error!(env, Error::InsufficientPot);
            }
            request.paid = true;
            pool.pot -= request.amount;
            Self::save(env, &pool);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Request(pool_id, request_id), &request);

        if majority {
            token::Client::new(env, &pool.token).transfer(
                &env.current_contract_address(),
                &request.member,
                &request.amount,
            );
            EmergencyPaid {
                pool_id,
                request_id,
                member: request.member,
                amount: request.amount,
            }
            .publish(env);
        }
    }

    fn recipient(pool: &Pool) -> Address {
        pool.members.get_unchecked(pool.round % pool.members.len())
    }

    fn contributed(env: &Env, pool_id: u64, round: u32, member: &Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Contributed(pool_id, round, member.clone()))
    }

    fn require_member(env: &Env, pool: &Pool, member: &Address) {
        if !pool.members.contains(member) {
            panic_with_error!(env, Error::NotMember);
        }
    }

    fn save(env: &Env, pool: &Pool) {
        env.storage()
            .persistent()
            .set(&DataKey::Pool(pool.id), pool);
    }
}

#[cfg(test)]
mod test;
