use crate::frb_generated::StreamSink;
use flutter_rust_bridge::frb;
use std::io;
use std::io::Write;
use std::sync::{Once, OnceLock};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

static INIT_LOGGER_ONCE: Once = Once::new();
#[cfg(not(target_family = "wasm"))]
static TRACING_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
static SEND_TO_DART_LOG_STREAM: OnceLock<StreamSink<String>> = OnceLock::new();

#[derive(Clone, Copy)]
struct DartWriter;

struct DartLogWrite {
    buffer: Vec<u8>,
}

impl DartLogWrite {
    fn flush_lines(&mut self, force_tail: bool) {
        while let Some(idx) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=idx).collect::<Vec<u8>>();
            let text = String::from_utf8_lossy(&line);
            self.send_line(text.trim_end_matches('\n'));
        }

        if force_tail && !self.buffer.is_empty() {
            let tail = std::mem::take(&mut self.buffer);
            let text = String::from_utf8_lossy(&tail);
            self.send_line(text.trim_end_matches('\n'));
        }
    }

    fn send_line(&self, line: &str) {
        if line.is_empty() {
            return;
        }
        if let Some(stream) = SEND_TO_DART_LOG_STREAM.get() {
            _ = stream.add(line.to_string());
        }
    }
}

impl<'a> MakeWriter<'a> for DartWriter {
    type Writer = DartLogWrite;

    fn make_writer(&'a self) -> Self::Writer {
        DartLogWrite { buffer: Vec::new() }
    }
}

impl Write for DartLogWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        self.flush_lines(false);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_lines(true);
        Ok(())
    }
}

#[frb(sync)]
pub fn create_log_stream(s: StreamSink<String>) {
    SEND_TO_DART_LOG_STREAM.get_or_init(|| s);
}

#[frb(sync)]
pub fn rust_set_up() {
    // https://stackoverflow.com/questions/30177845/how-to-initialize-the-logger-for-integration-tests
    INIT_LOGGER_ONCE.call_once(|| {
        let default_level = if cfg!(debug_assertions) {
            "telepathy_core=debug,telepathy_audio=info,tracing_panic=error"
        } else {
            "telepathy_core=warn,telepathy_audio=warn,tracing_panic=error"
        };
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                let env_filter = EnvFilter::new(default_level);
                let wasm_layer = tracing_wasm::WASMLayer::default();
                // TODO dart_layer is not working correctly on WASM
                let subscriber = tracing_subscriber::registry()
                    .with(env_filter)
                    .with(wasm_layer);
                if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
                    warn!(
                        "tracing subscriber already set, keeping existing subscriber (expected in hot reload / integration tests): {}",
                        error
                    );
                }
            } else {
                let dart_layer = tracing_subscriber::fmt::layer()
                    .compact()
                    .with_ansi(false)
                    .with_writer(DartWriter);

                let env_filter =
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
                let rolling = tracing_appender::rolling::daily(".", "telepathy-trace.log");
                let (non_blocking_writer, guard) = tracing_appender::non_blocking(rolling);
                let _ = TRACING_GUARD.set(guard);
                let json_layer = tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(true)
                    .with_span_list(true)
                    .with_writer(non_blocking_writer);
                let subscriber = tracing_subscriber::registry()
                    .with(env_filter)
                    .with(dart_layer)
                    .with(json_layer);
                if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
                    warn!(
                        "tracing subscriber already set, keeping existing subscriber (expected in hot reload / integration tests): {}",
                        error
                    );
                }
                std::panic::set_hook(Box::new(tracing_panic::panic_hook));
            }
        }

        info!(event = "logger_initialized");
    });
}
