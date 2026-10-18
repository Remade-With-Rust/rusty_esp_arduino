//! A board whose interface is up with no address yet (a join that timed out
//! while the driver keeps trying) reports `0.0.0.0`: the facade calls that
//! no address and not connected, so a sketch never prints `http://0.0.0.0/`.
//! Seen on a XIAO, 2026-10-18. One test per binary: the facade's state is
//! global.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rusty_esp_arduino::board::{Board, Jpeg, Pcm};
use rusty_esp_arduino::error::Result;
use rusty_esp_arduino::{board, cam, mic, wifi};

/// The interface's IPv4 address as the board reads it; 0 until DHCP answers.
struct Joining(Arc<AtomicU32>);

impl Board for Joining {
    fn wifi_begin(&mut self, _ssid: &str, _psk: &str) -> Result<()> {
        Ok(())
    }
    fn local_ip(&self) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::from(self.0.load(Ordering::SeqCst))))
    }
    fn cam_begin(&mut self, _config: &cam::Config) -> Result<()> {
        Ok(())
    }
    fn cam_grab(&mut self) -> Result<Option<Jpeg>> {
        Ok(None)
    }
    fn mic_begin(&mut self, _config: &mic::Config) -> Result<()> {
        Ok(())
    }
    fn mic_read(&mut self) -> Result<Option<Pcm>> {
        Ok(None)
    }
    fn millis(&self) -> u64 {
        0
    }
}

#[test]
fn an_interface_without_an_address_is_not_connected() {
    let addr = Arc::new(AtomicU32::new(0));
    board::install(Joining(Arc::clone(&addr)));
    assert_eq!(wifi::local_ip(), None, "0.0.0.0 is no address");
    assert!(!wifi::connected());

    // the network answers: the address, and connected
    addr.store(u32::from(Ipv4Addr::new(192, 168, 0, 74)), Ordering::SeqCst);
    assert_eq!(
        wifi::local_ip(),
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 74)))
    );
    assert!(wifi::connected());
}
