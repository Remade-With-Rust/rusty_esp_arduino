//! Provisioning, Arduino-shaped: when the board has no network to join, it
//! advertises the Janus provisioning service over BLE and a phone (or the
//! laptop's browser, through `espino serve`'s page) writes the Wi-Fi
//! credentials; the sketch joins with them and stores them in the owner's
//! settings, so the next boot needs no phone.
//!
//! The board does the radio work behind [`crate::board::Board`]'s
//! `provision_*` seam — Bluedroid through `rusty_esp_signal-esp` on ESP-IDF,
//! a queue a test fills on the laptop — and this module keeps the sketch
//! shape: `begin` once, `poll` in the loop, `report` the join, `store` what
//! worked. The passphrase never appears in a `Debug` or a log.

use std::fmt;
use std::sync::{Mutex, PoisonError};

use crate::board;
use crate::error;

/// What a phone wrote: the network to join.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The network name.
    pub ssid: String,
    /// The passphrase; empty for an open network. Never printed.
    pub psk: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("ssid", &self.ssid)
            .field("psk", &format_args!("<{} bytes>", self.psk.len()))
            .finish()
    }
}

static ACTIVE: Mutex<Option<String>> = Mutex::new(None);

/// Start advertising as `name` and accepting credentials. `false` (and
/// [`crate::last_error`]) when the board has no BLE, or no board is
/// installed.
pub fn begin(name: &str) -> bool {
    let ok = error::ok(board::with(|b| b.provision_begin(name)));
    if ok {
        *ACTIVE.lock().unwrap_or_else(PoisonError::into_inner) = Some(name.to_owned());
    }
    ok
}

/// Whether `begin` succeeded and no join has been reported since.
#[must_use]
pub fn active() -> bool {
    ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// The credentials a phone wrote since the last call, if any. Call from the
/// loop while [`active`].
#[must_use]
pub fn poll() -> Option<Credentials> {
    board::with(|b| b.provision_poll())
        .ok()
        .flatten()
        .map(|(ssid, psk)| Credentials { ssid, psk })
}

/// Tell the phone how the join went: the status characteristic changes and
/// a subscribed page sees it. A successful join ends provisioning.
pub fn report(joined: bool) -> bool {
    let ok = error::ok(board::with(|b| b.provision_report(joined)));
    if ok && joined {
        *ACTIVE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
    ok
}

/// Keep `credentials` in the owner's settings (the `nvs` partition on a
/// chip), so the next boot joins without a phone. `false` when the board has
/// no settings store.
pub fn store(credentials: &Credentials) -> bool {
    error::ok(board::with(|b| {
        b.store_settings(&credentials.ssid, &credentials.psk)
    }))
}
