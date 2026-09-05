# rusty_esp_arduino — the ledger

Every number this package claims, with the run that produced it. A row
without a method is not a number.

## M4 host half: the sketch on the laptop (2026-09-02)

Laptop, Windows 11, stable Rust, `cargo test --workspace` in the umbrella
checkout (siblings patched to their local checkouts).

| gate | result |
|---|---|
| unit (`host`: SOF parser, pattern camera, tone, WAV loop) | 4 pass |
| `tests/no_board.rs` — every call refused without a board, the peripheral named before `begin`, the record cleared by a success | 1 pass |
| `tests/loopback.rs` — the plan's sketch, 12 loops, colour bars + tone → `stream::rtp_to` + `stream::pcm_to` → `rusty_esp_video-esp::udp_net::{receive_rtp_jpeg, receive_raw}` on loopback | 1 pass: **12 / 12 frames** (320×240, every datagram sent was received, 0 lost, 0 dropped), **12 / 12 PCM blocks** of 640 B (20 ms at 16 kHz mono i16), 0 lost |
| `tests/http.rs` — `stream::listen(0)`, `/` read back (200, links `/stream`), `/stream` read back until three JPEG starts (200, `multipart/x-mixed-replace`), server counters folded in | 1 pass, 2.0 s |
| doctest (the README sketch, `no_run`) | 1 pass |
| `tests/boards.rs` — the three `boards/` records parse, hold schema v1, stay inside the chip's GPIO range (ESP32 ≤ 39, S3 ≤ 48), claim no GPIO twice, cite a source per peripheral; the XIAO record matches the pins the Janus firmwares already use | 2 pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo deny check` | clean (`IJG` allowed for `rusty_jpeg`'s tables; the local-patch warning is the note below) |

**External oracle, ffmpeg 8.1.2** (`examples/cam_mic`, `JANUS_HTTP_PORT=18080
JANUS_RTP_DEST=127.0.0.1:5004`, colour bars at QVGA 15 fps):

| probe | result |
|---|---|
| `ffmpeg -i http://127.0.0.1:18080/stream -frames:v 30 -f null -` | `Input #0, mpjpeg` · `Video: mjpeg (Baseline), yuvj420p, 320x240` · **frame= 30** |
| `ffmpeg -protocol_whitelist file,udp,rtp -i janus.sdp -frames:v 30 -f null -` (`m=video 5004 RTP/AVP 26`, `a=rtpmap:26 JPEG/90000`) | **frame= 30** |
| the example's own counters at 9 s | 150 frames pushed, `HttpStats { connections: 2, streams: 2, frames: 62 }` |

So the page a browser opens and the RTP a player receives are the same
bytes `rusty_esp_video` already proved against ffmpeg (its V2 rows); the
facade adds no format of its own, and this run shows it added no defect.

Notes:

- The loop is paced by whichever peripheral is due last: at 15 fps the
  camera sets the loop at ~66 ms, so a 20 ms microphone block per loop
  under-samples the tone on the laptop board (the example prints
  `0 blocks` when `JANUS_PCM_DEST` is unset, and one block per loop when
  set). A board's I2S DMA buffers between reads; the laptop board does not
  pretend to. A sketch that needs all the audio runs the microphone in its
  own loop, which is the function packages' job, not the facade's.
- `cargo deny check` in the umbrella checkout warns `unmatched-source` for
  every patched sibling (the lock records a patched crate without its git
  source); in CI, where the siblings resolve from GitHub, the sources match.

## M4 board half, the software part: the sketch firmware builds for the S3 (2026-09-02)

`firmware/xiao-s3-sense-idf-sketch`: the README's `setup` / `loop_once`
over `EspBoard` (`rusty_esp_image-esp::idf::IdfCamera`,
`rusty_esp_audio-esp::idf::PdmIn`, `esp-idf-svc` Wi-Fi), Track A,
`xtensa-esp32s3-espidf`, ESP-IDF v5.5.1, `espressif/esp32-camera ^2.0`,
esp toolchain, `--release` (`opt-level = "s"`, fat LTO).

| what | result |
|---|---|
| `cargo build --release` (dummy `JANUS_WIFI_*`, `CARGO_TARGET_DIR=C:/janus-f`) | **builds**; the bin, the facade and the two chip crates compiled last (the rest of the graph was warm from the first attempt, which failed on four missing `'static` lifetimes on the peripheral fields and nothing else) |
| ELF | 1,604,132 B |
| `espflash save-image --chip esp32s3` | **app image 1,092,080 B, 26.45 % of the 4,128,768 B app partition** (`PARTITION_TABLE_SINGLE_APP_LARGE`) |
| `unsafe` in the firmware | none (`#![deny(unsafe_code)]`); the camera's and the RNG's fences stay in the chip crates |

For scale, `rusty_esp_video`'s MJPEG-only firmware on the same board is
1,073,152 B: the facade, the PDM path and the two UDP senders cost about
19 KB of image. Nothing has been flashed; the board rows (frames at
`/stream`, blocks at `JANUS_PCM_DEST`) wait for the board.

## rusty_jpeg 0.4; rff reads the sketch (host, 2026-09-03)

The laptop board's pattern camera now encodes with `rusty_jpeg` 0.4 (the
same call, a crates.io bump); the eight tests pass unchanged. rff
(remade_ffmpeg_rs `e2c71cc`, built locally) joined ffmpeg as the reader of
what the sketch serves:

| probe | result |
|---|---|
| `rff -i http://127.0.0.1:18080/stream -c:v copy -f mjpeg out.mjpeg` for 12 s | `ffprobe -f mjpeg -count_frames`: **181 frames, 320×240** |
| `rff -i "rtp://0.0.0.0:5004?pt=26&timeout=3" -c:v copy -f mjpeg` while the sketch's `stream::rtp_to` sent for ~6 s | **98 frames, 320×240**, 98 packets written |

rff's documented `rtp://@:port` form received nothing on Windows at `e2c71cc`
(reported as remade_ffmpeg_rs#12). The report's cause was wrong: not an IPv6
bind but the empty host reaching `UdpSocket::bind` as `":5004"`, which Windows
resolves to one of the machine's own interfaces rather than the wildcard.
Fixed upstream in `de24a83` (PR #13), issue closed. Re-run on `ddc3355` with
the documented spelling: `rff -i "rtp://@:5004?pt=26&timeout=3" -c:v copy -f mjpeg`
while `stream::rtp_to` sent for ~5 s → **82 frames, 320×240**, 82 packets
written. Either spelling works now.

## `identity::begin` and the mesh verbs (host, 2026-09-04)

Laptop, Windows 11, stable Rust, `CARGO_TARGET_DIR=C:/janus-a`, siblings
patched to their checkouts (mid-core, iroh-host and iroh-core joined the
patch table).

| gate | result |
|---|---|
| `tests/identity.rs` | no board → `NoBoard`; a board without a store → `Missing`; the laptop board mints a `did:mata:…`; `end` and a new install on the **same store → the same DID**, now with a maker; `did:web:…` as maker refused with the message naming `did:mata`; another store → another DID; the key file is `mid.devkey` |
| `tests/mesh.rs` (`--features mesh`) | `mesh::begin` before identity refused by name (`NotBegun("identity")`); after `identity::begin`, the node comes up with the device's DID and a ticket; `begin` twice refused; the sketch pushes 60 frames at 40 ms from a thread while `rusty_esp_iroh-host::Client` subscribes through the ticket: **8 packets asked for, 8 received, 0 lost, each a pushed frame byte for byte, the subscriber's sequence 0..8**; `service()` counts ≥ 1 subscriber and ≥ 8 frames; **2.6 s** |
| `cargo test -p rusty_esp_arduino` (default features) | 7 suites, 11 tests, 0 failed |
| `cargo clippy --all-targets -- -D warnings` with and without `mesh` | clean |
| `cargo deny check` | clean, with `rusty_esp_mid`, `rusty_esp_iroh`, `mid` and n0's `rustls-rustcrypto` added to the allowed git sources |

Not run: a chip. The generated firmware's `EspBoard` (from `espino make`)
implements `take_kv` over `EspNvsKv::open_unchecked` and `take_rng` over
`EspRng::after_radio_start`; its first DID line on a serial port is the
board's row.
