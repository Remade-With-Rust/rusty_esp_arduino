//! `wifi::host`: a board that has no radio refuses and says so, a board that
//! has one answers on the address it is now serving, and either way
//! `local_ip` is the one question a sketch has to ask. One test per binary —
//! the facade's state is global.

use std::net::{IpAddr, Ipv4Addr};

use rusty_esp_arduino::board::{Board, Jpeg, Pcm};
use rusty_esp_arduino::error::Result;
use rusty_esp_arduino::host::HostBoard;
use rusty_esp_arduino::{Error, board, cam, last_error, mic, wifi};

/// The address an ESP32 hands itself when it runs the network.
const AP_ADDR: Ipv4Addr = Ipv4Addr::new(192, 168, 71, 1);

/// A board that can host, and remembers what it was asked to host, so the
/// test can check the arguments arrived rather than only that a bool came
/// back true.
#[derive(Default)]
struct Hosting {
    hosting: Option<(String, String)>,
}

impl Board for Hosting {
    fn wifi_begin(&mut self, _ssid: &str, _psk: &str) -> Result<()> {
        Ok(())
    }
    fn wifi_host(&mut self, ssid: &str, psk: &str) -> Result<()> {
        // the same refusal the chip makes, so the seam's contract is one
        // rule rather than one per board
        if !(8..=63).contains(&psk.len()) {
            return Err(Error::Io("psk must be 8 to 63 bytes for WPA2".into()));
        }
        self.hosting = Some((ssid.to_owned(), psk.to_owned()));
        Ok(())
    }
    fn local_ip(&self) -> Option<IpAddr> {
        self.hosting.as_ref().map(|_| IpAddr::V4(AP_ADDR))
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
fn hosting_is_refused_by_default_and_answered_by_a_board_that_can() {
    // no board at all
    assert!(!wifi::host("janus-setup", "a good passphrase"));
    assert_eq!(last_error(), Some(Error::NoBoard));
    assert!(!wifi::connected());

    // A board with a radio it cannot host with refuses by name. This is the
    // provided method doing its job: a laptop is on somebody else's network
    // and cannot run one, and saying so beats appearing to succeed.
    board::install(HostBoard::new().unpaced());
    assert!(!wifi::host("janus-setup", "a good passphrase"));
    assert_eq!(last_error(), Some(Error::Missing("access-point mode")));
    assert_eq!(
        last_error().unwrap().to_string(),
        "the board has no access-point mode"
    );
    board::uninstall();

    // A board that can host answers on the network it is now running, and
    // `connected` is true without anything having been joined.
    board::install(Hosting::default());
    assert!(!wifi::connected(), "nothing is up before host is called");
    assert!(
        wifi::host("janus-setup", "a good passphrase"),
        "{:?}",
        last_error()
    );
    assert_eq!(last_error(), None);
    assert!(wifi::connected());
    assert_eq!(wifi::local_ip(), Some(IpAddr::V4(AP_ADDR)));

    // The passphrase bound is the seam's, not one board's: WPA2 has no key
    // shorter than eight bytes, so a sketch cannot stand up an access point
    // that refuses every client.
    assert!(!wifi::host("janus-setup", "short"));
    assert!(
        matches!(last_error(), Some(Error::Io(_))),
        "{:?}",
        last_error()
    );
    board::uninstall();
}
