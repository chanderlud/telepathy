use super::common::{
    DEFAULT_SAMPLE_RATE, ManagerActiveGate, ManagerLifecycle, ManagerStartingGate,
    construct_mock_callbacks, init_test_tracing, shared_address_lookup, shared_relay_map,
};
use iroh::SecretKey;
use std::future::poll_fn;
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::TelepathyHandle;
use telepathy_core::overlay::Overlay;
use telepathy_core::types::{CallState, CodecConfig, Contact, NetworkConfig, ScreenshareConfig};
use tokio::time::{Duration, timeout};

type MockHandle = TelepathyHandle<
    telepathy_core::internal::callbacks::MockCoreCallbacks<
        telepathy_core::internal::callbacks::MockCoreStatisticsCallback,
    >,
    telepathy_core::internal::callbacks::MockCoreStatisticsCallback,
    MockAudioHost<MockAudioInput, MockAudioOutput>,
    (),
    (),
>;

fn mock_handle(network_config: &NetworkConfig, lifecycle: ManagerLifecycle) -> MockHandle {
    let callbacks = construct_mock_callbacks(
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(Vec::<CallState>::new())),
        None,
        lifecycle,
        None,
        None,
        None,
    );
    TelepathyHandle::new(
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        network_config,
        &ScreenshareConfig::default(),
        &Overlay::default(),
        &CodecConfig::new(true, true, 5.0),
        callbacks,
    )
}

fn runtime_config(listen_port: u16) -> NetworkConfig {
    NetworkConfig::mock(
        listen_port,
        shared_relay_map(),
        None,
        None,
        None,
        Some(shared_address_lookup().clone()),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_handle_rejects_all_runtime_dependent_starts() {
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
    let handle: MockHandle = TelepathyHandle::new(
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        &network_config,
        &ScreenshareConfig::default(),
        &Overlay::default(),
        &CodecConfig::new(true, true, 5.0),
        callbacks,
    );
    let contact = match Contact::new(
        "unapplied runtime".to_string(),
        SecretKey::generate().public().to_string(),
    ) {
        Ok(contact) => contact,
        Err(error) => panic!("contact construction failed: {}", error.message),
    };

    for result in [
        handle.try_start_session(&contact).await,
        handle.start_call(&contact).await,
        handle.join_room(Vec::new()).await,
        handle.audio_test().await,
    ] {
        match result {
            Err(error) => assert_eq!(
                error.to_string(),
                "Runtime configuration has not been applied by the session manager"
            ),
            Ok(()) => panic!("fresh handle accepted an operation before runtime application"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_manager_returns_setup_failure_when_endpoint_cannot_bind() {
    init_test_tracing();
    let occupied_socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(error) => panic!("failed to reserve UDP port: {error}"),
    };
    let port = match occupied_socket.local_addr() {
        Ok(address) => address.port(),
        Err(error) => panic!("failed to inspect reserved UDP port: {error}"),
    };
    let mut handle = mock_handle(&runtime_config(port), ManagerLifecycle::Restartable);
    let identity = SecretKey::generate();

    match handle.set_identity(&identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install test identity: {error}"),
    }
    handle.start_manager().await;

    let error = match timeout(Duration::from_secs(2), handle.restart_manager()).await {
        Ok(Err(error)) => error,
        Ok(Ok(())) => panic!("restart manager succeeded despite occupied endpoint port"),
        Err(_) => panic!("restart manager did not return setup failure"),
    };
    assert_eq!(
        error.to_string(),
        "Session manager setup failed before the runtime configuration was applied"
    );

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn start_manager_and_waits_for_runtime_application() {
    init_test_tracing();
    let gate = ManagerStartingGate::new();
    let mut handle = mock_handle(
        &runtime_config(0),
        ManagerLifecycle::StartingGate(gate.clone()),
    );
    let identity = SecretKey::generate();

    match handle.set_identity(&identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install test identity: {error}"),
    }

    let mut start = Box::pin(handle.start_manager_and_wait());
    let start_is_pending = poll_fn(|context| match start.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(_) => Poll::Ready(false),
    })
    .await;
    assert!(
        start_is_pending,
        "start manager completed before manager setup was released"
    );

    gate.wait_started().await;
    gate.release();
    match timeout(Duration::from_secs(2), start).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("manager runtime did not apply: {error}"),
        Err(_) => panic!("start manager did not wait for runtime application"),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_active_callback_does_not_block_shutdown_after_runtime_is_ready() {
    init_test_tracing();
    let gate = ManagerActiveGate::new();
    let mut handle = mock_handle(
        &runtime_config(0),
        ManagerLifecycle::ActiveGate(gate.clone()),
    );
    let identity = SecretKey::generate();

    match handle.set_identity(&identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install test identity: {error}"),
    }

    match timeout(Duration::from_secs(2), handle.start_manager_and_wait()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("manager runtime did not apply: {error}"),
        Err(_) => panic!("manager runtime did not become ready"),
    }
    gate.wait_active().await;

    match timeout(Duration::from_secs(2), handle.shutdown()).await {
        Ok(()) => {}
        Err(_) => panic!("blocked Active callback prevented manager shutdown"),
    }
    gate.release();
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_manager_returns_superseded_when_identity_changes_before_setup() {
    init_test_tracing();
    let gate = ManagerStartingGate::new();
    let mut handle = mock_handle(
        &runtime_config(0),
        ManagerLifecycle::StartingGate(gate.clone()),
    );
    let identity = SecretKey::generate();

    match handle.set_identity(&identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install test identity: {error}"),
    }
    handle.start_manager().await;
    gate.wait_started().await;

    let mut restart = Box::pin(handle.restart_manager());
    let restart_is_pending = poll_fn(|context| match restart.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(_) => Poll::Ready(false),
    })
    .await;
    assert!(
        restart_is_pending,
        "restart completed before manager setup was released"
    );

    let replacement_identity = SecretKey::generate();
    match handle.set_identity(&replacement_identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to supersede test identity: {error}"),
    }

    let error = match timeout(Duration::from_secs(2), &mut restart).await {
        Ok(Err(error)) => error,
        Ok(Ok(())) => panic!("superseded restart completed successfully"),
        Err(_) => panic!("superseded restart did not return"),
    };
    assert_eq!(
        error.to_string(),
        "Runtime configuration was superseded before the session manager applied it"
    );

    gate.release();
    match timeout(Duration::from_secs(2), handle.restart_manager()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("latest runtime did not apply: {error}"),
        Err(_) => panic!("latest runtime did not apply"),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_manager_returns_stopped_when_shutdown_interrupts_setup() {
    init_test_tracing();
    let gate = ManagerStartingGate::new();
    let mut handle = mock_handle(
        &runtime_config(0),
        ManagerLifecycle::StartingGate(gate.clone()),
    );
    let identity = SecretKey::generate();

    match handle.set_identity(&identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install test identity: {error}"),
    }
    handle.start_manager().await;
    gate.wait_started().await;

    let mut restart = Box::pin(handle.restart_manager());
    let restart_is_pending = poll_fn(|context| match restart.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(_) => Poll::Ready(false),
    })
    .await;
    assert!(restart_is_pending, "restart completed before shutdown");

    let mut shutdown = Box::pin(handle.shutdown());
    let shutdown_is_pending = poll_fn(|context| match shutdown.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(()) => Poll::Ready(false),
    })
    .await;
    assert!(
        shutdown_is_pending,
        "shutdown completed before setup gate release"
    );
    gate.release();
    shutdown.await;

    let error = match timeout(Duration::from_secs(2), restart).await {
        Ok(Err(error)) => error,
        Ok(Ok(())) => panic!("restart completed successfully after shutdown"),
        Err(_) => panic!("restart did not return after shutdown"),
    };
    assert_eq!(
        error.to_string(),
        "Session manager stopped before the runtime configuration was applied"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_token_commits_origin_runtime_after_origin_revision_applies() {
    init_test_tracing();
    let gate = ManagerStartingGate::new();
    let mut handle = mock_handle(
        &runtime_config(0),
        ManagerLifecycle::StartingGate(gate.clone()),
    );
    let initial_identity = SecretKey::generate();
    let target_identity = SecretKey::generate();

    match handle.set_identity(&initial_identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install initial identity: {error}"),
    }
    handle.start_manager().await;
    gate.wait_started().await;

    let prepared = match handle
        .prepare_identity_switch(target_identity.to_bytes(), Vec::new())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => panic!("failed to prepare identity switch: {error}"),
    };
    let mut commit = Box::pin(prepared.commit());
    let commit_is_pending = poll_fn(|context| match commit.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(_) => Poll::Ready(false),
    })
    .await;
    assert!(
        commit_is_pending,
        "prepared token committed before its origin runtime revision applied"
    );

    gate.release();
    match timeout(Duration::from_secs(2), commit).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("origin runtime did not apply: {error}"),
        Err(_) => panic!("prepared token did not wait for origin runtime application"),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn try_start_session_waits_for_runtime_then_dials() {
    init_test_tracing();
    let gate = ManagerStartingGate::new();
    let mut handle = mock_handle(
        &runtime_config(0),
        ManagerLifecycle::StartingGate(gate.clone()),
    );
    let identity = SecretKey::generate();

    match handle.set_identity(&identity.to_bytes()).await {
        Ok(()) => {}
        Err(error) => panic!("failed to install test identity: {error}"),
    }
    handle.start_manager().await;
    gate.wait_started().await;

    let contact = match Contact::new(
        "gated peer".to_string(),
        SecretKey::generate().public().to_string(),
    ) {
        Ok(contact) => contact,
        Err(error) => panic!("contact construction failed: {}", error.message),
    };

    let mut start_session = Box::pin(handle.try_start_session(&contact));
    let session_is_pending = poll_fn(|context| match start_session.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(_) => Poll::Ready(false),
    })
    .await;
    assert!(
        session_is_pending,
        "try_start_session completed before manager setup was released"
    );

    gate.release();
    match timeout(Duration::from_secs(5), start_session).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("try_start_session failed after gate release: {error}"),
        Err(_) => panic!("try_start_session did not complete after gate release"),
    }
    handle.shutdown().await;
}
