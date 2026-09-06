//! The presence sensor, Arduino-shaped: `begin` once, `read` in the loop.
//!
//! The first sensor here that is neither a camera nor a microphone. A sketch
//! does not learn which sensor the board has: a millimetre-wave module on a
//! UART and Wi-Fi channel sensing both answer in the one record
//! ([`Presence`]), so the loop that sends presence to a home computer is the
//! same either way and a sensor added later needs no new verb.
//!
//! ```no_run
//! use rusty_esp_arduino::prelude::*;
//!
//! radar::begin(radar::Config::ld2410());
//! if let Some(p) = radar::read() {
//!     if p.state.occupied() {
//!         // …
//!     }
//! }
//! ```

use std::sync::{Mutex, PoisonError};

pub use rusty_esp_signal_core::radar::presence::{ENCODED_LEN, Occupancy, Presence};

use crate::board;
use crate::error;

/// What the presence sensor is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The module's UART baud rate. Ignored by a sensor that is not on a
    /// UART (Wi-Fi channel sensing has none).
    pub baud: u32,
    /// Ask for per-gate energies as well as the summary. Costs bytes on the
    /// wire and tells you which gate saw the target; off by default.
    pub engineering: bool,
}

impl Config {
    /// An HLK-LD2410 at its default baud, summary readings only.
    #[must_use]
    pub const fn ld2410() -> Self {
        Config {
            baud: 256_000,
            engineering: false,
        }
    }

    /// The same sensor at another baud (the module's `SetBaud` command
    /// changes it, and then the firmware must agree).
    #[must_use]
    pub const fn at_baud(mut self, baud: u32) -> Self {
        self.baud = baud;
        self
    }

    /// Ask for per-gate energies.
    #[must_use]
    pub const fn with_engineering(mut self) -> Self {
        self.engineering = true;
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::ld2410()
    }
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

/// Start the presence sensor with `config`. `false` (and
/// [`crate::last_error`]) when the board has none, or none is installed.
pub fn begin(config: Config) -> bool {
    let ok = error::ok(board::with(|b| b.radar_begin(&config)));
    if ok {
        *CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config);
    }
    ok
}

/// The newest reading since the last call, or `None` when none is ready.
/// Call it in the loop.
#[must_use]
pub fn read() -> Option<Presence> {
    board::with(|b| b.radar_read()).ok().flatten()
}

/// The configuration [`begin`] was given, once it succeeded.
#[must_use]
pub fn config() -> Option<Config> {
    *CONFIG.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Forget the configuration (tests; a sketch that ends).
pub fn end() {
    *CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
}
