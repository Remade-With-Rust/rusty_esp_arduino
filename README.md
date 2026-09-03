# rusty_esp_arduino

[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The sketch a maker types. `setup` and `loop`, `wifi::begin`, `cam::grab`,
`mic::read`, `stream::push` — over the Janus function packages, in Rust,
with the same sketch running on your laptop today and on the board when it
arrives.

```rust
use rusty_esp_arduino::prelude::*;

fn setup() {
    board::install(HostBoard::from_env());   // the laptop; a chip installs its own
    wifi::begin("home", "psk");
    cam::begin(cam::Config::qvga_jpeg());
    mic::begin(mic::Config::pcm16_16k());
    stream::listen(8080);                    // http://<ip>:8080/  in any browser
}

fn loop_once() {
    if let Some(frame) = cam::grab() {
        stream::push_jpeg(&frame);
    }
    if let Some(pcm) = mic::read() {
        stream::push_pcm(&pcm);
    }
}

fn main() {
    sketch::run(setup, loop_once);
}
```

```sh
cargo run -p rusty_esp_arduino --example cam_mic
# then open http://127.0.0.1:8080/ — or:  ffmpeg -i http://127.0.0.1:8080/stream -frames:v 30 -f null -
```

Part of **Janus**, the Remade-With-Rust programme that rebuilds the Espressif
ESP32 and Arduino application portfolio in memory-safe Rust so hardware makers
can ship products that plug straight into the MATA home computer.

- This package's plan: [docs/plans/rusty_esp_arduino.md](docs/plans/rusty_esp_arduino.md)
- Every number: [docs/LEDGER.md](docs/LEDGER.md)
- The family plan and the API this implements: Janus `docs/plans/rusty-ESP-arduino.md` §8 (umbrella repo)

## The maker path

1. **Write the sketch on your laptop.** `HostBoard` is a camera (colour bars,
   or a folder of JPEGs: `JANUS_CAM_DIR=./frames`) and a microphone (a tone,
   or a WAV file: `JANUS_WAV=./voice.wav`). `stream::listen` serves the same
   MJPEG page the board will; `stream::rtp_to` and `stream::pcm_to` send the
   same datagrams. Every function returns `bool` or `Option`; when one says
   no, `last_error()` says why.
2. **Watch it in a browser or in ffmpeg.** No SDK, no flashing, no serial
   monitor.
3. **Install the board's `Board` instead of `HostBoard`.** Nothing else in
   the sketch changes. That is the board half of this package's plan and it
   waits for a board on the bench.

## Thin by law

Every function here is one call through the `Board` seam into a function
package. The camera and microphone belong to the board; the MJPEG server and
the RTP/JPEG and PCM senders are `rusty_esp_video-esp`'s and run unchanged
on the laptop and on ESP-IDF; the WAV reader is `rusty_esp_audio-core`'s. The
facade owns no codec, no packet format and no pixel loop. It will not grow
an Arduino core: no `String`, no `Wire`, no timers — only the surface the
product needs.

## Status

**M4's host half shipped 2026-09-02.** The prelude, the `Board` seam, the
laptop board, the three stream outputs, and the tests that hold them: the
plan's sketch runs on the laptop and every frame and block it pushes comes
back through `rusty_esp_video-esp`'s own receivers, counted; the page and
the stream are read back over TCP; ffmpeg decodes the stream. Counts are in
the ledger. What waits for a board: the chip's `Board` (camera DMA, I2S/PDM,
Wi-Fi join) and the second board file.

## Layout

```text
crates/rusty_esp_arduino   std, forbid(unsafe): the facade
  src/board.rs             the Board seam, Jpeg and Pcm, install()
  src/cam.rs  mic.rs  wifi.rs   the sketch functions, one call each
  src/stream.rs            listen / rtp_to / pcm_to / push_jpeg / push_pcm over rusty_esp_video-esp
  src/host.rs              HostBoard: pattern or JPEG-directory camera, tone or WAV microphone
  src/sketch.rs            run / run_for / delay / millis
  src/error.rs             last_error()
  examples/cam_mic.rs      the sketch above
  tests/                   no_board, loopback (RTP + PCM back through the video package's receivers), http
firmware/                  the board sketches, when there is a board (own cargo projects)
docs/plans/                the plan; docs/LEDGER.md every number
```

## Build

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

No `no_std` rung: the facade is the std (Track A) surface by design, the same
code on the laptop and on ESP-IDF. The function packages it wraps carry the
bare-metal gates.

## License

MIT OR Apache-2.0, at your option.
