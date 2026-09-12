//! The laptop board: the same sketch, no chip.
//!
//! The camera is a colour-bar test pattern JPEG-encoded by `rusty_jpeg`, or a
//! directory of JPEGs played in name order; the microphone is a tone, or a
//! PCM WAV file played on a loop. Both pace themselves to the configured
//! rate like a sensor would, unless [`HostBoard::unpaced`] (tests). Wi-Fi
//! "joins" instantly: the laptop is already on its network.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusty_esp_audio_core::codec::wav::{WavCodec, WavHeader};
use rusty_esp_core::error::Error as CoreError;
use rusty_esp_core::error::Result as CoreResult;
use rusty_esp_core::frame::{Geometry, PixelFormat};
use rusty_esp_core::hal::{Kv, Rng, check_key};
use rusty_esp_core::pcm::{PcmFormat, SampleFormat};
use rusty_esp_core::time::Micros;
use rusty_esp_image_core::source::{ImageSource, TestPattern};

use crate::board::{Board, Jpeg, Pcm};
use crate::error::{Error, Result};
use crate::{cam, mic};

/// Where the laptop's frames come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Camera {
    /// Colour bars at the configured size, JPEG-encoded per frame.
    Pattern,
    /// Every `*.jpg` / `*.jpeg` in a directory, in name order, looped.
    JpegDir(PathBuf),
}

/// Where the laptop's audio comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Microphone {
    /// A sine tone in the configured format (`I16` only).
    Sine {
        /// Frequency in Hz.
        freq_hz: f32,
        /// Peak amplitude.
        amplitude: i16,
    },
    /// A PCM WAV file, looped; its own format wins over the configured one.
    Wav(PathBuf),
}

/// The laptop board.
pub struct HostBoard {
    camera: Camera,
    microphone: Microphone,
    paced: bool,
    started: Instant,
    ssid: Option<String>,
    cam: Option<CamRun>,
    mic: Option<MicRun>,
    store: PathBuf,
    store_taken: bool,
    provisioning: Option<String>,
    settings: Option<(String, String)>,
    radar: Option<crate::radar::Config>,
}

/// What the installed laptop board was last asked to publish on the local
/// network: `(hostname, instance, service_type, port, txt)`. The laptop has
/// no mDNS of its own, and a device nothing can discover is exactly the
/// defect, so the ask is recorded and [`advertised`] hands it to a test.
type Advert = (String, String, String, u16, Vec<(String, String)>);
static ADVERTISED: Mutex<Option<Advert>> = Mutex::new(None);

/// What the laptop board was last asked to publish, if anything.
#[must_use]
pub fn advertised() -> Option<Advert> {
    ADVERTISED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Every `max_fds` the installed laptop board was asked to prepare for, in
/// order. The laptop has nothing to register, but a sketch that reached a
/// runtime without asking is the defect that stopped C2's first boot, so the
/// ask is recorded and [`async_prepared`] hands it to a test.
static ASYNC_PREPARED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// What the laptop board was asked to prepare for, oldest first.
#[must_use]
pub fn async_prepared() -> Vec<usize> {
    ASYNC_PREPARED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// What a "sensor" saw: the queue a test fills with
/// [`inject_presence`]; `radar_read` drains it.
static READINGS: Mutex<Vec<crate::radar::Presence>> = Mutex::new(Vec::new());

/// Act as the sensor: hand the installed laptop board this reading; the
/// sketch's next `radar::read` returns it.
pub fn inject_presence(reading: crate::radar::Presence) {
    READINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(reading);
}

/// What a "phone" wrote to the laptop board: the credential queue a test
/// fills with [`inject_credentials`]; `provision_poll` drains it.
static INJECTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static LAST_PROVISIONING: Mutex<Option<String>> = Mutex::new(None);
static LAST_SETTINGS: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Act as the phone: hand the installed laptop board these credentials; the
/// sketch's next `provision::poll` returns them.
pub fn inject_credentials(ssid: &str, psk: &str) {
    INJECTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((ssid.to_owned(), psk.to_owned()));
}

/// The name the laptop board last advertised under, for tests.
#[must_use]
pub fn provisioning_name() -> Option<String> {
    LAST_PROVISIONING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// How many times the sketch asked the laptop board to restart.
static RESTARTS: AtomicUsize = AtomicUsize::new(0);

/// How many restarts the laptop board was asked for, for tests.
#[must_use]
pub fn restarts() -> usize {
    RESTARTS.load(Ordering::Relaxed)
}

/// The settings the laptop board last stored, for tests.
#[must_use]
pub fn stored_settings() -> Option<(String, String)> {
    LAST_SETTINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

impl fmt::Debug for HostBoard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostBoard")
            .field("camera", &self.camera)
            .field("microphone", &self.microphone)
            .field("paced", &self.paced)
            .field("ssid", &self.ssid)
            .field("cam_begun", &self.cam.is_some())
            .field("mic_begun", &self.mic.is_some())
            .finish()
    }
}

impl Default for HostBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl HostBoard {
    /// Colour bars and a 440 Hz tone.
    #[must_use]
    pub fn new() -> Self {
        HostBoard {
            camera: Camera::Pattern,
            microphone: Microphone::Sine {
                freq_hz: 440.0,
                amplitude: 8000,
            },
            paced: true,
            started: Instant::now(),
            ssid: None,
            cam: None,
            mic: None,
            store: default_store(),
            store_taken: false,
            provisioning: None,
            settings: None,
            radar: None,
        }
    }

    /// [`Self::new`], then `JANUS_CAM_DIR` (a directory of JPEGs) and
    /// `JANUS_WAV` (a PCM WAV file) from the environment when set.
    #[must_use]
    pub fn from_env() -> Self {
        let mut board = Self::new();
        if let Some(dir) = std::env::var_os("JANUS_CAM_DIR") {
            board = board.with_camera(Camera::JpegDir(PathBuf::from(dir)));
        }
        if let Some(wav) = std::env::var_os("JANUS_WAV") {
            board = board.with_microphone(Microphone::Wav(PathBuf::from(wav)));
        }
        if let Some(dir) = std::env::var_os("JANUS_KV_DIR") {
            board = board.with_store(PathBuf::from(dir));
        }
        board
    }

    /// Where the device's keys live on the laptop (`JANUS_KV_DIR`, else a
    /// `janus-host-kv` directory in the temp dir). Plaintext; the directory
    /// is the host's stand-in for NVS, and a development one.
    #[must_use]
    pub fn with_store(mut self, dir: impl Into<PathBuf>) -> Self {
        self.store = dir.into();
        self
    }

    /// Use another frame source.
    #[must_use]
    pub fn with_camera(mut self, camera: Camera) -> Self {
        self.camera = camera;
        self
    }

    /// Use another audio source.
    #[must_use]
    pub fn with_microphone(mut self, microphone: Microphone) -> Self {
        self.microphone = microphone;
        self
    }

    /// Deliver frames and blocks as fast as they are asked for (tests).
    #[must_use]
    pub fn unpaced(mut self) -> Self {
        self.paced = false;
        self
    }

    fn now(&self) -> Micros {
        Micros(self.started.elapsed().as_micros() as u64)
    }

    /// Sleep until item `seq` of a stream at `interval` is due.
    fn pace(&self, seq: u64, interval: Duration) {
        if !self.paced {
            return;
        }
        let due = self.started + interval.saturating_mul(u32::try_from(seq).unwrap_or(u32::MAX));
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
}

// ------------------------------------------------------------------ camera --

enum CamState {
    Pattern {
        pattern: TestPattern,
        rgb: Vec<u8>,
        width: u16,
        height: u16,
        quality: u8,
    },
    Dir {
        frames: Vec<(Vec<u8>, u16, u16)>,
        next: usize,
    },
}

struct CamRun {
    state: CamState,
    interval: Duration,
    seq: u64,
}

impl CamRun {
    fn grab(&mut self, timestamp: Micros) -> Result<Jpeg> {
        self.seq += 1;
        match &mut self.state {
            CamState::Pattern {
                pattern,
                rgb,
                width,
                height,
                quality,
            } => {
                pattern.grab(rgb)?;
                let mut jpeg = Vec::new();
                let enc = rusty_jpeg::encode::Encoder::new(&mut jpeg, *quality);
                enc.encode(rgb, *width, *height, rusty_jpeg::encode::ColorType::Rgb)
                    .map_err(|_| CoreError::Hardware)?;
                Ok(Jpeg {
                    bytes: jpeg,
                    width: *width,
                    height: *height,
                    timestamp,
                })
            }
            CamState::Dir { frames, next } => {
                let (bytes, width, height) = &frames[*next % frames.len()];
                *next = (*next + 1) % frames.len();
                Ok(Jpeg {
                    bytes: bytes.clone(),
                    width: *width,
                    height: *height,
                    timestamp,
                })
            }
        }
    }
}

/// Width and height from a JPEG's SOF marker; `None` for anything that is
/// not a JPEG with a frame header before its scan.
#[must_use]
pub fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            if i + 9 > bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]);
            return Some((width, height));
        }
        if matches!(marker, 0xD9 | 0xDA) {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn load_dir(dir: &Path) -> Result<Vec<(Vec<u8>, u16, u16)>> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        })
        .collect();
    names.sort();
    let mut frames = Vec::with_capacity(names.len());
    for path in &names {
        let bytes = std::fs::read(path)?;
        let (w, h) = jpeg_size(&bytes).ok_or(CoreError::InvalidFormat)?;
        frames.push((bytes, w, h));
    }
    if frames.is_empty() {
        return Err(Error::Io(format!("{}: no JPEG files", dir.display())));
    }
    Ok(frames)
}

// -------------------------------------------------------------- microphone --

enum MicState {
    Sine {
        format: PcmFormat,
        step: f32,
        phase: f32,
        amplitude: i16,
    },
    Wav {
        format: PcmFormat,
        data: Vec<u8>,
        pos: usize,
    },
}

impl MicState {
    fn format(&self) -> PcmFormat {
        match self {
            MicState::Sine { format, .. } | MicState::Wav { format, .. } => *format,
        }
    }
}

struct MicRun {
    state: MicState,
    block_bytes: usize,
    interval: Duration,
    seq: u64,
}

impl MicRun {
    fn read(&mut self, timestamp: Micros) -> Pcm {
        self.seq += 1;
        let n = self.block_bytes;
        match &mut self.state {
            MicState::Sine {
                format,
                step,
                phase,
                amplitude,
            } => {
                let channels = usize::from(format.channels).max(1);
                let frames = n / (2 * channels);
                let mut bytes = Vec::with_capacity(frames * 2 * channels);
                for _ in 0..frames {
                    let v =
                        (f32::from(*amplitude) * (core::f32::consts::TAU * *phase).sin()) as i16;
                    *phase += *step;
                    if *phase >= 1.0 {
                        *phase -= 1.0;
                    }
                    for _ in 0..channels {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                }
                Pcm {
                    format: *format,
                    bytes,
                    timestamp,
                }
            }
            MicState::Wav { format, data, pos } => {
                let mut bytes = Vec::with_capacity(n);
                while bytes.len() < n {
                    let take = (n - bytes.len()).min(data.len() - *pos);
                    bytes.extend_from_slice(&data[*pos..*pos + take]);
                    *pos = (*pos + take) % data.len();
                }
                Pcm {
                    format: *format,
                    bytes,
                    timestamp,
                }
            }
        }
    }
}

fn load_wav(path: &Path) -> Result<(PcmFormat, Vec<u8>)> {
    let bytes = std::fs::read(path)?;
    let (header, data_at) = WavHeader::parse(&bytes)?;
    let WavCodec::Pcm(format) = header.codec else {
        return Err(CoreError::Unsupported.into());
    };
    let end = data_at
        .saturating_add(header.data_len as usize)
        .min(bytes.len());
    let data = bytes.get(data_at..end).unwrap_or(&[]).to_vec();
    if data.is_empty() {
        return Err(Error::Io(format!("{}: no PCM data", path.display())));
    }
    Ok((format, data))
}

// ------------------------------------------------------------------- board --

impl Board for HostBoard {
    fn wifi_begin(&mut self, ssid: &str, _psk: &str) -> Result<()> {
        self.ssid = Some(ssid.to_owned());
        Ok(())
    }

    fn local_ip(&self) -> Option<IpAddr> {
        self.ssid.as_ref()?;
        // The address the OS would route a packet from; no packet is sent.
        let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
        probe.connect("192.0.2.1:9").ok()?;
        probe
            .local_addr()
            .ok()
            .map(|a| a.ip())
            .or(Some(IpAddr::V4(Ipv4Addr::LOCALHOST)))
    }

    fn cam_begin(&mut self, config: &cam::Config) -> Result<()> {
        if config.fps == 0 || config.width == 0 || config.height == 0 {
            return Err(CoreError::Unsupported.into());
        }
        let state = match &self.camera {
            Camera::Pattern => {
                let g = Geometry::new(config.width, config.height, PixelFormat::Rgb888)?;
                let width = u16::try_from(config.width).map_err(|_| CoreError::Unsupported)?;
                let height = u16::try_from(config.height).map_err(|_| CoreError::Unsupported)?;
                CamState::Pattern {
                    pattern: TestPattern::new(g, config.fps)?,
                    rgb: vec![0u8; g.byte_len().ok_or(CoreError::Unsupported)?],
                    width,
                    height,
                    quality: config.quality.clamp(1, 100),
                }
            }
            Camera::JpegDir(dir) => CamState::Dir {
                frames: load_dir(dir)?,
                next: 0,
            },
        };
        self.cam = Some(CamRun {
            state,
            interval: Duration::from_micros(1_000_000 / u64::from(config.fps)),
            seq: 0,
        });
        Ok(())
    }

    fn cam_grab(&mut self) -> Result<Option<Jpeg>> {
        let Some(run) = self.cam.as_ref() else {
            return Err(Error::NotBegun("camera"));
        };
        self.pace(run.seq, run.interval);
        let timestamp = self.now();
        let Some(run) = self.cam.as_mut() else {
            return Err(Error::NotBegun("camera"));
        };
        run.grab(timestamp).map(Some)
    }

    fn mic_begin(&mut self, config: &mic::Config) -> Result<()> {
        if config.frame_ms == 0 {
            return Err(CoreError::Unsupported.into());
        }
        let state = match &self.microphone {
            Microphone::Sine { freq_hz, amplitude } => {
                if config.format.sample != SampleFormat::I16 {
                    return Err(CoreError::Unsupported.into());
                }
                MicState::Sine {
                    format: config.format,
                    step: freq_hz / config.format.sample_rate_hz as f32,
                    phase: 0.0,
                    amplitude: *amplitude,
                }
            }
            Microphone::Wav(path) => {
                let (format, data) = load_wav(path)?;
                MicState::Wav {
                    format,
                    data,
                    pos: 0,
                }
            }
        };
        let block_bytes = state
            .format()
            .bytes_for_micros(u64::from(config.frame_ms) * 1000)
            .max(1);
        self.mic = Some(MicRun {
            state,
            block_bytes,
            interval: Duration::from_millis(u64::from(config.frame_ms)),
            seq: 0,
        });
        Ok(())
    }

    fn mic_read(&mut self) -> Result<Option<Pcm>> {
        let Some(run) = self.mic.as_ref() else {
            return Err(Error::NotBegun("microphone"));
        };
        self.pace(run.seq, run.interval);
        let timestamp = self.now();
        let Some(run) = self.mic.as_mut() else {
            return Err(Error::NotBegun("microphone"));
        };
        Ok(Some(run.read(timestamp)))
    }

    fn millis(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn take_kv(&mut self) -> Option<Box<dyn Kv + Send>> {
        if self.store_taken {
            return None;
        }
        self.store_taken = true;
        FileKv::open(&self.store)
            .ok()
            .map(|kv| Box::new(kv) as Box<dyn Kv + Send>)
    }

    fn advertise(
        &mut self,
        hostname: &str,
        instance: &str,
        service_type: &str,
        port: u16,
        txt: &[(String, String)],
    ) -> Result<()> {
        *ADVERTISED
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((
            hostname.to_owned(),
            instance.to_owned(),
            service_type.to_owned(),
            port,
            txt.to_vec(),
        ));
        Ok(())
    }

    fn prepare_async(&mut self, max_fds: usize) -> Result<()> {
        ASYNC_PREPARED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(max_fds);
        Ok(())
    }

    fn take_rng(&mut self) -> Option<Box<dyn Rng + Send>> {
        Some(Box::new(HostRng))
    }

    fn provision_begin(&mut self, name: &str) -> Result<()> {
        self.provisioning = Some(name.to_owned());
        *LAST_PROVISIONING
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(name.to_owned());
        Ok(())
    }

    fn provision_poll(&mut self) -> Result<Option<(String, String)>> {
        if self.provisioning.is_none() {
            return Ok(None);
        }
        let mut q = INJECTED.lock().unwrap_or_else(PoisonError::into_inner);
        if q.is_empty() {
            Ok(None)
        } else {
            Ok(Some(q.remove(0)))
        }
    }

    fn provision_report(&mut self, joined: bool) -> Result<()> {
        if joined {
            self.provisioning = None;
        }
        Ok(())
    }

    /// The laptop cannot restart itself and must not try; it records the
    /// request so a test can see the sketch asked.
    fn restart(&mut self) -> Result<()> {
        RESTARTS.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn radar_begin(&mut self, config: &crate::radar::Config) -> Result<()> {
        self.radar = Some(*config);
        Ok(())
    }

    fn radar_read(&mut self) -> Result<Option<crate::radar::Presence>> {
        if self.radar.is_none() {
            return Ok(None);
        }
        let mut q = READINGS.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        })
    }

    fn store_settings(&mut self, ssid: &str, psk: &str) -> Result<()> {
        self.settings = Some((ssid.to_owned(), psk.to_owned()));
        *LAST_SETTINGS.lock().unwrap_or_else(PoisonError::into_inner) = self.settings.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_size_reads_sof0_and_refuses_non_jpegs() {
        // SOI, APP0 (2 bytes), SOF0 with 8 bytes of payload: precision 8,
        // height 240, width 320, 1 component
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02];
        j.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0xF0, 0x01, 0x40, 0x01, 0x01, 0x11, 0x00,
        ]);
        assert_eq!(jpeg_size(&j), Some((320, 240)));
        assert_eq!(jpeg_size(b"RIFF"), None);
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xD9]), None);
    }

    #[test]
    fn pattern_camera_delivers_jpegs_of_the_configured_size() {
        let mut board = HostBoard::new().unpaced();
        assert_eq!(board.cam_grab().unwrap_err(), Error::NotBegun("camera"));
        board.cam_begin(&cam::Config::qvga_jpeg()).unwrap();
        let frame = board.cam_grab().unwrap().unwrap();
        assert_eq!((frame.width, frame.height), (320, 240));
        assert_eq!(jpeg_size(&frame.bytes), Some((320, 240)));
        assert!(frame.bytes.len() > 1000 && frame.bytes.len() < 100_000);
    }

    #[test]
    fn sine_microphone_fills_blocks_of_the_configured_length() {
        let mut board = HostBoard::new().unpaced();
        assert_eq!(board.mic_read().unwrap_err(), Error::NotBegun("microphone"));
        let config = mic::Config::pcm16_16k();
        board.mic_begin(&config).unwrap();
        let block = board.mic_read().unwrap().unwrap();
        assert_eq!(block.bytes.len(), config.block_bytes());
        assert_eq!(block.frames(), 320);
        let peak = block
            .bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
            .max()
            .unwrap();
        assert!(peak > 7000 && peak <= 8000, "peak {peak}");
    }

    #[test]
    fn wav_microphone_loops_the_file_in_its_own_format() {
        let format = PcmFormat::new(8000, 1, SampleFormat::I16).unwrap();
        let data: Vec<u8> = (0..100i16).flat_map(i16::to_le_bytes).collect();
        let header = WavHeader::pcm(format, data.len() as u32);
        let mut file = vec![0u8; 64];
        let n = header.write(&mut file).unwrap();
        file.truncate(n);
        file.extend_from_slice(&data);
        let dir = std::env::temp_dir().join(format!("janus-arduino-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        std::fs::write(&path, &file).unwrap();
        let mut board = HostBoard::new()
            .with_microphone(Microphone::Wav(path))
            .unpaced();
        board
            .mic_begin(&mic::Config::pcm16_16k().blocks_of_ms(30))
            .unwrap();
        let block = board.mic_read().unwrap().unwrap();
        assert_eq!(block.format, format, "the file's format wins");
        assert_eq!(block.bytes.len(), 480, "30 ms at 8 kHz mono i16");
        assert_eq!(&block.bytes[..200], &data[..], "then it wraps");
        assert_eq!(&block.bytes[200..400], &data[..]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn default_store() -> PathBuf {
    std::env::temp_dir().join("janus-host-kv")
}

/// A [`Kv`] over a directory: one file per key, the value its bytes. The
/// laptop's stand-in for NVS — nothing here is encrypted, which is why the
/// default directory is a temp one and a firmware never uses this type.
#[derive(Debug, Clone)]
pub struct FileKv {
    dir: PathBuf,
}

impl FileKv {
    /// Open (creating) `dir`.
    pub fn open(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(FileKv {
            dir: dir.to_path_buf(),
        })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }
}

impl Kv for FileKv {
    fn get(&self, key: &str, out: &mut [u8]) -> CoreResult<Option<usize>> {
        check_key(key)?;
        match std::fs::read(self.path(key)) {
            Ok(value) => {
                if out.len() < value.len() {
                    return Err(CoreError::BufferTooSmall {
                        needed: value.len(),
                    });
                }
                out[..value.len()].copy_from_slice(&value);
                Ok(Some(value.len()))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(CoreError::Hardware),
        }
    }

    fn put(&mut self, key: &str, value: &[u8]) -> CoreResult<()> {
        check_key(key)?;
        std::fs::write(self.path(key), value).map_err(|_| CoreError::Hardware)
    }

    fn remove(&mut self, key: &str) -> CoreResult<bool> {
        check_key(key)?;
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(CoreError::Hardware),
        }
    }
}

/// The operating system's random source as the family's [`Rng`] seam.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostRng;

impl Rng for HostRng {
    fn fill(&mut self, buf: &mut [u8]) -> CoreResult<()> {
        rand::RngCore::fill_bytes(&mut rand::rng(), buf);
        Ok(())
    }
}
