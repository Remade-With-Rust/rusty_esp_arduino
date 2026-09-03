//! The microphone, Arduino-shaped: `begin` once, `read` in the loop.

use std::sync::{Mutex, PoisonError};

use rusty_esp_core::pcm::{PcmFormat, SampleFormat};

use crate::board::{self, Pcm};
use crate::error;

/// What the microphone is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Rate, channels and sample format the board should deliver. A board
    /// that cannot (a WAV file has its own) reports what it does deliver in
    /// each [`Pcm`].
    pub format: PcmFormat,
    /// Milliseconds of audio per `read`.
    pub frame_ms: u32,
}

const fn format(rate: u32, channels: u8, sample: SampleFormat) -> PcmFormat {
    match PcmFormat::new(rate, channels, sample) {
        Ok(f) => f,
        Err(_) => panic!("a non-zero rate and channel count"),
    }
}

impl Config {
    /// 16 kHz mono 16-bit, 20 ms blocks: speech, and what every Janus board
    /// can do.
    #[must_use]
    pub const fn pcm16_16k() -> Self {
        Config {
            format: format(16_000, 1, SampleFormat::I16),
            frame_ms: 20,
        }
    }

    /// 48 kHz mono 16-bit, 20 ms blocks.
    #[must_use]
    pub const fn pcm16_48k() -> Self {
        Config {
            format: format(48_000, 1, SampleFormat::I16),
            frame_ms: 20,
        }
    }

    /// The same format in blocks of another length.
    #[must_use]
    pub const fn blocks_of_ms(mut self, frame_ms: u32) -> Self {
        self.frame_ms = frame_ms;
        self
    }

    /// Bytes in one block of this configuration.
    #[must_use]
    pub const fn block_bytes(&self) -> usize {
        self.format.bytes_for_micros(self.frame_ms as u64 * 1000)
    }
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

/// Start the microphone with `config`. `false` (and [`crate::last_error`])
/// when the board refuses or none is installed.
pub fn begin(config: Config) -> bool {
    let r = board::with(|b| b.mic_begin(&config));
    if r.is_ok() {
        *CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config);
    }
    error::ok(r)
}

/// The next block, if one is ready. `None` with no error recorded means "not
/// yet"; `None` with [`crate::last_error`] set means something is wrong.
pub fn read() -> Option<Pcm> {
    error::some(board::with(|b| b.mic_read()))
}

/// The configuration `begin` was given, once it succeeded.
#[must_use]
pub fn config() -> Option<Config> {
    *CONFIG.lock().unwrap_or_else(PoisonError::into_inner)
}
