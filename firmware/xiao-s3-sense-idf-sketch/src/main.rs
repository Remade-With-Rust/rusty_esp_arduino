//! Janus M4 on the XIAO ESP32-S3 Sense: the README's sketch, on the board.
//!
//! `setup` and `loop_once` below are the facade README's, line for line; the
//! only board-specific line is `board::install(EspBoard::take())`. The
//! kill test: `http://<ip>/` opens the MJPEG page from the sensor, and with
//! `JANUS_PCM_DEST` set the microphone's blocks arrive at the laptop as the
//! same datagrams `tests/loopback.rs` counts. Credentials are compile-time:
//!
//! ```sh
//! JANUS_WIFI_SSID=mynet JANUS_WIFI_PASS=secret cargo run --release
//! JANUS_PCM_DEST=192.168.1.20:5006 …                      # optional
//! ```

#![deny(unsafe_code)]

use std::net::IpAddr;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio41, Gpio42};
use esp_idf_svc::hal::i2s::I2S0;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{link_patches, EspError};
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use rusty_esp_arduino::board::Board;
use rusty_esp_arduino::error::Result as SketchResult;
use rusty_esp_arduino::prelude::*;
use rusty_esp_audio_core::AudioSource;
use rusty_esp_audio_esp::idf::PdmIn;
use rusty_esp_core::error::Error as CoreError;
use rusty_esp_image_core::sensor::{FrameSize, Mode};
use rusty_esp_image_core::source::ImageSource;
use rusty_esp_image_esp::idf::IdfCamera;
use rusty_esp_image_esp::XIAO_ESP32S3_SENSE;

const SSID: &str = env!("JANUS_WIFI_SSID");
const PASS: &str = env!("JANUS_WIFI_PASS");

// ------------------------------------------------------------- the sketch --

fn setup() {
    board::install(EspBoard::take().expect("the chip's peripherals, taken once at boot"));
    if !wifi::begin(SSID, PASS) {
        log::error!("wifi: {:?}", last_error());
    }
    if !cam::begin(cam::Config::qvga_jpeg()) {
        log::error!("camera: {:?}", last_error());
    }
    if !mic::begin(mic::Config::pcm16_16k()) {
        log::error!("microphone: {:?}", last_error());
    }
    if stream::listen(80) {
        log::info!(
            "janus m4: http://{:?}/  (stream at /stream)",
            wifi::local_ip()
        );
    } else {
        log::error!("listen: {:?}", last_error());
    }
    if let Some(dest) = option_env!("JANUS_PCM_DEST") {
        log::info!("pcm -> {dest}: {}", stream::pcm_to(dest));
    }
}

fn loop_once() {
    if let Some(frame) = cam::grab() {
        stream::push_jpeg(&frame);
    }
    if let Some(pcm) = mic::read() {
        stream::push_pcm(&pcm);
    }
    let s = stream::stats();
    if s.jpeg_pushed % 150 == 0 && s.jpeg_pushed > 0 {
        log::info!(
            "{} s: {} frames, {} blocks, http {:?}",
            millis() / 1000,
            s.jpeg_pushed,
            s.pcm_pushed,
            s.http
        );
    }
}

fn main() {
    link_patches();
    EspLogger::initialize_default();
    sketch::run(setup, loop_once);
}

// --------------------------------------------------------------- the board --

/// The XIAO ESP32-S3 Sense as a `Board`: OV2640 through `rusty_esp_image-esp`,
/// the PDM microphone through `rusty_esp_audio-esp`, Wi-Fi through ESP-IDF.
/// Peripherals are taken once at boot and handed to each `begin`.
struct EspBoard {
    started: Instant,
    net: Option<(Modem<'static>, EspSystemEventLoop, EspDefaultNvsPartition)>,
    pdm: Option<(I2S0<'static>, Gpio42<'static>, Gpio41<'static>)>,
    wifi: Option<BlockingWifi<EspWifi<'static>>>,
    camera: Option<Cam>,
    mic: Option<Mic>,
}

struct Cam {
    camera: IdfCamera,
    buf: Vec<u8>,
    width: u16,
    height: u16,
    interval: Duration,
    seq: u64,
}

struct Mic {
    pdm: PdmIn<'static>,
    buf: Vec<u8>,
}

/// Largest JPEG a grab may return; QVGA from an OV2640 is 10–25 KB, VGA under
/// 100 KB. Lives in PSRAM.
const FRAME_BYTES: usize = 256 * 1024;

fn esp(e: EspError) -> Error {
    Error::Io(e.to_string())
}

fn frame_size_for(width: u32, height: u32) -> Option<FrameSize> {
    [
        FrameSize::S96x96,
        FrameSize::Qqvga,
        FrameSize::S128x128,
        FrameSize::Qcif,
        FrameSize::Hqvga,
        FrameSize::S240x240,
        FrameSize::Qvga,
        FrameSize::S320x320,
        FrameSize::Cif,
        FrameSize::Hvga,
        FrameSize::Vga,
        FrameSize::Svga,
        FrameSize::Xga,
    ]
    .into_iter()
    .find(|s| s.dimensions() == (width, height))
}

impl EspBoard {
    /// Take the chip's peripherals. Once per boot.
    fn take() -> anyhow::Result<Self> {
        let peripherals = Peripherals::take()?;
        let sysloop = EspSystemEventLoop::take()?;
        let nvs = EspDefaultNvsPartition::take()?;
        Ok(EspBoard {
            started: Instant::now(),
            net: Some((peripherals.modem, sysloop, nvs)),
            pdm: Some((
                peripherals.i2s0,
                peripherals.pins.gpio42,
                peripherals.pins.gpio41,
            )),
            wifi: None,
            camera: None,
            mic: None,
        })
    }

    fn now(&self) -> rusty_esp_core::time::Micros {
        rusty_esp_core::time::Micros(self.started.elapsed().as_micros() as u64)
    }

    /// Sleep until frame `seq` at `interval` is due, so the sketch's loop
    /// runs at the configured rate rather than the sensor's.
    fn pace(&self, seq: u64, interval: Duration) {
        let due = self.started + interval.saturating_mul(u32::try_from(seq).unwrap_or(u32::MAX));
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
}

impl Board for EspBoard {
    fn wifi_begin(&mut self, ssid: &str, psk: &str) -> SketchResult<()> {
        let Some((modem, sysloop, nvs)) = self.net.take() else {
            return Err(Error::Io("wifi: begin was already called".into()));
        };
        let mut wifi = BlockingWifi::wrap(
            EspWifi::new(modem, sysloop.clone(), Some(nvs)).map_err(esp)?,
            sysloop,
        )
        .map_err(esp)?;
        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid
                .try_into()
                .map_err(|_| Error::Io("ssid longer than 32 bytes".into()))?,
            password: psk
                .try_into()
                .map_err(|_| Error::Io("password longer than 64 bytes".into()))?,
            ..Default::default()
        }))
        .map_err(esp)?;
        wifi.start().map_err(esp)?;
        wifi.connect().map_err(esp)?;
        wifi.wait_netif_up().map_err(esp)?;
        self.wifi = Some(wifi);
        Ok(())
    }

    fn local_ip(&self) -> Option<IpAddr> {
        let wifi = self.wifi.as_ref()?;
        let info = wifi.wifi().sta_netif().get_ip_info().ok()?;
        Some(IpAddr::V4(info.ip))
    }

    fn cam_begin(&mut self, config: &cam::Config) -> SketchResult<()> {
        if config.fps == 0 {
            return Err(CoreError::Unsupported.into());
        }
        let size = frame_size_for(config.width, config.height).ok_or(CoreError::Unsupported)?;
        let mut mode = Mode::jpeg(size)?;
        mode.fps = u8::try_from(config.fps).unwrap_or(u8::MAX);
        mode.jpeg_quality = config.quality.clamp(1, 100);
        let camera = IdfCamera::init(&XIAO_ESP32S3_SENSE, &mode, 2)?;
        self.camera = Some(Cam {
            camera,
            buf: vec![0u8; FRAME_BYTES],
            width: u16::try_from(config.width).map_err(|_| CoreError::Unsupported)?,
            height: u16::try_from(config.height).map_err(|_| CoreError::Unsupported)?,
            interval: Duration::from_micros(1_000_000 / u64::from(config.fps)),
            seq: 0,
        });
        Ok(())
    }

    fn cam_grab(&mut self) -> SketchResult<Option<Jpeg>> {
        let Some(cam) = self.camera.as_ref() else {
            return Err(Error::NotBegun("camera"));
        };
        self.pace(cam.seq, cam.interval);
        let Some(cam) = self.camera.as_mut() else {
            return Err(Error::NotBegun("camera"));
        };
        cam.seq += 1;
        let frame = cam.camera.grab(&mut cam.buf)?;
        let bytes = frame.coded().ok_or(CoreError::InvalidFormat)?;
        Ok(Some(Jpeg {
            bytes: bytes.to_vec(),
            width: cam.width,
            height: cam.height,
            timestamp: frame.timestamp,
        }))
    }

    fn mic_begin(&mut self, config: &mic::Config) -> SketchResult<()> {
        if config.frame_ms == 0 || config.format.channels != 1 {
            return Err(CoreError::Unsupported.into());
        }
        let Some((i2s0, clk, din)) = self.pdm.take() else {
            return Err(Error::Io("microphone: begin was already called".into()));
        };
        let pdm = PdmIn::new(i2s0, clk, din, config.format.sample_rate_hz)?;
        let block_bytes = pdm
            .format()
            .bytes_for_micros(u64::from(config.frame_ms) * 1000)
            .max(pdm.format().frame_bytes());
        self.mic = Some(Mic {
            pdm,
            buf: vec![0u8; block_bytes],
        });
        Ok(())
    }

    fn mic_read(&mut self) -> SketchResult<Option<Pcm>> {
        let Some(mic) = self.mic.as_mut() else {
            return Err(Error::NotBegun("microphone"));
        };
        let block = mic.pdm.read(&mut mic.buf)?;
        Ok(Some(Pcm {
            format: block.format,
            bytes: block.data.to_vec(),
            timestamp: block.timestamp,
        }))
    }

    fn millis(&self) -> u64 {
        self.now().0 / 1000
    }
}
