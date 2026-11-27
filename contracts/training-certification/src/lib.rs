#![no_std]

//! # Training Certification
//!
//! Records the training of Iwosan personnel on-chain, so buyers, employers and
//! other contracts can check it:
//!
//! - the admin accredits trainers (hospitals, nursing schools, pharmacist
//!   associations, first-aid instructors),
//! - a trainer publishes courses in one of three tracks: selling medical
//!   equipment, selling orthodox and native drugs, and first aid for common
//!   diseases and emergencies,
//! - the trainer issues a certificate to a trainee who reached the course's
//!   pass mark; certificates expire and can be revoked,
//! - anyone can call `has_valid_certificate(person, track)`.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, Address,
    BytesN, Env, String, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotTrainer = 1,
    NotAuthorized = 2,
    CourseNotFound = 3,
    CertificateNotFound = 4,
    ScoreTooLow = 5,
    InvalidCourse = 6,
    CourseInactive = 7,
    InvalidScore = 8,
}

/// What a course trains people to do.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrainingTrack {
    /// Selling and renting medical equipment safely.
    EquipmentSales,
    /// Selling and dispensing orthodox and native drugs safely.
    DrugSales,
    /// First aid for common diseases and emergencies.
    FirstAid,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Course {
    pub id: u32,
    pub trainer: Address,
    pub track: TrainingTrack,
    pub title: String,
    /// Hash of the off-chain course material (lessons and quiz).
    pub content_hash: BytesN<32>,
    /// Minimum score out of 100 needed to pass.
    pub pass_score: u32,
    /// How long a certificate from this course stays valid.
    pub validity_secs: u64,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Certificate {
    pub course_id: u32,
    pub holder: Address,
    pub track: TrainingTrack,
    pub trainer: Address,
    /// Score out of 100.
    pub score: u32,
    pub issued_at: u64,
    pub expires_at: u64,
    pub revoked: bool,
}

#[contracttype]
enum DataKey {
    Admin,
    Trainer(Address),
    NextCourseId,
    Course(u32),
    Certificate(Address, u32),
    /// Courses in a track that a person holds a certificate for.
    HolderCourses(Address, TrainingTrack),
}

#[contractevent(topics = ["trainer_add"])]
pub struct TrainerAdded {
    pub trainer: Address,
}

#[contractevent(topics = ["trainer_rem"])]
pub struct TrainerRemoved {
    pub trainer: Address,
}

#[contractevent(topics = ["course_new"])]
pub struct CourseCreated {
    #[topic]
    pub course_id: u32,
    pub trainer: Address,
    pub track: TrainingTrack,
}

#[contractevent(topics = ["cert_issued"])]
pub struct CertificateIssued {
    #[topic]
    pub holder: Address,
    pub course_id: u32,
    pub track: TrainingTrack,
    pub score: u32,
    pub expires_at: u64,
}

#[contractevent(topics = ["cert_revoked"])]
pub struct CertificateRevoked {
    #[topic]
    pub holder: Address,
    pub course_id: u32,
}

#[contract]
pub struct TrainingCertification;

#[contractimpl]
impl TrainingCertification {
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Admin accredits a trainer.
    pub fn add_trainer(env: Env, trainer: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::Trainer(trainer.clone()), &true);
        TrainerAdded { trainer }.publish(&env);
    }

    /// Admin withdraws accreditation. Certificates already issued stay valid
    /// until they expire or are revoked; no new ones can be issued.
    pub fn remove_trainer(env: Env, trainer: Address) {
        Self::admin(&env).require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::Trainer(trainer.clone()));
        TrainerRemoved { trainer }.publish(&env);
    }

    pub fn is_trainer(env: Env, trainer: Address) -> bool {
        env.storage().persistent().has(&DataKey::Trainer(trainer))
    }

    /// An accredited trainer publishes a course.
    pub fn create_course(
        env: Env,
        trainer: Address,
        track: TrainingTrack,
        title: String,
        content_hash: BytesN<32>,
        pass_score: u32,
        validity_secs: u64,
    ) -> u32 {
        trainer.require_auth();
        Self::require_trainer(&env, &trainer);
        if pass_score == 0 || pass_score > 100 || validity_secs == 0 {
            panic_with_error!(&env, Error::InvalidCourse);
        }

        let id: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextCourseId)
            .unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::NextCourseId, &(id + 1));

        let course = Course {
            id,
            trainer: trainer.clone(),
            track,
            title,
            content_hash,
            pass_score,
            validity_secs,
            active: true,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Course(id), &course);
        CourseCreated {
            course_id: id,
            trainer,
            track,
        }
        .publish(&env);
        id
    }

    /// The course's trainer or the admin stops new certificates for a course.
    pub fn deactivate_course(env: Env, caller: Address, course_id: u32) {
        caller.require_auth();
        let mut course = Self::get_course(env.clone(), course_id);
        if caller != course.trainer && caller != Self::admin(&env) {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        course.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Course(course_id), &course);
    }

    /// The course's trainer certifies a trainee who reached the pass mark.
    /// Passing the same course again renews the certificate.
    pub fn issue_certificate(
        env: Env,
        trainer: Address,
        course_id: u32,
        trainee: Address,
        score: u32,
    ) {
        trainer.require_auth();
        Self::require_trainer(&env, &trainer);
        let course = Self::get_course(env.clone(), course_id);
        if course.trainer != trainer {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        if !course.active {
            panic_with_error!(&env, Error::CourseInactive);
        }
        if score > 100 {
            panic_with_error!(&env, Error::InvalidScore);
        }
        if score < course.pass_score {
            panic_with_error!(&env, Error::ScoreTooLow);
        }

        let now = env.ledger().timestamp();
        let certificate = Certificate {
            course_id,
            holder: trainee.clone(),
            track: course.track,
            trainer,
            score,
            issued_at: now,
            expires_at: now + course.validity_secs,
            revoked: false,
        };
        env.storage().persistent().set(
            &DataKey::Certificate(trainee.clone(), course_id),
            &certificate,
        );

        let courses_key = DataKey::HolderCourses(trainee.clone(), course.track);
        let mut courses: Vec<u32> = env
            .storage()
            .persistent()
            .get(&courses_key)
            .unwrap_or_else(|| Vec::new(&env));
        if !courses.contains(course_id) {
            courses.push_back(course_id);
            env.storage().persistent().set(&courses_key, &courses);
        }

        CertificateIssued {
            holder: trainee,
            course_id,
            track: course.track,
            score,
            expires_at: certificate.expires_at,
        }
        .publish(&env);
    }

    /// The course's trainer or the admin revokes a certificate.
    pub fn revoke_certificate(env: Env, caller: Address, holder: Address, course_id: u32) {
        caller.require_auth();
        let course = Self::get_course(env.clone(), course_id);
        if caller != course.trainer && caller != Self::admin(&env) {
            panic_with_error!(&env, Error::NotAuthorized);
        }
        let key = DataKey::Certificate(holder.clone(), course_id);
        let mut certificate: Certificate = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::CertificateNotFound));
        certificate.revoked = true;
        env.storage().persistent().set(&key, &certificate);
        CertificateRevoked { holder, course_id }.publish(&env);
    }

    pub fn get_course(env: Env, course_id: u32) -> Course {
        env.storage()
            .persistent()
            .get(&DataKey::Course(course_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::CourseNotFound))
    }

    pub fn get_certificate(env: Env, holder: Address, course_id: u32) -> Certificate {
        env.storage()
            .persistent()
            .get(&DataKey::Certificate(holder, course_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::CertificateNotFound))
    }

    /// `true` when `person` holds an unrevoked, unexpired certificate from any
    /// course in `track`.
    pub fn has_valid_certificate(env: Env, person: Address, track: TrainingTrack) -> bool {
        let courses: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::HolderCourses(person.clone(), track))
            .unwrap_or_else(|| Vec::new(&env));
        let now = env.ledger().timestamp();
        courses.iter().any(|course_id| {
            env.storage()
                .persistent()
                .get::<DataKey, Certificate>(&DataKey::Certificate(person.clone(), course_id))
                .is_some_and(|cert| !cert.revoked && cert.expires_at > now)
        })
    }

    fn require_trainer(env: &Env, trainer: &Address) {
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Trainer(trainer.clone()))
        {
            panic_with_error!(env, Error::NotTrainer);
        }
    }

    fn admin(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }
}

#[cfg(test)]
mod test;
