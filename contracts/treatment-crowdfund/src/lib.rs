#![no_std]

//! # Treatment Crowdfund
//!
//! Lets a patient raise money for surgery or treatment without ever holding
//! the funds themselves:
//!
//! - the patient opens a campaign naming the hospital, a goal, a deadline and
//!   a hash of the off-chain details (diagnosis, quote, etc.),
//! - donors contribute until the deadline; a campaign never takes more than
//!   its goal,
//! - once the goal is met, the hospital confirms the treatment and `release`
//!   pays the whole pot to the hospital, never to the patient,
//! - if the deadline passes without reaching the goal, each donor can take
//!   their own contribution back exactly once.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, token,
    Address, BytesN, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    CampaignNotFound = 1,
    InvalidConfig = 2,
    InvalidAmount = 3,
    CampaignClosed = 4,
    GoalNotMet = 5,
    AlreadyReleased = 6,
    RefundUnavailable = 7,
    NothingToRefund = 8,
    AlreadyRefunded = 9,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Campaign {
    pub id: u64,
    pub patient: Address,
    /// The only address donations can ever be paid to.
    pub hospital: Address,
    pub goal: i128,
    /// Contributions currently held for the campaign.
    pub raised: i128,
    pub deadline: u64,
    /// Hash of the off-chain details (diagnosis, treatment quote, etc.).
    pub details_hash: BytesN<32>,
    pub released: bool,
}

#[contracttype]
enum DataKey {
    Token,
    NextCampaignId,
    Campaign(u64),
    /// Amount `donor` contributed to a campaign.
    Contribution(u64, Address),
    /// Set once `donor` has been refunded for a campaign.
    Refunded(u64, Address),
}

#[contractevent(topics = ["camp_new"])]
pub struct CampaignCreated {
    #[topic]
    pub campaign_id: u64,
    pub patient: Address,
    pub hospital: Address,
    pub goal: i128,
    pub deadline: u64,
}

#[contractevent(topics = ["donated"])]
pub struct Donated {
    #[topic]
    pub campaign_id: u64,
    pub donor: Address,
    pub amount: i128,
    pub raised: i128,
}

#[contractevent(topics = ["released"])]
pub struct Released {
    #[topic]
    pub campaign_id: u64,
    pub hospital: Address,
    pub amount: i128,
}

#[contractevent(topics = ["refunded"])]
pub struct Refunded {
    #[topic]
    pub campaign_id: u64,
    pub donor: Address,
    pub amount: i128,
}

#[contract]
pub struct TreatmentCrowdfund;

#[contractimpl]
impl TreatmentCrowdfund {
    /// Sets the donation token (e.g. a USDC or XLM SAC address).
    pub fn __constructor(env: Env, token: Address) {
        env.storage().instance().set(&DataKey::Token, &token);
    }

    /// Opens a campaign. Donations can only go to `hospital`.
    pub fn create_campaign(
        env: Env,
        patient: Address,
        hospital: Address,
        goal: i128,
        deadline: u64,
        details_hash: BytesN<32>,
    ) -> u64 {
        patient.require_auth();
        if goal <= 0 || deadline <= env.ledger().timestamp() {
            panic_with_error!(&env, Error::InvalidConfig);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextCampaignId)
            .unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::NextCampaignId, &(id + 1));

        let campaign = Campaign {
            id,
            patient: patient.clone(),
            hospital: hospital.clone(),
            goal,
            raised: 0,
            deadline,
            details_hash,
            released: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(id), &campaign);
        CampaignCreated {
            campaign_id: id,
            patient,
            hospital,
            goal,
            deadline,
        }
        .publish(&env);
        id
    }

    /// A donor contributes `amount` while the campaign is still open. The
    /// contract refuses donations that would take it past the goal.
    pub fn donate(env: Env, donor: Address, campaign_id: u64, amount: i128) {
        donor.require_auth();
        let mut campaign = Self::get_campaign(env.clone(), campaign_id);
        if campaign.released {
            panic_with_error!(&env, Error::CampaignClosed);
        }
        if env.ledger().timestamp() >= campaign.deadline {
            panic_with_error!(&env, Error::CampaignClosed);
        }
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let raised = campaign
            .raised
            .checked_add(amount)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        if raised > campaign.goal {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let key = DataKey::Contribution(campaign_id, donor.clone());
        let contributed: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&key, &(contributed + amount));
        campaign.raised = raised;
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(campaign_id), &campaign);

        Self::token(&env).transfer(&donor, env.current_contract_address(), &amount);
        Donated {
            campaign_id,
            donor,
            amount,
            raised,
        }
        .publish(&env);
    }

    /// The hospital confirms the treatment once the goal is met; the whole pot
    /// is paid to the hospital. The patient can never withdraw these funds.
    pub fn release(env: Env, campaign_id: u64) {
        let mut campaign = Self::get_campaign(env.clone(), campaign_id);
        campaign.hospital.require_auth();
        if campaign.released {
            panic_with_error!(&env, Error::AlreadyReleased);
        }
        if campaign.raised < campaign.goal {
            panic_with_error!(&env, Error::GoalNotMet);
        }

        let amount = campaign.raised;
        // Close the campaign before moving funds so it cannot be paid twice.
        campaign.released = true;
        campaign.raised = 0;
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(campaign_id), &campaign);
        Self::token(&env).transfer(&env.current_contract_address(), &campaign.hospital, &amount);
        Released {
            campaign_id,
            hospital: campaign.hospital,
            amount,
        }
        .publish(&env);
    }

    /// After the deadline, if the goal was not met, a donor takes back their
    /// own contribution. Each donor can be refunded once.
    pub fn refund(env: Env, donor: Address, campaign_id: u64) {
        donor.require_auth();
        let mut campaign = Self::get_campaign(env.clone(), campaign_id);
        if campaign.released {
            panic_with_error!(&env, Error::AlreadyReleased);
        }
        if env.ledger().timestamp() < campaign.deadline || campaign.raised >= campaign.goal {
            panic_with_error!(&env, Error::RefundUnavailable);
        }

        let refunded_key = DataKey::Refunded(campaign_id, donor.clone());
        if env.storage().persistent().has(&refunded_key) {
            panic_with_error!(&env, Error::AlreadyRefunded);
        }
        let contribution_key = DataKey::Contribution(campaign_id, donor.clone());
        let amount: i128 = env
            .storage()
            .persistent()
            .get(&contribution_key)
            .unwrap_or(0);
        if amount <= 0 {
            panic_with_error!(&env, Error::NothingToRefund);
        }

        // Mark the refund before moving funds so it cannot happen twice.
        env.storage().persistent().set(&refunded_key, &true);
        env.storage().persistent().set(&contribution_key, &0i128);
        campaign.raised -= amount;
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(campaign_id), &campaign);

        Self::token(&env).transfer(&env.current_contract_address(), &donor, &amount);
        Refunded {
            campaign_id,
            donor,
            amount,
        }
        .publish(&env);
    }

    pub fn get_campaign(env: Env, campaign_id: u64) -> Campaign {
        env.storage()
            .persistent()
            .get(&DataKey::Campaign(campaign_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::CampaignNotFound))
    }

    /// How much `donor` currently has in a campaign (0 once refunded).
    pub fn contribution(env: Env, campaign_id: u64, donor: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Contribution(campaign_id, donor))
            .unwrap_or(0)
    }

    pub fn has_refunded(env: Env, campaign_id: u64, donor: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Refunded(campaign_id, donor))
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
