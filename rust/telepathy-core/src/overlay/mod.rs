#[cfg(windows)]
use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize};

#[cfg(windows)]
extern crate windows as other_windows;

use lazy_static::lazy_static;
#[cfg(windows)]
use widestring::error::ContainsNul;

/// flutter_rust_bridge:ignore
#[cfg(windows)]
mod color;
#[allow(clippy::module_inception)]
pub mod overlay;
/// flutter_rust_bridge:ignore
#[cfg(windows)]
mod windows;

lazy_static! {
    pub(crate) static ref LATENCY: Arc<AtomicUsize> = Default::default();
    pub(crate) static ref LOSS: Arc<AtomicUsize> = Default::default();
    pub(crate) static ref CONNECTED: Arc<AtomicBool> = Default::default();
    static ref FONT_HEIGHT: Arc<AtomicI32> = Default::default();
    static ref BACKGROUND_COLOR: Arc<AtomicU32> = Default::default();
    static ref FONT_COLOR: Arc<AtomicU32> = Default::default();
}

#[cfg(windows)]
type Result<T> = std::result::Result<T, Error>;

#[cfg(windows)]
#[derive(Debug)]
struct Error {
    _kind: ErrorKind,
}

#[cfg(windows)]
#[derive(Debug)]
enum ErrorKind {
    Windows(other_windows::core::Error),
    ContainsNul,
}

#[cfg(windows)]
impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self._kind {
            ErrorKind::Windows(error) => write!(f, "windows error: {:?}", error),
            ErrorKind::ContainsNul => write!(f, "string contains nul byte"),
        }
    }
}

#[cfg(windows)]
impl From<ContainsNul<u16>> for Error {
    fn from(_: ContainsNul<u16>) -> Self {
        Error {
            _kind: ErrorKind::ContainsNul,
        }
    }
}

#[cfg(windows)]
impl From<other_windows::core::Error> for Error {
    fn from(error: other_windows::core::Error) -> Self {
        Error {
            _kind: ErrorKind::Windows(error),
        }
    }
}

#[cfg(windows)]
#[cfg(test)]
mod tests {
    use tokio::time::sleep;

    use crate::overlay::color::Color;
    use crate::overlay::overlay::Overlay;
    use crate::overlay::{CONNECTED, LATENCY, LOSS};

    #[ignore]
    #[tokio::test]
    async fn test_overlay() {
        let bcolor = Color::new(0, 0, 0, 125);
        let fcolor = Color::new(255, 255, 255, 255);
        let overlay = Overlay::new(true, 100, 0, 600, 36, 36, bcolor.argb(), fcolor.argb()).await;

        sleep(std::time::Duration::from_secs(1)).await;

        overlay.show();

        CONNECTED.store(false, std::sync::atomic::Ordering::Relaxed);

        for x in 0..=500 {
            sleep(std::time::Duration::from_millis(100)).await;

            LOSS.store(x as usize, std::sync::atomic::Ordering::Relaxed);

            if x == 250 {
                CONNECTED.store(false, std::sync::atomic::Ordering::Relaxed);
            }

            // if x % 10 == 0 {
            //     overlay.move_overlay(600 + x, 2, 545 + x * 8, 36 + x);
            //     overlay.set_font_height(36 + x / 2);
            // }

            if x % 5 == 0 {
                LATENCY.store(x as usize / 5, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}
