#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, String,
};

const YEAR: u64 = 365 * 24 * 60 * 60;

struct Setup {
    env: Env,
    client: TrainingCertificationClient<'static>,
    admin: Address,
    trainer: Address,
    trainee: Address,
    course: u32,
}

/// An accredited trainer with a first-aid course (pass mark 70, valid a year).
fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let admin = Address::generate(&env);
    let id = env.register(TrainingCertification, (&admin,));
    let client = TrainingCertificationClient::new(&env, &id);

    let trainer = Address::generate(&env);
    client.add_trainer(&trainer);
    let course = client.create_course(
        &trainer,
        &TrainingTrack::FirstAid,
        &String::from_str(&env, "First aid for common emergencies"),
        &BytesN::from_array(&env, &[1; 32]),
        &70,
        &YEAR,
    );

    Setup {
        trainee: Address::generate(&env),
        env,
        client,
        admin,
        trainer,
        course,
    }
}

#[test]
fn passing_trainee_gets_a_valid_certificate() {
    let s = setup();

    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &85);

    let cert = s.client.get_certificate(&s.trainee, &s.course);
    assert_eq!(cert.score, 85);
    assert_eq!(cert.track, TrainingTrack::FirstAid);
    assert_eq!(cert.expires_at, 1_000 + YEAR);
    assert!(s
        .client
        .has_valid_certificate(&s.trainee, &TrainingTrack::FirstAid));
}

#[test]
fn certificates_are_per_track() {
    let s = setup();
    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &90);

    assert!(!s
        .client
        .has_valid_certificate(&s.trainee, &TrainingTrack::DrugSales));
    assert!(!s
        .client
        .has_valid_certificate(&s.trainee, &TrainingTrack::EquipmentSales));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn failing_score_is_rejected() {
    let s = setup();

    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &69);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn score_above_100_is_rejected() {
    let s = setup();

    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn unaccredited_trainer_cannot_create_courses() {
    let s = setup();
    let stranger = Address::generate(&s.env);

    s.client.create_course(
        &stranger,
        &TrainingTrack::DrugSales,
        &String::from_str(&s.env, "Safe drug dispensing"),
        &BytesN::from_array(&s.env, &[2; 32]),
        &70,
        &YEAR,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn trainer_cannot_certify_for_another_trainers_course() {
    let s = setup();
    let other = Address::generate(&s.env);
    s.client.add_trainer(&other);

    s.client
        .issue_certificate(&other, &s.course, &s.trainee, &90);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn pass_score_must_be_between_1_and_100() {
    let s = setup();

    s.client.create_course(
        &s.trainer,
        &TrainingTrack::EquipmentSales,
        &String::from_str(&s.env, "Selling medical equipment"),
        &BytesN::from_array(&s.env, &[3; 32]),
        &101,
        &YEAR,
    );
}

#[test]
fn certificate_expires() {
    let s = setup();
    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &80);

    s.env.ledger().set_timestamp(1_000 + YEAR);

    assert!(!s
        .client
        .has_valid_certificate(&s.trainee, &TrainingTrack::FirstAid));
}

#[test]
fn passing_again_renews_an_expired_certificate() {
    let s = setup();
    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &80);
    s.env.ledger().set_timestamp(1_000 + YEAR);

    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &95);

    assert!(s
        .client
        .has_valid_certificate(&s.trainee, &TrainingTrack::FirstAid));
    assert_eq!(s.client.get_certificate(&s.trainee, &s.course).score, 95);
}

#[test]
fn admin_can_revoke_a_certificate() {
    let s = setup();
    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &80);

    s.client.revoke_certificate(&s.admin, &s.trainee, &s.course);

    assert!(s.client.get_certificate(&s.trainee, &s.course).revoked);
    assert!(!s
        .client
        .has_valid_certificate(&s.trainee, &TrainingTrack::FirstAid));
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn deactivated_course_issues_no_new_certificates() {
    let s = setup();
    s.client.deactivate_course(&s.trainer, &s.course);

    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &90);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn removed_trainer_cannot_issue() {
    let s = setup();
    s.client.remove_trainer(&s.trainer);

    s.client
        .issue_certificate(&s.trainer, &s.course, &s.trainee, &90);
}
