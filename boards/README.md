# boards/

One record per board the sketch facade targets: which chip, how much
memory, and which GPIO each peripheral the facade exposes is on. A chip's
`Board` reads its record; a maker picks a board by name and never types a
pin number.

Schema v1 (every key required unless marked optional):

```toml
schema = 1

[board]
name = "xiao-esp32s3-sense"   # the file name, and what a maker names
vendor = "Seeed Studio"
chip = "esp32s3"              # esp32 | esp32s3 | esp32c6 | esp32p4
psram_mb = 8                  # 0 when none
flash_mb = 8
track = "idf"                 # idf (std) | hal (no_std); what the Janus firmware for it uses

[camera]                      # optional: boards without one omit the table
sensor = "ov2640"
interface = "dvp"             # dvp today; mipi-csi on the P4
xclk_hz = 20_000_000
pwdn = -1                     # -1 = not wired
reset = -1
xclk = 10
sccb_sda = 40
sccb_scl = 39
d0 = 15                       # DVP data pins, D0 = LSB (camera_pins.h's Y2)
d1 = 17
d2 = 18
d3 = 16
d4 = 14
d5 = 12
d6 = 11
d7 = 48                       # camera_pins.h's Y9
vsync = 38
href = 47
pclk = 13

[microphone]                  # optional
kind = "pdm"                  # pdm | i2s
clk = 42                      # pdm: CLK; i2s: SCLK (bit clock)
data = 41                     # pdm: DATA; i2s: DIN
ws = -1                       # i2s: LRCLK / word select; -1 for pdm

[led]                         # optional
gpio = 21
active = "low"                # low | high

[sources]                     # where every number above came from
camera = "…"
microphone = "…"
```

Pin numbers are facts about a board and are taken from the vendor's own
published tables, cited in `[sources]`. The record carries nothing else: no
driver settings, no defaults a sketch could change. `tests/boards.rs` parses
every record here and checks the schema, the pin ranges for the chip, and
that no two peripherals claim one GPIO.
