//! The network, Arduino-shaped: `begin` once, then `local_ip`.

use std::net::IpAddr;

use crate::board;
use crate::error;

/// Join `ssid` with `psk`; returns once joined. `false` (and
/// [`crate::last_error`]) when refused or no board is installed. On the
/// laptop board this records the name and succeeds: the laptop is already
/// on its network.
pub fn begin(ssid: &str, psk: &str) -> bool {
    error::ok(board::with(|b| b.wifi_begin(ssid, psk)))
}

/// The interface address once joined.
#[must_use]
pub fn local_ip() -> Option<IpAddr> {
    board::with(|b| Ok(b.local_ip())).unwrap_or(None)
}

/// Whether `begin` has succeeded.
#[must_use]
pub fn connected() -> bool {
    local_ip().is_some()
}
