//! The seam: one trait a board implements, installed once per program.
//!
//! A board owns the peripherals the sketch functions reach — the camera, the
//! microphone, the network interface — and nothing else. The stream side
//! (HTTP, RTP, UDP) is not the board's: it is `rusty_esp_video-esp`'s and is
//! the same code on every board, which is why `stream` is a module and not a
//! trait method.

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use rusty_esp_core::pcm::PcmFormat;
use rusty_esp_core::time::Micros;

use crate::error::{Error, Result};
use crate::{cam, mic};

/// One JPEG frame from the camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jpeg {
    /// The whole JFIF file, SOI to EOI.
    pub bytes: Vec<u8>,
    /// Pixels across.
    pub width: u16,
    /// Pixels down.
    pub height: u16,
    /// When it was captured, in the board's clock.
    pub timestamp: Micros,
}

/// One block of PCM from the microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    /// Rate, channels and sample format of `bytes`.
    pub format: PcmFormat,
    /// Interleaved little-endian samples.
    pub bytes: Vec<u8>,
    /// When the first sample was captured, in the board's clock.
    pub timestamp: Micros,
}

impl Pcm {
    /// Sample frames in the block.
    #[must_use]
    pub fn frames(&self) -> usize {
        let frame = self.format.sample.bytes() * usize::from(self.format.channels);
        self.bytes.len().checked_div(frame).unwrap_or(0)
    }
}

/// What a board provides. Every method may be called before the matching
/// `begin`; a board answers `Err(Error::NotBegun(..))` then.
pub trait Board: Send {
    /// Join the network `ssid` with `psk`; return when joined or refused.
    fn wifi_begin(&mut self, ssid: &str, psk: &str) -> Result<()>;
    /// The interface address once joined.
    fn local_ip(&self) -> Option<IpAddr>;
    /// Configure and start the camera.
    fn cam_begin(&mut self, config: &cam::Config) -> Result<()>;
    /// The next frame, or `None` when no frame is ready yet.
    fn cam_grab(&mut self) -> Result<Option<Jpeg>>;
    /// Configure and start the microphone.
    fn mic_begin(&mut self, config: &mic::Config) -> Result<()>;
    /// The next block, or `None` when no block is ready yet.
    fn mic_read(&mut self) -> Result<Option<Pcm>>;
    /// Milliseconds since the board started.
    fn millis(&self) -> u64;
}

static BOARD: Mutex<Option<Box<dyn Board>>> = Mutex::new(None);

/// Install the board the sketch runs on. Call once, first thing in `setup`.
/// Installing again replaces the previous board.
pub fn install(board: impl Board + 'static) {
    *BOARD.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(board));
}

/// Remove the installed board (tests, and a sketch that ends).
pub fn uninstall() {
    *BOARD.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether a board is installed.
#[must_use]
pub fn installed() -> bool {
    BOARD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// Run `f` against the installed board.
pub(crate) fn with<R>(f: impl FnOnce(&mut dyn Board) -> Result<R>) -> Result<R> {
    let mut guard = BOARD.lock().unwrap_or_else(PoisonError::into_inner);
    match guard.as_mut() {
        Some(board) => f(board.as_mut()),
        None => Err(Error::NoBoard),
    }
}

/// Milliseconds since the board started (0 without a board).
#[must_use]
pub fn millis() -> u64 {
    with(|b| Ok(b.millis())).unwrap_or(0)
}
