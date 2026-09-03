//! Where frames and blocks go: an MJPEG page any browser opens, RTP/JPEG to
//! a player, raw PCM over UDP to a receiver — all of it `rusty_esp_video-esp`,
//! all of it the same code on the laptop and on the chip.
//!
//! `listen(port)` serves `/` and `/stream`; `push_jpeg` hands the latest
//! frame to every open stream. `rtp_to(dest)` and `pcm_to(dest)` add the two
//! UDP senders; `push_jpeg` and `push_pcm` then also send. Nothing is
//! configured by default: a sketch that never calls `listen` opens no socket.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use rusty_esp_core::error::{Error as CoreError, Result as CoreResult};
use rusty_esp_core::media::{Codec, MediaPacket};
use rusty_esp_video_core::source::PacketSource;
use rusty_esp_video_esp::net::{MjpegHttpServer, ServeStats};
use rusty_esp_video_esp::udp_net::{RawUdpSender, RtpJpegSender, TxStats};

use crate::board::{Jpeg, Pcm};
use crate::error::{self, Error, Result};

/// RTP/JPEG and PCM datagrams are cut to fit this many bytes.
pub const MTU: usize = 1400;
/// The largest JPEG a stream will carry (QVGA to 1080p at quality 80 fit
/// with room to spare).
pub const MAX_JPEG: usize = 512 * 1024;
/// The RTP SSRC the sender uses; one sender per program.
pub const SSRC: u32 = 0x4A41_4E55; // "JANU"

/// What the HTTP server has served.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HttpStats {
    /// Connections accepted.
    pub connections: u64,
    /// `/stream` requests served to the end.
    pub streams: u64,
    /// Frames pushed across all streams.
    pub frames: u64,
    /// Other requests answered (index, 404, 405).
    pub other: u64,
}

/// What the stream module has done so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// `push_jpeg` calls that succeeded.
    pub jpeg_pushed: u64,
    /// `push_pcm` calls that succeeded.
    pub pcm_pushed: u64,
    /// The RTP/JPEG sender's totals (zeros when none).
    pub rtp: TxStats,
    /// The PCM sender's totals (zeros when none).
    pub pcm: TxStats,
    /// The HTTP server's totals (zeros when none).
    pub http: HttpStats,
}

/// The latest frame, shared between `push_jpeg` and every open `/stream`.
#[derive(Default)]
struct Slot {
    frame: Mutex<(u64, Option<Jpeg>)>,
    fresh: Condvar,
}

impl Slot {
    fn publish(&self, frame: &Jpeg) {
        let mut guard = self.frame.lock().unwrap_or_else(PoisonError::into_inner);
        guard.0 += 1;
        guard.1 = Some(frame.clone());
        self.fresh.notify_all();
    }
}

/// One open `/stream`: waits for a frame newer than the last it sent.
struct SlotSource {
    slot: Arc<Slot>,
    last: u64,
}

impl PacketSource for SlotSource {
    fn next_packet<'b>(&mut self, scratch: &'b mut [u8]) -> CoreResult<MediaPacket<'b>> {
        let mut guard = self
            .slot
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while guard.0 == self.last || guard.1.is_none() {
            guard = self
                .slot
                .fresh
                .wait_timeout(guard, Duration::from_secs(1))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        self.last = guard.0;
        let frame = guard.1.as_ref().ok_or(CoreError::Hardware)?;
        let n = frame.bytes.len();
        if scratch.len() < n {
            return Err(CoreError::BufferTooSmall { needed: n });
        }
        scratch[..n].copy_from_slice(&frame.bytes);
        let timestamp = frame.timestamp;
        drop(guard);
        Ok(MediaPacket::new(
            Codec::Jpeg,
            true,
            timestamp,
            &scratch[..n],
        ))
    }
}

struct Server {
    addr: SocketAddr,
    slot: Arc<Slot>,
    stats: Arc<Mutex<HttpStats>>,
}

struct State {
    server: Option<Server>,
    rtp: Option<RtpJpegSender>,
    pcm: Option<RawUdpSender>,
    jpeg_pushed: u64,
    pcm_pushed: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    server: None,
    rtp: None,
    pcm: None,
    jpeg_pushed: 0,
    pcm_pushed: 0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serve `/` and `/stream` on every interface at `port` (0 picks one; see
/// [`listen_addr`]). One server per program; calling again replaces the
/// address frames go to but the first server keeps running. `false` (and
/// [`crate::last_error`]) when the port cannot be bound.
pub fn listen(port: u16) -> bool {
    error::ok(listen_inner(port))
}

fn listen_inner(port: u16) -> Result<()> {
    let server = MjpegHttpServer::bind(("0.0.0.0", port))?;
    let addr = server.local_addr()?;
    let slot = Arc::new(Slot::default());
    let stats = Arc::new(Mutex::new(HttpStats::default()));
    let (thread_slot, thread_stats) = (Arc::clone(&slot), Arc::clone(&stats));
    thread::Builder::new()
        .name(format!("mjpeg-{port}"))
        // ESP-IDF's default pthread stack is a few KB; the server parses
        // request heads and writes frames from this thread. Harmless on a
        // laptop.
        .stack_size(24 * 1024)
        .spawn(move || {
            let mut scratch = vec![0u8; MAX_JPEG];
            let mut served = ServeStats::default();
            loop {
                let mut source = SlotSource {
                    slot: Arc::clone(&thread_slot),
                    last: 0,
                };
                // A client disconnect is not an error; a source error would
                // be, and the slot source has none — so this loops forever.
                let _ = server.serve_one(&mut source, &mut scratch, &mut served);
                *thread_stats.lock().unwrap_or_else(PoisonError::into_inner) = HttpStats {
                    connections: served.connections,
                    streams: served.streams,
                    frames: served.frames,
                    other: served.other,
                };
            }
        })?;
    state().server = Some(Server { addr, slot, stats });
    Ok(())
}

/// Where [`listen`] is serving, once it is.
#[must_use]
pub fn listen_addr() -> Option<SocketAddr> {
    state().server.as_ref().map(|s| s.addr)
}

/// Also send every pushed frame as RTP/JPEG (RFC 2435) to `dest`. `false`
/// (and [`crate::last_error`]) when the socket cannot be made.
pub fn rtp_to(dest: impl ToSocketAddrs) -> bool {
    error::ok((|| {
        let sender = RtpJpegSender::bind("0.0.0.0:0", dest, SSRC, MTU)?;
        state().rtp = Some(sender);
        Ok::<(), Error>(())
    })())
}

/// Also send every pushed block as a Janus media packet over UDP to `dest`.
/// `false` (and [`crate::last_error`]) when the socket cannot be made.
pub fn pcm_to(dest: impl ToSocketAddrs) -> bool {
    error::ok((|| {
        let sender = RawUdpSender::bind("0.0.0.0:0", dest, MTU)?;
        state().pcm = Some(sender);
        Ok::<(), Error>(())
    })())
}

/// Hand a frame to every open `/stream` and to the RTP sender, if any.
/// `true` when at least one destination exists and took it.
pub fn push_jpeg(frame: &Jpeg) -> bool {
    error::ok((|| {
        let mut st = state();
        let mut took = false;
        if let Some(server) = st.server.as_ref() {
            server.slot.publish(frame);
            took = true;
        }
        if let Some(rtp) = st.rtp.as_mut() {
            rtp.send_frame(&frame.bytes, frame.timestamp)?;
            took = true;
        }
        if took {
            st.jpeg_pushed += 1;
            Ok(())
        } else {
            Err(Error::NotBegun("stream"))
        }
    })())
}

/// Hand a block to the PCM sender. `true` when one exists and took it.
pub fn push_pcm(pcm: &Pcm) -> bool {
    error::ok((|| {
        let mut st = state();
        let Some(sender) = st.pcm.as_mut() else {
            return Err(Error::NotBegun("stream::pcm_to"));
        };
        let packet = MediaPacket::new(Codec::Pcm(pcm.format), true, pcm.timestamp, &pcm.bytes);
        sender.send_packet(&packet)?;
        st.pcm_pushed += 1;
        Ok(())
    })())
}

/// Totals so far.
#[must_use]
pub fn stats() -> Stats {
    let st = state();
    Stats {
        jpeg_pushed: st.jpeg_pushed,
        pcm_pushed: st.pcm_pushed,
        rtp: st.rtp.as_ref().map(|s| s.stats).unwrap_or_default(),
        pcm: st.pcm.as_ref().map(|s| s.stats).unwrap_or_default(),
        http: st
            .server
            .as_ref()
            .map(|s| *s.stats.lock().unwrap_or_else(PoisonError::into_inner))
            .unwrap_or_default(),
    }
}

/// Drop the RTP and PCM senders (the HTTP server, once started, stays).
pub fn stop_senders() {
    let mut st = state();
    st.rtp = None;
    st.pcm = None;
}
