//! The seam: one trait a board implements, installed once per program.
//!
//! A board owns the peripherals the sketch functions reach — the camera, the
//! microphone, the network interface — and nothing else. The stream side
//! (HTTP, RTP, UDP) is not the board's: it is `rusty_esp_video-esp`'s and is
//! the same code on every board, which is why `stream` is a module and not a
//! trait method.

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use rusty_esp_core::hal::{Kv, Rng};
use rusty_esp_core::pcm::PcmFormat;
use rusty_esp_core::time::Micros;

use crate::error::{Error, Result};
use crate::radar::Presence;
use crate::{cam, mic};

/// One JPEG frame from the camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jpeg {
    /// The whole JFIF file, SOI to EOI.
    pub bytes: Vec<u8>,
    /// Pixels across.
    pub width: u16,
    /// Pixels down.
    pub height: u16,
    /// When it was captured, in the board's clock.
    pub timestamp: Micros,
}

/// One block of PCM from the microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    /// Rate, channels and sample format of `bytes`.
    pub format: PcmFormat,
    /// Interleaved little-endian samples.
    pub bytes: Vec<u8>,
    /// When the first sample was captured, in the board's clock.
    pub timestamp: Micros,
}

impl Pcm {
    /// Sample frames in the block.
    #[must_use]
    pub fn frames(&self) -> usize {
        let frame = self.format.sample.bytes() * usize::from(self.format.channels);
        self.bytes.len().checked_div(frame).unwrap_or(0)
    }
}

/// What a board provides. Every method may be called before the matching
/// `begin`; a board answers `Err(Error::NotBegun(..))` then.
pub trait Board: Send {
    /// Join the network `ssid` with `psk`; return when joined or refused.
    fn wifi_begin(&mut self, ssid: &str, psk: &str) -> Result<()>;
    /// The interface address, once joined or once hosting.
    fn local_ip(&self) -> Option<IpAddr>;
    /// Configure and start the camera.
    fn cam_begin(&mut self, config: &cam::Config) -> Result<()>;
    /// The next frame, or `None` when no frame is ready yet.
    fn cam_grab(&mut self) -> Result<Option<Jpeg>>;
    /// Configure and start the microphone.
    fn mic_begin(&mut self, config: &mic::Config) -> Result<()>;
    /// The next block, or `None` when no block is ready yet.
    fn mic_read(&mut self) -> Result<Option<Pcm>>;
    /// Milliseconds since the board started.
    fn millis(&self) -> u64;

    /// Host an access point called `ssid` with `psk`, instead of joining
    /// one. A device with no network in reach is still a device someone
    /// needs to reach: this is how a camera in a shed, or a board on a
    /// bench with no router, is viewed and provisioned.
    ///
    /// Provided, and refusing by default, so a board that has no radio says
    /// so rather than appearing to succeed. `psk` must be 8 to 63 bytes:
    /// WPA2 has no shorter key, and an open access point is not offered
    /// here because a device that serves its camera should not serve it to
    /// the street.
    ///
    /// After this returns `Ok`, [`Board::local_ip`] is the address the
    /// board answers on, and clients reach it over the network it now runs.
    fn wifi_host(&mut self, _ssid: &str, _psk: &str) -> Result<()> {
        Err(Error::Missing("access-point mode"))
    }

    /// The board's persistent store — NVS on a chip, a directory on the
    /// host — for the device's keys and its adoption. Taken once; `None`
    /// when the board has none, and `identity::begin` says so.
    fn take_kv(&mut self) -> Option<Box<dyn Kv + Send>> {
        None
    }

    /// Publish this device on the local network so something can find it
    /// without being told a ticket first. `service_type` is the full mDNS
    /// type (`_mata-oem-sidecar._tcp.local.`), `txt` the record the home
    /// computer's pair client reads back, and the board keeps whatever
    /// handle its platform needs alive. A board with no mDNS — the laptop,
    /// a chip whose firmware did not compile it in — answers `Ok` and is
    /// reachable by ticket alone, which is what every generated mesh cell
    /// was until 2026-09-11.
    fn advertise(
        &mut self,
        _hostname: &str,
        _instance: &str,
        _service_type: &str,
        _port: u16,
        _txt: &[(String, String)],
    ) -> Result<()> {
        Ok(())
    }

    /// Prepare the platform for the async runtime [`mesh::begin`] is about
    /// to build, and do it once. On ESP-IDF tokio's I/O driver opens an
    /// `eventfd`, which is served by a VFS that must be registered first:
    /// without it `eventfd()` answers `EACCES` and every runtime fails to
    /// build with `Permission denied` — C2's first boot on the XIAO,
    /// 2026-09-11. `max_fds` is how many the driver may hold at once. A
    /// board with nothing to prepare — the laptop, anything that is not
    /// ESP-IDF — answers `Ok`, which is the default here.
    ///
    /// [`mesh::begin`]: crate::mesh::begin
    fn prepare_async(&mut self, _max_fds: usize) -> Result<()> {
        Ok(())
    }

    /// The board's entropy — the chip's TRNG, the OS on the host. Taken once.
    fn take_rng(&mut self) -> Option<Box<dyn Rng + Send>> {
        None
    }

    /// Start advertising the Janus provisioning service as `name` (BLE on a
    /// chip); a phone writes the network to join. A board without a radio
    /// for it answers `Err(Error::Missing(..))`, and `provision::begin` says
    /// so.
    fn provision_begin(&mut self, _name: &str) -> Result<()> {
        Err(Error::Missing("BLE provisioning on this board"))
    }

    /// The `(ssid, psk)` a phone wrote since the last call, if any.
    fn provision_poll(&mut self) -> Result<Option<(String, String)>> {
        Ok(None)
    }

    /// Tell the phone how the join went; a board without provisioning
    /// ignores it.
    fn provision_report(&mut self, _joined: bool) -> Result<()> {
        Ok(())
    }

    /// Keep the network in the owner's settings (the `nvs` partition on a
    /// chip) so the next boot joins without a phone.
    fn store_settings(&mut self, _ssid: &str, _psk: &str) -> Result<()> {
        Err(Error::Missing("a settings store on this board"))
    }

    /// Restart the board. On a chip this never returns; a board that cannot
    /// restart itself answers `Err(Error::Missing(..))` and the sketch
    /// carries on.
    fn restart(&mut self) -> Result<()> {
        Err(Error::Missing("a way to restart this board"))
    }

    /// Start the presence sensor. A board without one answers
    /// `Err(Error::Missing(..))` and `radar::begin` says so.
    fn radar_begin(&mut self, _config: &crate::radar::Config) -> Result<()> {
        Err(Error::Missing("a presence sensor on this board"))
    }

    /// The newest reading since the last call, or `None` when none is ready.
    fn radar_read(&mut self) -> Result<Option<Presence>> {
        Ok(None)
    }
}

static BOARD: Mutex<Option<Box<dyn Board>>> = Mutex::new(None);

/// Install the board the sketch runs on. Call once, first thing in `setup`.
/// Installing again replaces the previous board.
pub fn install(board: impl Board + 'static) {
    *BOARD.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(board));
}

/// Remove the installed board (tests, and a sketch that ends).
pub fn uninstall() {
    *BOARD.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether a board is installed.
#[must_use]
pub fn installed() -> bool {
    BOARD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// Run `f` against the installed board.
pub(crate) fn with<R>(f: impl FnOnce(&mut dyn Board) -> Result<R>) -> Result<R> {
    let mut guard = BOARD.lock().unwrap_or_else(PoisonError::into_inner);
    match guard.as_mut() {
        Some(board) => f(board.as_mut()),
        None => Err(Error::NoBoard),
    }
}

/// Milliseconds since the board started (0 without a board).
#[must_use]
pub fn millis() -> u64 {
    with(|b| Ok(b.millis())).unwrap_or(0)
}
