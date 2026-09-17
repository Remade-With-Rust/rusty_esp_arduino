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

/// Why a boot happened, as the board's reset logic names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    /// Power applied.
    PowerOn,
    /// The reset pin, or a USB reset.
    External,
    /// A software restart the sketch asked for.
    Software,
    /// The panic handler.
    Panic,
    /// The task watchdog: a loop pass that never came back.
    TaskWatchdog,
    /// The interrupt watchdog, or another hardware watchdog.
    InterruptWatchdog,
    /// The supply sagged.
    Brownout,
    /// A wake from deep sleep.
    DeepSleep,
    /// Something this list does not name.
    Other,
}

impl ResetReason {
    /// The word a record carries.
    #[must_use]
    pub const fn wire_tag(self) -> &'static str {
        match self {
            ResetReason::PowerOn => "power-on",
            ResetReason::External => "external",
            ResetReason::Software => "software",
            ResetReason::Panic => "panic",
            ResetReason::TaskWatchdog => "task-watchdog",
            ResetReason::InterruptWatchdog => "interrupt-watchdog",
            ResetReason::Brownout => "brownout",
            ResetReason::DeepSleep => "deep-sleep",
            ResetReason::Other => "other",
        }
    }

    /// Whether this boot followed a failure rather than a request.
    #[must_use]
    pub const fn is_crash(self) -> bool {
        matches!(
            self,
            ResetReason::Panic
                | ResetReason::TaskWatchdog
                | ResetReason::InterruptWatchdog
                | ResetReason::Brownout
        )
    }
}

/// What a board knows about this boot and the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootRecord {
    /// Why this boot happened.
    pub reason: ResetReason,
    /// Boots counted by the board, this one included.
    pub boots: u32,
    /// Boots that followed a failure, this one included if it did.
    pub crashes: u32,
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

    /// Replace the TXT record of the service [`Board::advertise`] published,
    /// because something in it changed -- adoption, above all: a device that
    /// has an owner must stop advertising itself as free to claim, and until
    /// 2026-09-12 it did not, because the record was composed once at boot.
    /// A board that never advertised has nothing to update and answers `Ok`.
    fn advertise_txt(&mut self, _service_type: &str, _txt: &[(String, String)]) -> Result<()> {
        Ok(())
    }

    /// The inactive slot a signed update is written to, if this board has
    /// one: `esp-ota`'s next update partition on a chip, two buffers on the
    /// laptop. `None` is a board with a single app partition, and the
    /// manifest then does not promise `ota`. Asked only when the device has
    /// a maker to trust, because without one no image can be accepted.
    #[cfg(feature = "mesh")]
    fn ota_sink(&mut self) -> Option<Box<dyn rusty_esp_iroh_core::ota::OtaSink + Send>> {
        None
    }

    /// The running image is good: cancel a pending bootloader rollback.
    /// Called once the endpoint is up, so that an image which boots but
    /// never reaches the mesh is the one the bootloader drops.
    fn ota_running_valid(&mut self) -> Result<()> {
        Ok(())
    }

    /// Put the sketch's thread on a liveness watchdog: if [`Board::liveness_feed`]
    /// is not called within about `timeout_s` seconds, the board must reboot
    /// itself. `sketch::run` arms this before `setup` and feeds it after every
    /// pass. A board without one answers `Ok` and the loop runs unwatched, as
    /// the XIAO did on 2026-09-16 when a microphone hang sat silent for 73 s.
    fn liveness_begin(&mut self, _timeout_s: u32) -> Result<()> {
        Ok(())
    }

    /// One pass of the loop completed.
    fn liveness_feed(&mut self) -> Result<()> {
        Ok(())
    }

    /// Why this boot happened and how many boots and crashes the board has
    /// counted, from its own store; read once at the start of `run`. `None`
    /// on a board that keeps no such record.
    fn boot_record(&mut self) -> Result<Option<BootRecord>> {
        Ok(None)
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
