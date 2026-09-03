//! A sketch without a board fails loudly and says why. Its own binary, and
//! one test: the facade's state is per process.

use rusty_esp_arduino::prelude::*;
use rusty_esp_core::time::Micros;

#[test]
fn every_call_is_refused_with_no_board_and_the_reason_is_one_call_away() {
    assert!(!board::installed());
    assert!(!wifi::begin("x", "y"));
    assert_eq!(last_error(), Some(Error::NoBoard));
    assert!(!cam::begin(cam::Config::qvga_jpeg()));
    assert_eq!(last_error(), Some(Error::NoBoard));
    assert!(cam::grab().is_none());
    assert_eq!(last_error(), Some(Error::NoBoard));
    assert!(!mic::begin(mic::Config::pcm16_16k()));
    assert!(mic::read().is_none());
    assert_eq!(last_error(), Some(Error::NoBoard));
    assert_eq!(wifi::local_ip(), None);
    assert!(!wifi::connected());
    assert_eq!(millis(), 0);
    assert_eq!(cam::config(), None);
    assert_eq!(mic::config(), None);
    assert_eq!(
        last_error().unwrap().to_string(),
        "no board installed: call board::install first"
    );

    // with a board that has not begun, the peripheral is named
    board::install(HostBoard::new().unpaced());
    assert!(board::installed());
    assert!(cam::grab().is_none());
    assert_eq!(last_error(), Some(Error::NotBegun("camera")));
    assert!(mic::read().is_none());
    assert_eq!(last_error(), Some(Error::NotBegun("microphone")));
    let frame = Jpeg {
        bytes: vec![0xFF, 0xD8, 0xFF, 0xD9],
        width: 1,
        height: 1,
        timestamp: Micros(0),
    };
    assert!(!stream::push_jpeg(&frame));
    assert_eq!(last_error(), Some(Error::NotBegun("stream")));
    assert_eq!(
        last_error().unwrap().to_string(),
        "stream: begin was not called"
    );
    let pcm = Pcm {
        format: mic::Config::pcm16_16k().format,
        bytes: vec![0; 64],
        timestamp: Micros(0),
    };
    assert!(!stream::push_pcm(&pcm));
    assert_eq!(last_error(), Some(Error::NotBegun("stream::pcm_to")));

    // a successful call clears the record
    assert!(cam::begin(cam::Config::qvga_jpeg()));
    assert_eq!(last_error(), None);
    assert_eq!(cam::config(), Some(cam::Config::qvga_jpeg()));
    assert!(cam::grab().is_some());
    board::uninstall();
    assert!(!board::installed());
}
