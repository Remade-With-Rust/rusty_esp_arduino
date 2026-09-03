# rusty_esp_arduino — plan

> The Arduino-shaped sketch facade over the Janus function packages. The API
> is the umbrella's `docs/plans/rusty-ESP-arduino.md` §8; this file is what
> exists, the seam, and what waits for a board.
>
> **Status:** M4 ◐ — host half 2026-09-02 (prelude, `Board` seam, `HostBoard`,
> three stream outputs, tests, ffmpeg oracle); board files ✅ (three records);
> the S3 firmware builds ✅ (app image 1,092,080 B); the board run ☐.

## 1. The rule

Thin by law. A function in this crate is one call through the `Board` seam
into a function package, or one call into `rusty_esp_video-esp`'s stream
code. The facade owns no codec, no packet format, no pixel loop, and grows
no Arduino core. When a sketch needs something the facade lacks, the
question is which function package it belongs to, and the facade gains one
function that calls it.

## 2. The seam

```rust
pub trait Board: Send {
    fn wifi_begin(&mut self, ssid: &str, psk: &str) -> Result<()>;
    fn local_ip(&self) -> Option<IpAddr>;
    fn cam_begin(&mut self, config: &cam::Config) -> Result<()>;
    fn cam_grab(&mut self) -> Result<Option<Jpeg>>;
    fn mic_begin(&mut self, config: &mic::Config) -> Result<()>;
    fn mic_read(&mut self) -> Result<Option<Pcm>>;
    fn millis(&self) -> u64;
}
```

Installed once (`board::install`), behind a mutex, reached by the free
functions. A board owns peripherals only; the stream side is not a board
method because it is the same code everywhere (`rusty_esp_video-esp::net`
and `::udp_net` under `std`, on the laptop and on ESP-IDF alike).

`Jpeg` and `Pcm` are owned (`Vec<u8>`): the sketch surface allocates per
frame by design, on the std track, so a maker holds a frame without a
lifetime. The function packages underneath keep their borrowed-slice APIs.

Errors are Arduino-shaped: `bool` / `Option` out, the reason in
`last_error()`, cleared by the next success. A board that has not `begin`-ed
says which peripheral.

## 3. Milestones

| # | what | gate |
|---|---|---|
| **M4 host half** ✅ 2026-09-02 | prelude; `Board`; `HostBoard` (pattern / JPEG-dir camera, tone / WAV microphone, paced like a sensor); `stream::{listen, rtp_to, pcm_to, push_jpeg, push_pcm}`; `sketch::{run, run_for}` | the plan's sketch runs on the laptop; every frame and block comes back through `rusty_esp_video-esp`'s receivers, counted; the page and the stream read back over TCP; ffmpeg decodes `/stream` — ledger |
| **M4 board files** ✅ 2026-09-02 | `boards/` schema v1 and three records — `xiao-esp32s3-sense`, `esp32-s3-eye` (the second board the plan asks for), `ai-thinker-esp32-cam` — pins from the vendors' published tables, cited per record | `tests/boards.rs`: every record parses, keeps the schema, stays inside the chip's GPIO range, claims no GPIO twice |
| **M4 board half** ◐ | `firmware/xiao-s3-sense-idf-sketch` ✅ 2026-09-02 (software): `EspBoard` over `rusty_esp_image-esp` (camera), `rusty_esp_audio-esp` (PDM), ESP-IDF Wi-Fi; the README's sketch, unchanged; builds for the S3, app image 1,092,080 B (ledger) | ☐ on the board: the same tests' receivers on a laptop, the board sending; the record in `boards/` read by the firmware rather than the image crate's constant |
| **M5 hook** ☐ | `rusty_esp_dsp` PIE kernels reach a sketch only through the function packages — nothing to do here | — |

## 4. Decision log

| Date | Decision |
|---|---|
| 2026-09-02 | One std crate, no `-core`/`-esp` split: the facade is the Track A surface and has no bare-metal half; a chip's `Board` lives in its firmware project (or a later `-esp` crate if two boards share one). |
| 2026-09-02 | `HostBoard` behind a default-on `host` feature so a firmware turns it (and `rusty_jpeg`, `rusty_esp_image-core`) off. |
| 2026-09-02 | The MJPEG server thread holds the latest frame in a slot and each `/stream` waits for a newer one: a slow client drops frames rather than delaying the sketch, which is what a board must do too. |
| 2026-09-02 | `stream::listen` binds `0.0.0.0` — the page is for another device on the LAN, which is the product. |
