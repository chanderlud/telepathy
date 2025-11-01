// The following is a modified version of the code found at
// https://github.com/fzyzcjy/flutter_rust_bridge/issues/486

use fast_log::Config;
use flutter_rust_bridge::frb;
use lazy_static::lazy_static;
use log::{LevelFilter, info, warn};
use parking_lot::RwLock;
use std::sync::Once;

use crate::frb_generated::StreamSink;

static INIT_LOGGER_ONCE: Once = Once::new();

lazy_static! {
    static ref SEND_TO_DART_LOGGER_STREAM_SINK: RwLock<Option<StreamSink<String>>> =
        RwLock::new(None);
}

pub fn init_logger() {
    // https://stackoverflow.com/questions/30177845/how-to-initialize-the-logger-for-integration-tests
    INIT_LOGGER_ONCE.call_once(|| {
        // let level = if cfg!(debug_assertions) {
        //     LevelFilter::Debug
        // } else {
        //     LevelFilter::Warn
        // };

        let level = LevelFilter::Debug;

        assert!(
            level <= log::STATIC_MAX_LEVEL,
            "Should respect log::STATIC_MAX_LEVEL={:?}, which is done in compile time. level{:?}",
            log::STATIC_MAX_LEVEL,
            level
        );

        // TODO reintegrate logging with dart

        #[cfg(not(target_family = "wasm"))]
        fast_log::init(Config::new().file("telepathy.log").chan_len(Some(1000))).unwrap();

        #[cfg(target_family = "wasm")]
        wasm_logger::init(wasm_logger::Config::default());

        log_panics::init();

        info!("init_logger (inside 'once') finished");

        warn!(
            "init_logger finished, chosen level={:?} (deliberately output by warn level)",
            level
        );
    });
}

pub struct SendToDartLogger {}

impl SendToDartLogger {
    pub fn set_stream_sink(stream_sink: StreamSink<String>) {
        let mut guard = SEND_TO_DART_LOGGER_STREAM_SINK.write();
        let overriding = guard.is_some();

        *guard = Some(stream_sink);

        drop(guard);

        if overriding {
            warn!(
                "SendToDartLogger::set_stream_sink but already exist a sink, thus overriding. \
                (This may or may not be a problem. It will happen normally if hot-reload Flutter app.)"
            );
        }
    }
}

#[frb(sync)]
pub fn create_log_stream(s: StreamSink<String>) {
    SendToDartLogger::set_stream_sink(s);
}

#[frb(sync)]
pub fn rust_set_up() {
    init_logger();
}
