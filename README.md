### In The Wild with 9 Active Installs

FREE RAG Converter Online -- <a href="https://RAGconverter.com">RAGconverter.com</a>

# rusty_esp_arduino

[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/remade-with-rust) [![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network) [![crates.io](https://img.shields.io/crates/v/rusty_esp_arduino.svg)](https://crates.io/crates/rusty_esp_arduino) [![docs.rs](https://docs.rs/rusty_esp_arduino/badge.svg)](https://docs.rs/rusty_esp_arduino) [![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/rusty_esp_arduino/blob/main/LICENSE-MIT)

The Arduino shape, in memory-safe Rust: `setup`, a loop, and verbs that read
like the sketches people already write — `wifi::begin`, `cam::grab`,
`mic::read`, `stream::push`, `mesh::begin`. Underneath are the **Janus**
function packages; above is a sketch with no GPIO number, no register, no
codec parameter and no key in it. Pure Rust, no C, no FFI.

* **Thin by law.** This crate owns a thread, two frame slots and a subscriber
  factory. Everything else belongs to the package that owns it, and the tests
  enforce the direction.
* **The board is a seam, not a target.** A `Board` trait supplies the camera,
  the microphone, the network, the store and the entropy — so the same sketch
  runs on a chip and on a laptop, where the camera is a test pattern and the
  microphone a tone. A sketch is testable without hardware.
* **Hosting counts as connecting.** A device with no access point in reach can
  host the network it was provisioned with, which is how a camera in a shed
  stays reachable. One call changes which.
* **The mesh in three verbs.** `mesh::begin` brings up a peer-to-peer node,
  `push_media` hands it a frame, `service` returns what happened — including
  what the node actually sent, which is not the same number as what the sketch
  pushed.

## What has run on hardware

The generated sketch built on this facade is what five of seven verified device
profiles run. Measured on a Seeed XIAO ESP32-S3 Sense over a network the board
hosts itself:

| what | measured |
|---|---|
| identity | one `did:mata` held across boots, reflashes and three foreign flashes |
| the gated page | refused without its token, served with it |
| video over ten minutes | 20,119 packets, **one lost** |
| audio over ten minutes | 7,201 datagrams, **none lost** |
| the camera over the mesh | **721 packets in 60 s, none lost, none reordered** |
| adoption | accepted; a stranger refused; a superseded record refused |

**Three defects this facade had that only a board could show**, each now a
regression test: a media subscriber asking for "whatever this device makes" got
nothing, because the factory matched exact tags only and the test asked the one
way the board always answered; the async runtime was built before the platform
could serve the descriptor it needs; and its thread ran on an 8 KB stack while
a whole peer-to-peer node ran on it.

Every number, with the run that produced it:
[`docs/LEDGER.md`](https://github.com/Remade-With-Rust/rusty_esp_arduino/blob/main/docs/LEDGER.md).

## A sketch

```rust
use rusty_esp_arduino::prelude::*;

fn setup() {
    wifi::begin("home", &psk);          // or wifi::host(..) to be the network
    identity::begin(None);              // mints a did:mata, once, forever
    cam::begin(&cam::Config::qvga(12));
    stream::listen(80);                 // a gated MJPEG page
}

fn loop_once() {
    if let Some(frame) = cam::grab() {
        stream::push(&frame);           // borrowed -- no copy
    }
}
```

Run the same sketch on a laptop by installing the host board, whose camera is a
test pattern and whose microphone is a tone:

```rust
board::install(HostBoard::new());
```

## Two tracks

| track | what it is | this crate |
|---|---|---|
| **A** | `std` on ESP-IDF — where the sketch shape belongs | default |
| **B** | `no_std` on `esp-hal` | use the function packages directly; the facade assumes an operating system |

## Part of Janus

**Janus** rebuilds the Espressif ESP32 and Arduino application portfolio as
independent, memory-safe Rust packages — so a hardware maker can ship a device
that the [MATA](https://www.mata.network) home computer discovers, catalogs honestly, adopts
under its own identity, and pays for. Ten packages, three layers, and the
dependency direction never reverses.

| layer | packages |
|---|---|
| **0 — the vocabulary** | [`rusty_esp_core`](https://crates.io/crates/rusty_esp_core) · [`rusty_esp_dsp`](https://crates.io/crates/rusty_esp_dsp) |
| **1 — the functions** | [`rusty_esp_image`](https://crates.io/crates/rusty_esp_image) · [`rusty_esp_video`](https://crates.io/crates/rusty_esp_video) · [`rusty_esp_audio`](https://crates.io/crates/rusty_esp_audio) · [`rusty_esp_signal`](https://crates.io/crates/rusty_esp_signal) · [`rusty_esp_mid`](https://crates.io/crates/rusty_esp_mid) · [`rusty_esp_iroh`](https://crates.io/crates/rusty_esp_iroh) |
| **2 — the surfaces** | [`rusty_esp_arduino`](https://crates.io/crates/rusty_esp_arduino) — the sketch facade · `espino` — the maker's CLI (not published) |

Every package is host-verified against an external oracle and keeps a ledger
in which no number appears without the run that produced it. **Five of seven
device profiles have now run their kill tests on real silicon**, three of them
over a Wi-Fi network the board hosts itself.

Also check out the rest of [Remade With Rust](https://github.com/remade-with-rust) — including
[`rusty_alloc`](https://crates.io/crates/rusty_alloc), the pure-Rust rebuild of
mimalloc that these firmwares run on, and
[`rusty_jpeg`](https://crates.io/crates/rusty_jpeg), the JPEG engine behind the
camera path — and our sister project
[remade_ffmpeg_rs](https://github.com/Remade-With-Rust/remade_ffmpeg_rs), a ground-up Rust rebuild of FFmpeg.

## About Mata Network

[Mata Network](https://www.mata.network) builds sovereign, self-hostable infrastructure.
**Remade With Rust** is our open-source home for the permissively-licensed
building blocks that work depends on.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/rusty_esp_arduino/blob/main/LICENSE-MIT)
and [LICENSE-APACHE](https://github.com/Remade-With-Rust/rusty_esp_arduino/blob/main/LICENSE-APACHE).
