use super::*;
use crate::audio::{InputProcessorState, OutputProcessorState, input_processor, ChannelInput, ChannelOutput};
use crate::flutter::callbacks::{
    FrbStatisticsCallback, MockFrbCallbacks, MockFrbStatisticsCallback,
};
use fast_log::Config;
use kanal::{bounded, unbounded};
use log::{LevelFilter, info};
use nnnoiseless::DenoiseState;
use rand::Rng;
use rand::prelude::SliceRandom;
use relay_server::{RelayInfo, spawn_relay};
use sea_codec::ProcessorMessage;
use std::collections::HashMap;
use std::fs::read;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::thread::{sleep, spawn};
use std::time::Instant;
use tokio::sync::OnceCell;
use tokio::time::interval;

const HOGWASH_BYTES: &[u8] = include_bytes!("../../../../assets/models/hogwash.rnn");

static RELAY: OnceCell<RelayInfo> = OnceCell::const_new();

struct BenchmarkResult {
    average: Duration,
    min: Duration,
    max: Duration,
    end: Duration,
}

impl Default for InputProcessorState {
    fn default() -> Self {
        Self {
            input_volume: Arc::new(AtomicF32::new(1.0)),
            rms_threshold: Arc::new(AtomicF32::new(db_to_multiplier(50_f32))),
            muted: Arc::new(Default::default()),
            rms_sender: Arc::new(Default::default()),
        }
    }
}

impl Default for OutputProcessorState {
    fn default() -> Self {
        Self {
            output_volume: Arc::new(AtomicF32::new(1.0)),
            rms_sender: Arc::new(Default::default()),
            deafened: Arc::new(Default::default()),
            loss_sender: Arc::new(Default::default()),
        }
    }
}

impl Contact {
    fn mock(is_room_only: bool, nickname: &str) -> (Self, Keypair) {
        let key = Keypair::generate_ed25519();
        let peer_id = key.public().to_peer_id();
        (
            Self {
                id: peer_id.to_string(),
                nickname: nickname.to_string(),
                peer_id,
                is_room_only,
            },
            key,
        )
    }
}

impl<C, S> TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    fn mock(callbacks: C, network_config: &NetworkConfig, codec_config: &CodecConfig) -> Self {
        let screenshare_config = ScreenshareConfig::default();
        let overlay = Overlay::default();

        Self::new(
            Arc::new(Host::default()),
            network_config,
            &screenshare_config,
            &overlay,
            codec_config,
            callbacks,
        )
    }
}

#[tokio::test]
async fn mock_callbacks_test_network_matrix() {
    for profile in PROFILES {
        // pick ports (include relay if you want it impaired too)
        let _net = NetemGuard::apply(profile, &[40143, 40144]).expect("netem setup failed");

        // run your existing test logic with timeouts
        run_test(profile).await;
    }
}

async fn run_test(profile: &NetProfile) {
    fast_log::init(
        Config::new()
            .file("mock_callbacks.log")
            .level(LevelFilter::Debug),
    )
    .unwrap();

    // get local relay
    let relay: &RelayInfo = relay().await;

    // craft network config for the test instance
    let network_config_a = NetworkConfig::mock(
        "127.0.0.1:40142".parse().unwrap(),
        relay.peer_id,
        40143,
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
    );

    let network_config_b = NetworkConfig::mock(
        "127.0.0.1:40142".parse().unwrap(),
        relay.peer_id,
        40144,
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
    );

    // default codec config
    let codec_config = CodecConfig::new(true, true, 5.0);

    // create contacts & identities
    let (contact_a, key_a) = Contact::mock(false, "client-a");
    let (contact_b, key_b) = Contact::mock(false, "client-b");

    // set up client a
    let a_is_active = Arc::new(AtomicBool::new(false));
    let a_is_relayed = Arc::new(AtomicBool::new(false));
    let mock_a = construct_mock_callbacks(
        vec![contact_b.clone()],
        a_is_active.clone(),
        a_is_relayed.clone(),
    );
    let mut telepathy_a = TelepathyCore::mock(mock_a, &network_config_a, &codec_config);
    *telepathy_a.core_state.identity.write().await = Some(key_a);
    let handle_a = telepathy_a.start_manager().await;
    telepathy_a.core_state.manager_active.notified().await;

    // set up client b
    let b_is_active = Arc::new(AtomicBool::new(false));
    let b_is_relayed = Arc::new(AtomicBool::new(false));
    let mock_b = construct_mock_callbacks(
        vec![contact_a.clone()],
        b_is_active.clone(),
        b_is_relayed.clone(),
    );
    let mut telepathy_b = TelepathyCore::mock(mock_b, &network_config_b, &codec_config);
    *telepathy_b.core_state.identity.write().await = Some(key_b);
    let handle_b = telepathy_b.start_manager().await;
    telepathy_b.core_state.manager_active.notified().await;

    // a starts session with b
    telepathy_a
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.peer_id)
        .await
        .unwrap();

    // b starts session with a
    telepathy_b
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_a.peer_id)
        .await
        .unwrap();

    // poll for the session status callback to become connected
    let mut interval = interval(Duration::from_millis(100));
    loop {
        interval.tick().await;
        let a_active = a_is_active.load(Relaxed);
        let b_active = b_is_active.load(Relaxed);

        if a_active && b_active {
            info!("both clients got connected");
            break;
        }
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    // grab session states for inspection
    let b_session = telepathy_a
        .session_states
        .read()
        .await
        .get(&contact_b.peer_id)
        .cloned()
        .unwrap();
    let a_session = telepathy_b
        .session_states
        .read()
        .await
        .get(&contact_a.peer_id)
        .cloned()
        .unwrap();

    info!("session state a: {:?}", a_session);
    info!("session state b: {:?}", b_session);

    // direct connections should have been established
    assert!(!a_is_relayed.load(Relaxed));
    assert!(!b_is_relayed.load(Relaxed));

    // each client starts a call with the other at the same time
    a_session.start_call.notify_one();
    b_session.start_call.notify_one();

    tokio::time::sleep(Duration::from_secs(5)).await;

    // ensure shutdown is a success
    telepathy_a.shutdown().await;
    telepathy_b.shutdown().await;
    handle_a.unwrap().await.unwrap();
    handle_b.unwrap().await.unwrap();
}

/// returns mock callbacks that will establish a telepathy instance with the provided contacts
/// sets is_active to true when the first session connected event is received
fn construct_mock_callbacks(
    contacts: Vec<Contact>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
) -> MockFrbCallbacks<MockFrbStatisticsCallback> {
    let mut mock: MockFrbCallbacks<MockFrbStatisticsCallback> = MockFrbCallbacks::new();

    // handle session status callbacks
    mock.expect_session_status().returning(move |status, peer| {
        info!("session status got called {status:?} {peer}");
        let is_active_clone = is_active.clone();
        let is_relayed_clone = is_relayed.clone();
        Box::pin(async move {
            match status {
                SessionStatus::Connected { relayed } => {
                    is_active_clone.store(true, Relaxed);
                    is_relayed_clone.store(relayed, Relaxed);
                }
                _ => (),
            }
        })
    });

    // ensure manager activates
    mock.expect_manager_active()
        .withf(|a, b| *a && *b)
        .once()
        .returning(|_, _| Box::pin(async move {}));

    // ensure manager deactivates
    mock.expect_manager_active()
        .withf(|a, b| !a && !b)
        .once()
        .returning(|_, _| Box::pin(async move {}));

    // return the contacts
    let contacts_clone = contacts.clone();
    mock.expect_get_contacts().returning(move || {
        let contacts_clone = contacts_clone.clone();
        Box::pin(async move { contacts_clone })
    });

    mock.expect_get_contact().returning(move |peer_id| {
        let contacts_clone = contacts.clone();
        Box::pin(async move {
            for contact in contacts_clone.iter() {
                if contact.peer_id.to_bytes() == peer_id {
                    return Some(contact.clone());
                }
            }

            None
        })
    });

    // immediately accepts prompt for calls
    mock.expect_get_accept_handle().returning(move |_, _, _| {
        info!("accept call called");
        tokio::spawn(async move { true })
    });

    mock.expect_call_state().returning(|state| {
        info!("got call state: {state:?}");
        Box::pin(async move {})
    });

    mock
}

async fn relay() -> &'static RelayInfo {
    RELAY
        .get_or_init(|| async {
            // panic on error here is ok in tests
            spawn_relay(true).await.expect("failed to start relay")
        })
        .await
}

#[ignore]
#[test]
fn benchmark() {
    fast_log::init(Config::new().file("bench.log").level(LevelFilter::Trace)).unwrap();

    let sample_rate = 44_100;

    let mut samples = Vec::new();
    let bytes = read("../bench.raw").unwrap();

    for chunk in bytes.chunks(4) {
        let sample = f32::from_ne_bytes(chunk.try_into().unwrap());
        samples.push(sample);
    }

    // warmup
    for _ in 0..5 {
        simulate_input_stack(false, false, sample_rate, &samples, 2400);
    }

    let num_iterations = 10;
    let mut results: HashMap<(bool, bool), (Vec<Duration>, Duration)> = HashMap::new();

    for _ in 0..num_iterations {
        let mut cases = vec![(false, false), (false, true), (true, false), (true, true)];
        cases.shuffle(&mut rand::thread_rng()); // Shuffle for each iteration

        for (denoise, codec_enabled) in cases {
            let (durations, end, _) =
                simulate_input_stack(denoise, codec_enabled, sample_rate, &samples, 2400);

            // Update the results in a cumulative way
            results
                .entry((denoise, codec_enabled))
                .and_modify(|(all_durations, total_time)| {
                    all_durations.extend(durations.clone());
                    *total_time += end;
                })
                .or_insert((durations, end));
        }
    }

    // compute final averages
    for ((_denoise, _codec_enabled), (_durations, total_time)) in results.iter_mut() {
        *total_time /= num_iterations as u32; // Average total runtime
    }

    compare_runs(results);
}

#[ignore]
#[test]
fn packet_burst_simulation() {
    fast_log::init(
        Config::new()
            .file("burst_simulation.log")
            .level(LevelFilter::Trace),
    )
    .unwrap();

    let sample_rate = 44_100;
    let codec_enabled = true;

    let mut samples = Vec::new();
    let bytes = read("../bench.raw").unwrap();

    let mut duration = 0.0;
    let length = 1_f64 / sample_rate as f64;
    for chunk in bytes.chunks(4) {
        let sample = f32::from_ne_bytes(chunk.try_into().unwrap());
        samples.push(sample);
        duration += length;
    }
    let audio_duration = Duration::from_secs_f64(duration);
    info!(
        "loaded audio with length {:?} samples_len={}",
        audio_duration,
        samples.len()
    );

    let now = Instant::now();
    // use the input stack simulator to construct realistic stream of ProcessorMessage
    let (_, _, messages) = simulate_input_stack(
        true,
        codec_enabled,
        sample_rate,
        &samples,
        crate::telepathy::CHANNEL_SIZE,
    );
    info!(
        "processed {} messages in {:?}",
        messages.len(),
        now.elapsed()
    );

    let now = Instant::now();
    // use the output stack simulator to process the messages in a burst situation
    let received_samples = simulate_output_stack(
        messages,
        crate::telepathy::CHANNEL_SIZE,
        codec_enabled,
        sample_rate as f64,
        sample_rate as f64 / 48_000_f64,
    );
    info!(
        "received {} samples in {:?} aprox {}",
        received_samples.len(),
        now.elapsed(),
        received_samples.len() as f64 / sample_rate as f64
    );

    // save processed samples to output file
    let mut output = std::fs::File::create("../bench-out.raw").unwrap();
    for sample in received_samples {
        output.write(sample.to_ne_bytes().as_slice()).unwrap();
    }
}

fn simulate_input_stack(
    denoise: bool,
    codec_enabled: bool,
    sample_rate: u32,
    samples: &[f32],
    channel_size: usize,
) -> (Vec<Duration>, Duration, Vec<ProcessorMessage>) {
    // input stream -> input processor
    let (input_sender, input_receiver) = bounded(channel_size);
    let processor_input = ChannelInput::from(input_receiver);

    // input processor -> encoder or dummy
    let (processed_input_sender, processed_input_receiver) = unbounded::<ProcessorMessage>();

    // encoder -> dummy
    let (encoded_input_sender, encoded_input_receiver) = unbounded::<ProcessorMessage>();

    let model = RnnModel::from_bytes(HOGWASH_BYTES).unwrap();
    let denoiser = denoise.then_some(DenoiseState::from_model(model));

    spawn(move || {
        let result = input_processor(
            processor_input,
            processed_input_sender,
            sample_rate as f64,
            denoiser,
            codec_enabled,
            InputProcessorState::default(),
        );

        if let Err(error) = result {
            error!("{}", error);
        }
    });

    let output_receiver = if codec_enabled {
        spawn(move || {
            crate::audio::codec::encoder(
                processed_input_receiver,
                encoded_input_sender,
                if denoise { 48_000 } else { sample_rate },
                true,
                5.0,
                false,
            );
        });

        encoded_input_receiver
    } else {
        processed_input_receiver
    };

    let handle = spawn(move || {
        let start = Instant::now();
        let mut now = Instant::now();
        let mut durations = Vec::new();
        let mut messages = Vec::new();

        while let Ok(message) = output_receiver.recv() {
            durations.push(now.elapsed());
            now = Instant::now();
            messages.push(message);
        }

        let end = start.elapsed();
        (durations, end, messages)
    });

    for sample in samples {
        input_sender.send(*sample).unwrap();
    }
    _ = input_sender.close();
    handle.join().unwrap()
}

fn simulate_output_stack(
    input: Vec<ProcessorMessage>,
    channel_size: usize,
    codec_enabled: bool,
    sample_rate: f64,
    ratio: f64,
) -> Vec<f32> {
    // receiving socket -> output processor or decoder
    let (network_output_sender, network_output_receiver) = unbounded_async::<ProcessorMessage>();

    // decoder -> output processor
    let (decoded_output_sender, decoded_output_receiver) = unbounded_async::<ProcessorMessage>();

    // output processor -> dummy output stream
    let (output_sender, output_receiver) = bounded::<f32>(channel_size * 4);
    let processor_output = ChannelOutput::from(output_sender);

    let output_processor_receiver = if codec_enabled {
        spawn(move || {
            crate::audio::codec::decoder(
                network_output_receiver.to_sync(),
                decoded_output_sender.to_sync(),
                None,
            );
        });

        decoded_output_receiver.to_sync()
    } else {
        network_output_receiver.to_sync()
    };

    spawn(move || {
        crate::audio::output_processor(
            output_processor_receiver,
            processor_output,
            ratio,
            OutputProcessorState::default(),
        )
    });

    // simulate network dumping burst of packets into sender
    let sender = network_output_sender.to_sync();
    spawn(move || {
        let interval = Duration::from_secs_f64(FRAME_SIZE as f64 / sample_rate);
        let mut c = 0;

        for i in input {
            _ = sender.send(i);
            c += 1;

            // big ol lag spike + packet dump
            if c < 525 || c > 550 {
                sleep(interval);
            } else if c == 500 {
                sleep(Duration::from_millis(250));
            }
        }
    });

    let mut result = Vec::new();

    // mildly accurate simulation of an output stream reading at sample_rate
    let interval = Duration::from_secs_f64(2048_f64 / sample_rate);
    'outer: loop {
        for _ in 0..2048 {
            if let Ok(sample) = output_receiver.recv() {
                result.push(sample);
            } else {
                break 'outer;
            }
        }

        sleep(interval);
    }

    result
}

fn compute_statistics(durations: &[Duration]) -> (Duration, Duration, Duration) {
    let sum: Duration = durations.iter().sum();
    let average = sum / durations.len() as u32;

    let min = *durations.iter().min().unwrap();
    let max = *durations.iter().max().unwrap();

    (average, min, max)
}

fn compare_runs(benchmark_results: HashMap<(bool, bool), (Vec<Duration>, Duration)>) {
    let mut summary: HashMap<(bool, bool), BenchmarkResult> = HashMap::new();

    for ((denoise, codec_enabled), (durations, end)) in benchmark_results {
        let (average, min, max) = compute_statistics(&durations);
        summary.insert(
            (denoise, codec_enabled),
            BenchmarkResult {
                average,
                min,
                max,
                end,
            },
        );
    }

    info!("\nComparison of Runs:");
    info!("===================================================");
    info!(" Denoise | Codec Enabled | Avg Duration | Min Duration | Max Duration | Runtime ");
    info!("---------------------------------------------------");

    for ((denoise, codec_enabled), result) in summary {
        info!(
            " {}   | {}     | {:?} | {:?} | {:?} | {:?}",
            denoise, codec_enabled, result.average, result.min, result.max, result.end
        );
    }
}

/// returns a frame of random samples
pub(crate) fn dummy_frame() -> [f32; FRAME_SIZE] {
    let mut frame = [0_f32; FRAME_SIZE];
    let mut rng = rand::thread_rng();
    rng.fill(&mut frame[..]);

    for x in &mut frame {
        *x = x.clamp(i16::MIN as f32, i16::MAX as f32);
        *x /= i16::MAX as f32;
    }

    frame
}

pub(crate) fn dummy_int_frame() -> [i16; FRAME_SIZE] {
    let mut frame = [0_i16; FRAME_SIZE];
    let mut rng = rand::thread_rng();
    rng.fill(&mut frame[..]);
    frame
}

struct NetemGuard;

impl NetemGuard {
    fn apply(profile: &NetProfile, ports: &[u16]) -> std::io::Result<Self> {
        // Always start clean
        let _ = Command::new("tc")
            .args(["qdisc", "del", "dev", "lo", "root"])
            .status();

        if profile.netem_args.is_empty() {
            return Ok(Self);
        }

        // Root prio with 3 bands, netem attached to band 3
        cmd(&["qdisc", "add", "dev", "lo", "root", "handle", "1:", "prio"])?;
        cmd([
            "qdisc", "add", "dev", "lo", "parent", "1:3", "handle", "30:", "netem",
        ]
        .into_iter()
        .chain(profile.netem_args.iter().copied())
        .collect::<Vec<_>>()
        .as_slice())?;

        // Filters: UDP protocol (17) + dest port -> band 3
        for p in ports {
            cmd(&[
                "filter",
                "add",
                "dev",
                "lo",
                "protocol",
                "ip",
                "parent",
                "1:",
                "prio",
                "3",
                "u32",
                "match",
                "ip",
                "protocol",
                "17",
                "0xff",
                "match",
                "ip",
                "dport",
                &p.to_string(),
                "0xffff",
                "flowid",
                "1:3",
            ])?;
        }

        Ok(Self)
    }
}

impl Drop for NetemGuard {
    fn drop(&mut self) {
        let _ = Command::new("tc")
            .args(["qdisc", "del", "dev", "lo", "root"])
            .status();
    }
}

fn cmd(args: &[&str]) -> std::io::Result<()> {
    let st = Command::new("tc").args(args).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "tc failed"))
    }
}

#[derive(Clone)]
struct NetProfile {
    name: &'static str,
    // tc netem args, like: ["delay","120ms","30ms","loss","5%"]
    netem_args: &'static [&'static str],
}

// keep this curated and explicit; you can go full property-testing later.
static PROFILES: &[NetProfile] = &[
    NetProfile {
        name: "clean",
        netem_args: &[],
    },
    NetProfile {
        name: "wifi_jitter",
        netem_args: &["delay", "40ms", "20ms", "loss", "0.2%"],
    },
    NetProfile {
        name: "cellular",
        netem_args: &["delay", "120ms", "40ms", "loss", "1%"],
    },
    NetProfile {
        name: "satellite-ish",
        netem_args: &["delay", "600ms", "80ms", "loss", "0.5%"],
    },
    NetProfile {
        name: "reorder",
        netem_args: &["delay", "80ms", "20ms", "reorder", "15%", "50%"],
    },
    // burst loss: Gilbert-Elliott “gemodel” exists in netem
    NetProfile {
        name: "bursty_loss",
        netem_args: &["loss", "gemodel", "2%", "20%", "10%", "10%"],
    },
];
