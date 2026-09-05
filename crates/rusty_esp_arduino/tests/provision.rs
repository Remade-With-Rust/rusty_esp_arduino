//! The provisioning verbs over the laptop board: `begin` advertises,
//! `poll` returns what a phone wrote, `report` ends it, `store` keeps it,
//! and a `Debug` of the credentials never carries the passphrase.

use rusty_esp_arduino::prelude::*;

#[test]
fn a_phone_writes_credentials_and_the_sketch_joins_and_keeps_them() {
    board::install(HostBoard::new());
    assert!(!provision::active());
    assert!(provision::poll().is_none(), "nothing before begin");

    assert!(provision::begin("janus/test-cam"), "{:?}", last_error());
    assert!(provision::active());
    assert_eq!(host::provisioning_name(), Some("janus/test-cam".to_owned()));
    assert!(provision::poll().is_none(), "no phone yet");

    // the phone writes the credential TLV; on the laptop a test does
    host::inject_credentials("Pixel_3333", "correct horse battery");
    let creds = provision::poll().expect("credentials arrive once");
    assert_eq!(creds.ssid, "Pixel_3333");
    assert_eq!(creds.psk, "correct horse battery");
    assert!(provision::poll().is_none(), "…and only once");
    let shown = format!("{creds:?}");
    assert!(shown.contains("Pixel_3333"));
    assert!(
        !shown.contains("correct horse"),
        "the passphrase never prints: {shown}"
    );
    assert!(shown.contains("21 bytes"));

    assert!(wifi::begin(&creds.ssid, &creds.psk));
    assert!(provision::report(true));
    assert!(!provision::active(), "a join ends provisioning");
    assert!(provision::store(&creds));
    assert_eq!(
        host::stored_settings(),
        Some(("Pixel_3333".to_owned(), "correct horse battery".to_owned()))
    );
    board::uninstall();

    // and without a board every verb says so (in the same test: the board is a
    // process-wide singleton, so two tests cannot share it)
    assert!(!provision::begin("x"));
    assert!(provision::poll().is_none());
    assert!(!provision::report(true));
    assert!(!provision::store(&provision::Credentials {
        ssid: "a".into(),
        psk: String::new()
    }));
    assert!(
        matches!(last_error(), Some(Error::NoBoard)),
        "{:?}",
        last_error()
    );
}
