//! Every board record in `boards/` parses, follows the schema, keeps its pins
//! inside the chip's GPIO range, and claims no GPIO twice.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    board: Board,
    camera: Option<Camera>,
    microphone: Option<Microphone>,
    led: Option<Led>,
    sources: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Board {
    name: String,
    vendor: String,
    chip: String,
    psram_mb: u32,
    flash_mb: u32,
    track: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Camera {
    sensor: String,
    interface: String,
    xclk_hz: u32,
    pwdn: i32,
    reset: i32,
    xclk: i32,
    sccb_sda: i32,
    sccb_scl: i32,
    d0: i32,
    d1: i32,
    d2: i32,
    d3: i32,
    d4: i32,
    d5: i32,
    d6: i32,
    d7: i32,
    vsync: i32,
    href: i32,
    pclk: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Microphone {
    kind: String,
    clk: i32,
    data: i32,
    ws: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Led {
    gpio: i32,
    active: String,
}

const RECORDS: &[(&str, &str)] = &[
    (
        "xiao-esp32s3-sense",
        include_str!("../../../boards/xiao-esp32s3-sense.toml"),
    ),
    (
        "esp32-s3-eye",
        include_str!("../../../boards/esp32-s3-eye.toml"),
    ),
    (
        "ai-thinker-esp32-cam",
        include_str!("../../../boards/ai-thinker-esp32-cam.toml"),
    ),
];

/// The highest GPIO number a chip has.
fn max_gpio(chip: &str) -> i32 {
    match chip {
        "esp32" => 39,
        "esp32s3" => 48,
        "esp32c6" => 30,
        "esp32p4" => 54,
        other => panic!("unknown chip {other}"),
    }
}

#[test]
fn every_record_parses_and_holds_the_schema() {
    for (name, text) in RECORDS {
        let r: Record = toml::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.schema, 1, "{name}");
        assert_eq!(r.board.name, *name, "the file name is the board name");
        assert!(!r.board.vendor.is_empty(), "{name}");
        assert!(matches!(r.board.track.as_str(), "idf" | "hal"), "{name}");
        assert!(
            r.board.flash_mb >= 4,
            "{name}: {} MB flash",
            r.board.flash_mb
        );
        let top = max_gpio(&r.board.chip);
        let mut claimed: Vec<(i32, &str)> = Vec::new();
        let mut claim = |pin: i32, what: &'static str, optional: bool| {
            if pin < 0 {
                assert!(optional, "{name}: {what} must be wired");
                return;
            }
            assert!(pin <= top, "{name}: {what} = GPIO{pin} beyond GPIO{top}");
            if let Some((_, other)) = claimed.iter().find(|(p, _)| *p == pin) {
                panic!("{name}: GPIO{pin} claimed by both {other} and {what}");
            }
            claimed.push((pin, what));
        };
        if let Some(c) = &r.camera {
            assert_eq!(c.interface, "dvp", "{name}");
            assert!(!c.sensor.is_empty(), "{name}");
            assert!((8_000_000..=40_000_000).contains(&c.xclk_hz), "{name}");
            claim(c.pwdn, "camera pwdn", true);
            claim(c.reset, "camera reset", true);
            claim(c.xclk, "camera xclk", false);
            claim(c.sccb_sda, "camera sccb_sda", false);
            claim(c.sccb_scl, "camera sccb_scl", false);
            for (i, d) in [c.d0, c.d1, c.d2, c.d3, c.d4, c.d5, c.d6, c.d7]
                .into_iter()
                .enumerate()
            {
                let what: &'static str = ["d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7"][i];
                claim(d, what, false);
            }
            claim(c.vsync, "camera vsync", false);
            claim(c.href, "camera href", false);
            claim(c.pclk, "camera pclk", false);
            assert!(
                r.sources.contains_key("camera"),
                "{name}: cite the camera pins"
            );
            if r.board.psram_mb == 0 {
                panic!("{name}: a camera board without PSRAM cannot hold a frame");
            }
        }
        if let Some(m) = &r.microphone {
            match m.kind.as_str() {
                "pdm" => assert_eq!(m.ws, -1, "{name}: PDM has no word select"),
                "i2s" => claim(m.ws, "microphone ws", false),
                other => panic!("{name}: microphone kind {other}"),
            }
            claim(m.clk, "microphone clk", false);
            claim(m.data, "microphone data", false);
            assert!(
                r.sources.contains_key("microphone"),
                "{name}: cite the microphone pins"
            );
        }
        if let Some(l) = &r.led {
            claim(l.gpio, "led", false);
            assert!(matches!(l.active.as_str(), "low" | "high"), "{name}");
            assert!(r.sources.contains_key("led"), "{name}: cite the LED");
        }
        assert!(
            r.camera.is_some() || r.microphone.is_some(),
            "{name}: a board file without a camera or a microphone has nothing for a sketch"
        );
    }
}

#[test]
fn the_first_board_is_the_one_the_janus_firmwares_already_build_for() {
    let r: Record = toml::from_str(RECORDS[0].1).unwrap();
    let c = r.camera.expect("camera");
    let m = r.microphone.expect("microphone");
    assert_eq!((c.xclk, c.sccb_sda, c.sccb_scl, c.pclk), (10, 40, 39, 13));
    assert_eq!((m.kind.as_str(), m.clk, m.data), ("pdm", 42, 41));
    assert_eq!(r.board.chip, "esp32s3");
}
