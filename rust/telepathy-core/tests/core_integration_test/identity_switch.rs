use super::common::{
    DEFAULT_SAMPLE_RATE, ManagerLifecycle, build_client_with_options, construct_mock_callbacks,
    init_test_tracing, shared_address_lookup, shared_relay_map,
};
use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::TelepathyHandle;
use telepathy_core::overlay::Overlay;
use telepathy_core::types::{CallState, CodecConfig, Contact, NetworkConfig, ScreenshareConfig};
use tokio::time::{sleep, timeout};

type MockHandle = TelepathyHandle<
    telepathy_core::internal::callbacks::MockCoreCallbacks<
        telepathy_core::internal::callbacks::MockCoreStatisticsCallback,
    >,
    telepathy_core::internal::callbacks::MockCoreStatisticsCallback,
    MockAudioHost<MockAudioInput, MockAudioOutput>,
    (),
    (),
>;

fn host() -> MockAudioHost<MockAudioInput, MockAudioOutput> {
    MockAudioHost::new(
        MockAudioInput::default(),
        DEFAULT_SAMPLE_RATE,
        MockAudioOutput,
        DEFAULT_SAMPLE_RATE,
    )
}

async fn wait_for_start_readiness(handle: &MockHandle, contact: &Contact) {
    let result = timeout(Duration::from_secs(10), async {
        loop {
            if handle.try_start_session(contact).await.is_ok() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if result.is_err() {
        panic!("manager did not permit starts after applying desired runtime");
    }
}

async fn restartable_client(
    identity: SecretKey,
) -> super::common::ClientHarness<MockAudioHost<MockAudioInput, MockAudioOutput>, (), ()> {
    build_client_with_options(
        shared_relay_map(),
        identity,
        Vec::new(),
        &CodecConfig::new(true, true, 5.0),
        host(),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Restartable,
    )
    .await
}

async fn preparation_rejects_duplicate_contacts_without_mutating_runtime_or_slot() {
    init_test_tracing();
    let initial_identity = SecretKey::generate();
    let client = restartable_client(initial_identity.clone()).await;
    let contact = match Contact::new(
        "duplicate target".to_string(),
        SecretKey::generate().public().to_string(),
    ) {
        Ok(contact) => contact,
        Err(error) => panic!("contact construction failed: {}", error.message),
    };
    let outcome = client
        .telepathy
        .prepare_identity_switch(
            SecretKey::generate().to_bytes(),
            vec![contact.clone(), contact.clone()],
        )
        .await;

    assert!(outcome.is_err());
    let prepared = match client
        .telepathy
        .prepare_identity_switch(initial_identity.to_bytes(), Vec::new())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => panic!("valid preparation stayed blocked: {error}"),
    };
    let mut blocked_start = Box::pin(client.telepathy.try_start_session(&contact));
    assert!(
        timeout(Duration::from_millis(100), &mut blocked_start)
            .await
            .is_err()
    );
    drop(prepared);
    match timeout(Duration::from_secs(1), &mut blocked_start).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("session start failed after token drop: {error}"),
        Err(_) => panic!("session start stayed blocked after token drop"),
    }
    let released = client
        .telepathy
        .prepare_identity_switch(initial_identity.to_bytes(), Vec::new())
        .await;
    match released {
        Ok(token) => drop(token),
        Err(error) => panic!("dropped token did not release its lease: {error}"),
    }
    client.telepathy.shutdown().await;
}

async fn occupied_preparation_preserves_existing_slot_and_runtime() {
    init_test_tracing();
    let client = restartable_client(SecretKey::generate()).await;
    let room = client.telepathy.join_room(Vec::new()).await;
    if let Err(error) = room {
        panic!("room start could not occupy call slot: {error}");
    }

    let outcome = client
        .telepathy
        .prepare_identity_switch(SecretKey::generate().to_bytes(), Vec::new())
        .await;

    assert!(outcome.is_err());
    client.telepathy.end_call().await;
    client.telepathy.shutdown().await;
}

async fn latest_committed_revision_supersedes_stale_manager_generation() {
    init_test_tracing();
    let client = restartable_client(SecretKey::generate()).await;
    let first = SecretKey::generate();
    let second = SecretKey::generate();

    let first_prepared = match client
        .telepathy
        .prepare_identity_switch(first.to_bytes(), Vec::new())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => panic!("first preparation failed: {error}"),
    };
    if let Err(error) = first_prepared.commit().await {
        panic!("first commit failed: {error}");
    }
    let second_prepared = match client
        .telepathy
        .prepare_identity_switch(second.to_bytes(), Vec::new())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => panic!("second preparation failed: {error}"),
    };
    if let Err(error) = second_prepared.commit().await {
        panic!("second commit failed: {error}");
    }
    let contact = match Contact::new(
        "latest revision".to_string(),
        SecretKey::generate().public().to_string(),
    ) {
        Ok(contact) => contact,
        Err(error) => panic!("contact construction failed: {}", error.message),
    };
    wait_for_start_readiness(&client.telepathy, &contact).await;
    client.telepathy.shutdown().await;
}

async fn commit_returns_after_committed_revision_is_applied() {
    init_test_tracing();
    let client = restartable_client(SecretKey::generate()).await;
    let target = SecretKey::generate();
    let mut contacts = Vec::new();
    for index in 0..9 {
        match Contact::new(
            format!("rehydration target {index}"),
            SecretKey::generate().public().to_string(),
        ) {
            Ok(contact) => contacts.push(contact),
            Err(error) => panic!("contact construction failed: {}", error.message),
        }
    }
    let contact = contacts[0].clone();
    let prepared = match client
        .telepathy
        .prepare_identity_switch(target.to_bytes(), contacts)
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => panic!("preparation failed: {error}"),
    };
    if let Err(error) = prepared.commit().await {
        panic!("commit failed: {error}");
    }
    assert!(client.telepathy.try_start_session(&contact).await.is_ok());
    client.telepathy.shutdown().await;
}

async fn manager_retries_after_missing_identity_until_latest_runtime_arrives() {
    init_test_tracing();
    let network_config = NetworkConfig::mock(
        0,
        shared_relay_map(),
        None,
        None,
        None,
        Some(shared_address_lookup().clone()),
    );
    let callbacks = construct_mock_callbacks(
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(Vec::<CallState>::new())),
        None,
        ManagerLifecycle::Restartable,
        None,
        None,
        None,
    );
    let mut handle: MockHandle = TelepathyHandle::new(
        host(),
        &network_config,
        &ScreenshareConfig::default(),
        &Overlay::default(),
        &CodecConfig::new(true, true, 5.0),
        callbacks,
    );
    let contact = match Contact::new(
        "fresh handle".to_string(),
        SecretKey::generate().public().to_string(),
    ) {
        Ok(contact) => contact,
        Err(error) => panic!("contact construction failed: {}", error.message),
    };
    assert!(handle.try_start_session(&contact).await.is_err());
    handle.start_manager().await;
    sleep(Duration::from_millis(20)).await;
    let latest = SecretKey::generate();
    let outcome = handle.set_identity(&latest.to_bytes()).await;
    if let Err(error) = outcome {
        panic!("identity update failed: {error}");
    }
    let contact = match Contact::new(
        "retry readiness".to_string(),
        SecretKey::generate().public().to_string(),
    ) {
        Ok(contact) => contact,
        Err(error) => panic!("contact construction failed: {}", error.message),
    };
    wait_for_start_readiness(&handle, &contact).await;
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_switch_invariants() {
    preparation_rejects_duplicate_contacts_without_mutating_runtime_or_slot().await;
    occupied_preparation_preserves_existing_slot_and_runtime().await;
    latest_committed_revision_supersedes_stale_manager_generation().await;
    commit_returns_after_committed_revision_is_applied().await;
    manager_retries_after_missing_identity_until_latest_runtime_arrives().await;
}
