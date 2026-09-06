//! The presence verbs over the laptop board: `begin` once, `read` in the
//! loop, and a reading that survives the wire. Its own binary: the facade's
//! board is a process-wide singleton.

use rusty_esp_arduino::prelude::*;
use rusty_esp_core::time::Micros;

fn reading(cm: u16, energy: u8, at: u64) -> radar::Presence {
    radar::Presence {
        state: radar::Occupancy::Moving,
        moving_cm: cm,
        moving_energy: energy,
        at: Micros(at),
        ..radar::Presence::default()
    }
}

#[test]
fn a_sensor_reading_reaches_the_sketch_and_encodes_for_the_wire() {
    // without a board, every verb says so
    assert!(!radar::begin(radar::Config::ld2410()));
    assert!(
        matches!(last_error(), Some(Error::NoBoard)),
        "{:?}",
        last_error()
    );
    assert!(radar::read().is_none());
    assert!(radar::config().is_none());

    board::install(HostBoard::new().unpaced());

    // a board with a sensor: nothing before begin
    assert!(radar::read().is_none(), "nothing before begin");
    host::inject_presence(reading(120, 55, 1));
    assert!(radar::read().is_none(), "…even with a reading waiting");

    let config = radar::Config::ld2410().with_engineering().at_baud(115_200);
    assert!(radar::begin(config), "{:?}", last_error());
    assert_eq!(radar::config(), Some(config));
    assert_eq!(config.baud, 115_200);
    assert!(config.engineering);
    assert_eq!(radar::Config::ld2410().baud, 256_000);
    assert!(!radar::Config::ld2410().engineering);

    // the reading queued before begin is still there, then they arrive in order
    let first = radar::read().expect("the reading arrives");
    assert_eq!(first.moving_cm, 120);
    assert!(first.state.occupied());
    assert!(radar::read().is_none(), "…and only once");

    host::inject_presence(reading(80, 90, 2));
    host::inject_presence(reading(200, 20, 3));
    assert_eq!(radar::read().map(|p| p.moving_cm), Some(80));
    assert_eq!(radar::read().map(|p| p.moving_cm), Some(200));
    assert!(radar::read().is_none());

    // what goes on the wire is the record, and it comes back the same
    let mut buf = [0u8; radar::ENCODED_LEN];
    let n = first.encode(&mut buf).expect("encodes");
    assert_eq!(n, radar::ENCODED_LEN);
    assert_eq!(radar::Presence::decode(&buf).unwrap(), first);

    // an absent reading is a reading, not a silence
    host::inject_presence(radar::Presence {
        state: radar::Occupancy::Absent,
        at: Micros(4),
        ..radar::Presence::default()
    });
    let gone = radar::read().expect("absence is reported");
    assert!(!gone.state.occupied());
    assert_eq!(gone.moving_cm, 0);

    radar::end();
    assert!(radar::config().is_none());
    board::uninstall();
}
