//! The network, Arduino-shaped: `begin` or `host` once, then `local_ip`.
//!
//! Two ways onto a network and one way to ask where you are.
//! [`begin`] joins somebody else's; [`host`] runs one of the device's own,
//! which is what a board with no access point in reach has to do. Either
//! way [`local_ip`] is the address to hand out and [`connected`] is whether
//! there is one.

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

/// Host an access point named `ssid` with `psk` instead of joining one, for
/// a device with no network in reach. `false` (and [`crate::last_error`])
/// when the board has no access-point mode or refuses the arguments.
///
/// `psk` must be 8 to 63 bytes. Clients then reach the device at
/// [`local_ip`], which is the address of the network it is now running.
pub fn host(ssid: &str, psk: &str) -> bool {
    error::ok(board::with(|b| b.wifi_host(ssid, psk)))
}

/// The interface address, once joined or once hosting.
#[must_use]
pub fn local_ip() -> Option<IpAddr> {
    board::with(|b| Ok(b.local_ip())).unwrap_or(None)
}

/// Whether [`begin`] or [`host`] has succeeded.
#[must_use]
pub fn connected() -> bool {
    local_ip().is_some()
}
