//! `identity::begin`: the device DID is minted once and comes back the same
//! from the same store; the refusals name what is missing. One test per
//! binary — the facade's state is global.

use std::net::IpAddr;

use rusty_esp_arduino::board::{Board, Jpeg, Pcm};
use rusty_esp_arduino::error::Result;
use rusty_esp_arduino::host::HostBoard;
use rusty_esp_arduino::{Error, board, cam, identity, last_error, mic};
use rusty_esp_core::hal::host::InsecureTestRng;
use rusty_esp_mid_core::key::DeviceKey;

/// A board with no store and no entropy: what a bare `Board` gives.
struct Bare;

impl Board for Bare {
    fn wifi_begin(&mut self, _ssid: &str, _psk: &str) -> Result<()> {
        Ok(())
    }
    fn local_ip(&self) -> Option<IpAddr> {
        None
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
fn the_did_is_minted_once_and_the_refusals_say_why() {
    let dir = std::env::temp_dir().join(format!("janus-identity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // no board
    assert!(!identity::begin(None));
    assert_eq!(last_error(), Some(Error::NoBoard));
    assert!(identity::did().is_none());

    // a board without a store
    board::install(Bare);
    assert!(!identity::begin(None));
    assert!(
        matches!(last_error(), Some(Error::Missing(_))),
        "{:?}",
        last_error()
    );
    board::uninstall();

    // the laptop board: minted
    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(identity::begin(None), "{:?}", last_error());
    let first = identity::did().expect("a did");
    assert!(first.starts_with("did:mata:"), "{first}");
    assert!(identity::maker().is_none());
    assert!(identity::begun());

    // the same store, a new process worth of state: the same DID
    identity::end();
    board::uninstall();
    assert!(!identity::begun());
    board::install(HostBoard::new().unpaced().with_store(&dir));
    let maker = DeviceKey::generate(&mut InsecureTestRng::seeded(7), "maker")
        .unwrap()
        .did()
        .to_did_string();
    assert!(identity::begin(Some(&maker)), "{:?}", last_error());
    assert_eq!(
        identity::did().as_deref(),
        Some(first.as_str()),
        "stable across begins"
    );
    assert_eq!(identity::maker().as_deref(), Some(maker.as_str()));
    assert!(
        dir.join("mid.devkey").is_file(),
        "the key sits in the store under mid's name"
    );

    // a maker that is not a did:mata
    identity::end();
    board::uninstall();
    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(!identity::begin(Some("did:web:example.com")));
    assert!(
        matches!(last_error(), Some(Error::Io(m)) if m.contains("did:mata")),
        "{:?}",
        last_error()
    );

    // a different store: a different device
    let other = dir.join("other");
    identity::end();
    board::uninstall();
    board::install(HostBoard::new().unpaced().with_store(&other));
    assert!(identity::begin(None));
    assert_ne!(identity::did().as_deref(), Some(first.as_str()));

    identity::end();
    board::uninstall();
    let _ = std::fs::remove_dir_all(&dir);
}
