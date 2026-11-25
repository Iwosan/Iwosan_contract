#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, BytesN, Env, String, Symbol, TryFromVal, Vec,
};

fn setup() -> (Env, DrugBatchRegistryClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let admin = Address::generate(&env);
    let id = env.register(DrugBatchRegistry, (&admin,));
    let client = DrugBatchRegistryClient::new(&env, &id);
    (env, client, admin)
}

fn batch_id(env: &Env, seed: u8) -> BytesN<32> {
    BytesN::from_array(env, &[seed; 32])
}

fn register(
    env: &Env,
    client: &DrugBatchRegistryClient,
    manufacturer: &Address,
    batch: &BytesN<32>,
) {
    client.register_batch(
        manufacturer,
        batch,
        &String::from_str(env, "Paracetamol 500mg"),
        &DrugKind::OrthodoxDrug,
        &2_000,
        &String::from_str(env, "NAFDAC-04-1234"),
    );
}

#[test]
fn registered_manufacturer_can_create_a_batch() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);

    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    let batch = client.get_batch(&id);
    assert_eq!(batch.manufacturer, manufacturer);
    assert_eq!(batch.holder, manufacturer);
    assert_eq!(batch.kind, DrugKind::OrthodoxDrug);
    assert!(!batch.recalled);
    assert_eq!(client.get_history(&id).len(), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn unregistered_manufacturer_cannot_create_a_batch() {
    let (env, client, _admin) = setup();
    let stranger = Address::generate(&env);
    register(&env, &client, &stranger, &batch_id(&env, 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn batch_ids_are_unique() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);
    register(&env, &client, &manufacturer, &id);
}

#[test]
fn custody_transfers_are_recorded_in_history() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    let distributor = Address::generate(&env);
    let pharmacy = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    client.transfer_custody(&manufacturer, &distributor, &id);
    client.transfer_custody(&distributor, &pharmacy, &id);

    assert_eq!(client.get_batch(&id).holder, pharmacy);
    let history = client.get_history(&id);
    assert_eq!(history.len(), 3);
    assert_eq!(history.get(0).unwrap().from, None);
    assert_eq!(history.get(0).unwrap().to, manufacturer);
    assert_eq!(history.get(1).unwrap().from, Some(manufacturer));
    assert_eq!(history.get(1).unwrap().to, distributor);
    assert_eq!(history.get(2).unwrap().from, Some(distributor));
    assert_eq!(history.get(2).unwrap().to, pharmacy);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn non_holder_cannot_transfer_custody() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    let thief = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    client.transfer_custody(&thief, &thief, &id);
}

#[test]
fn manufacturer_can_recall_a_batch() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    client.recall_batch(&manufacturer, &id, &BytesN::from_array(&env, &[7; 32]));

    assert!(client.is_recalled(&id));
}

#[test]
fn admin_can_recall_a_batch() {
    let (env, client, admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    client.recall_batch(&admin, &id, &BytesN::from_array(&env, &[9; 32]));

    assert!(client.is_recalled(&id));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn other_callers_cannot_recall_a_batch() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    let outsider = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    client.recall_batch(&outsider, &id, &BytesN::from_array(&env, &[0; 32]));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn a_batch_cannot_be_recalled_twice() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    client.recall_batch(&manufacturer, &id, &BytesN::from_array(&env, &[1; 32]));
    client.recall_batch(&manufacturer, &id, &BytesN::from_array(&env, &[2; 32]));
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn recalled_batches_cannot_change_hands() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    let distributor = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);
    client.recall_batch(&manufacturer, &id, &BytesN::from_array(&env, &[3; 32]));

    client.transfer_custody(&manufacturer, &distributor, &id);
}

#[test]
fn recalled_batches_keep_their_history() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    let distributor = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);
    client.transfer_custody(&manufacturer, &distributor, &id);
    client.recall_batch(&manufacturer, &id, &BytesN::from_array(&env, &[4; 32]));

    assert_eq!(client.get_history(&id).len(), 2);
    assert_eq!(client.get_batch(&id).holder, distributor);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn batches_cannot_be_registered_already_expired() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);

    client.register_batch(
        &manufacturer,
        &batch_id(&env, 1),
        &String::from_str(&env, "Expired syrup"),
        &DrugKind::NativeDrug,
        &500,
        &String::from_str(&env, "NAFDAC-04-0001"),
    );
}

fn event_names(env: &Env) -> Vec<Symbol> {
    let mut names = Vec::new(env);
    for (_, topics, _) in env.events().all().iter() {
        if let Some(topic) = topics.get(0) {
            if let Ok(name) = Symbol::try_from_val(env, &topic) {
                names.push_back(name);
            }
        }
    }
    names
}

#[test]
fn registration_emits_a_registered_event() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);

    assert!(event_names(&env).contains(Symbol::new(&env, "registered")));
}

#[test]
fn transfers_emit_a_custody_event() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    let distributor = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);
    client.transfer_custody(&manufacturer, &distributor, &id);

    assert!(event_names(&env).contains(Symbol::new(&env, "custody")));
}

#[test]
fn recall_emits_a_recalled_event() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    let id = batch_id(&env, 1);
    register(&env, &client, &manufacturer, &id);
    client.recall_batch(&manufacturer, &id, &BytesN::from_array(&env, &[5; 32]));

    assert!(event_names(&env).contains(Symbol::new(&env, "recalled")));
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn unknown_batches_cannot_be_read() {
    let (env, client, _admin) = setup();
    client.get_history(&batch_id(&env, 42));
}

#[test]
fn removed_manufacturers_cannot_register_new_batches() {
    let (env, client, _admin) = setup();
    let manufacturer = Address::generate(&env);
    client.add_manufacturer(&manufacturer);
    assert!(client.is_manufacturer(&manufacturer));

    client.remove_manufacturer(&manufacturer);

    assert!(!client.is_manufacturer(&manufacturer));
}
