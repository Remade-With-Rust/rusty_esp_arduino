# xiao-s3-sense-idf-sketch

The facade README's sketch on the Seeed XIAO ESP32-S3 Sense, Track A (`std`
on ESP-IDF v5.5). `setup` / `loop_once` are the README's, line for line;
`EspBoard` (in `main.rs`, ~150 lines) is the board: OV2640 through
`rusty_esp_image-esp::idf::IdfCamera`, the PDM microphone through
`rusty_esp_audio-esp::idf::PdmIn`, Wi-Fi through `esp-idf-svc`. Pins are
the ones `boards/xiao-esp32s3-sense.toml` records (the image crate's
`XIAO_ESP32S3_SENSE` table and GPIO42 / GPIO41 for the microphone).

## Build

Same toolchain and walls as `rusty_esp_video`'s `xiao-s3-sense-idf-mjpeg`
(its README has the long form): `espup install`, the ESP-IDF tools in the
global dir, a non-venv Python 3 first on `PATH`, and on Windows a short
`CARGO_TARGET_DIR`.

```sh
export CARGO_TARGET_DIR=C:/janus-f                 # Windows only
JANUS_WIFI_SSID=yournet JANUS_WIFI_PASS=yourpass cargo build --release
JANUS_WIFI_SSID=yournet JANUS_WIFI_PASS=yourpass cargo run --release   # espflash flash --monitor
JANUS_PCM_DEST=192.168.1.20:5006 …                                     # also send the microphone
```

## Kill test (needs the board)

- `http://<ip>/` shows the sensor at 320×240; `ffmpeg -i http://<ip>/stream
  -frames:v 30 -f null -` decodes 30 frames (the same probe the facade's
  ledger ran against the laptop board).
- With `JANUS_PCM_DEST` set, the laptop's `rusty_esp_video-esp::udp_net::receive_raw`
  counts one 640-byte block per 20 ms.
- The log prints frames, blocks and the HTTP counters every 150 frames.

Until a board is on the bench this project is a compile gate: it proves the
facade, the two chip crates and ESP-IDF link into one image for the S3.
