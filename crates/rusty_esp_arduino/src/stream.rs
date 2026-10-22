//! Where frames and blocks go: an MJPEG page any browser opens, RTP/JPEG to
//! a player, raw PCM over UDP to a receiver — all of it `rusty_esp_video-esp`,
//! all of it the same code on the laptop and on the chip.
//!
//! `listen_gated(port)` serves `/` and `/stream` to whoever has the device's
//! page token (the URL printed at boot); `listen(port)` serves them to
//! anyone, by name. `push_jpeg` hands the latest frame to every open stream. `rtp_to(dest)` and `pcm_to(dest)` add the two
//! UDP senders; `push_jpeg` and `push_pcm` then also send. Nothing is
//! configured by default: a sketch that never calls `listen` opens no socket.
//! A board with no camera that a home's hub still has to find calls
//! `listen_status(port)`: `/v1/status` alone.

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use rusty_esp_core::error::{Error as CoreError, Result as CoreResult};
use rusty_esp_core::media::{Codec, MediaPacket};
use rusty_esp_video_core::http::{self, Path, Request, StatusQuery};
use rusty_esp_video_core::source::PacketSource;
use rusty_esp_video_esp::net::{
    Accepted, MAX_REQUEST_BYTES, MjpegHttpServer, ServeStats, StatusHandler, StreamConn, TcpSink,
};
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
/// How long an open `/stream` waits for a frame before the connection is
/// given up. The server serves one connection at a time, and a viewer that
/// arrived while the camera had nothing (it failed to probe, say) would
/// otherwise hold that one connection forever: the stream only notices a
/// gone client when it next pushes a frame, and no frame ever comes. Seen
/// on the board — every request after such a viewer queued unanswered.
pub const STREAM_STALL: Duration = Duration::from_secs(5);

/// The HTTP server thread's stack. ESP-IDF's default pthread stack is a few
/// KB; the server parses request heads and writes frames from this thread.
const SERVER_STACK: usize = 24 * 1024;

/// What a status handler adds to it: the handler signs (P-256) and builds
/// JSON on the server's thread. Measured by painting the stack (x86_64,
/// 2026-10-18): 5.7 KiB optimised, 21.4 KiB unoptimised, which overflowed
/// the 24 KiB thread on Linux CI. Windows rounds a thread's stack up to
/// 64 KiB and had hidden it.
const STATUS_STACK: usize = if cfg!(debug_assertions) {
    48 * 1024
} else {
    16 * 1024
};

/// How long [`listen_status`] waits for a request head before it gives the
/// connection up: one thread answers every hub, so a client that connects
/// and says nothing must not hold the next one for long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

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
        let started = std::time::Instant::now();
        while guard.0 == self.last || guard.1.is_none() {
            if started.elapsed() >= STREAM_STALL {
                // ends this connection; the server accepts the next
                return Err(CoreError::Timeout);
            }
            guard = self
                .slot
                .fresh
                .wait_timeout(guard, Duration::from_millis(250))
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
    /// The viewing token the server requires, when gated.
    token: Option<String>,
}

struct State {
    server: Option<Server>,
    rtp: Option<RtpJpegSender>,
    pcm: Option<RawUdpSender>,
    /// What `/v1/status` serves, for the next `listen`.
    status: Option<StatusHandler>,
    jpeg_pushed: u64,
    pcm_pushed: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    server: None,
    rtp: None,
    pcm: None,
    status: None,
    jpeg_pushed: 0,
    pcm_pushed: 0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serve `/` and `/stream` on every interface at `port` (0 picks one; see
/// [`listen_addr`]) **to anyone who finds the address** — the open page, by
/// name; a sketch that wants the token on it calls [`listen_gated`]. One
/// server per program; calling again replaces the address frames go to but
/// the first server keeps running. `false` (and [`crate::last_error`]) when
/// the port cannot be bound.
pub fn listen(port: u16) -> bool {
    error::ok(listen_inner(port, None))
}

/// Serve `/` and `/stream` at `port` to whoever has the device's page token:
/// `?t=<token>` on the URL (the page then sets it as a cookie for its own
/// `/stream`), `403` otherwise. The token is the identity's
/// ([`crate::identity::page_token`]), so `identity::begin` comes first;
/// without it this is `false` and [`crate::last_error`] says so. See
/// [`page_url`] for the line to print.
pub fn listen_gated(port: u16) -> bool {
    error::ok(
        crate::identity::page_token()
            .ok_or(Error::Missing(
                "the page token: identity::begin comes before listen_gated",
            ))
            .and_then(|token| listen_inner(port, Some(token))),
    )
}

/// Serve `GET /v1/status` from `handler` on the server the next [`listen`]
/// or [`listen_gated`] starts, ungated (a home's hub has no page token):
/// `handler` gets `None` for the plain status and the verifier's 32-byte
/// nonce for `?nonce=<64 lowercase hex>`, and returns the JSON body. Any
/// other query is a `400`; without a handler the path is a `404`. So this
/// comes before `listen`.
pub fn on_status(handler: impl Fn(Option<[u8; 32]>) -> String + Send + Sync + 'static) {
    state().status = Some(Arc::new(handler));
}

/// The token the running server requires, if it is gated.
#[must_use]
pub fn token() -> Option<String> {
    state().server.as_ref().and_then(|s| s.token.clone())
}

/// The URL to open at `host` (an address or a name): `http://host/?t=…`
/// when gated, `http://host/` when open, with the port when it is not 80.
/// `None` before [`listen`] or [`listen_gated`].
#[must_use]
pub fn page_url(host: &str) -> Option<String> {
    let st = state();
    let server = st.server.as_ref()?;
    let port = if server.addr.port() == 80 {
        String::new()
    } else {
        format!(":{}", server.addr.port())
    };
    Some(match &server.token {
        Some(t) => format!("http://{host}{port}/?t={t}"),
        None => format!("http://{host}{port}/"),
    })
}

fn listen_inner(port: u16, token: Option<String>) -> Result<()> {
    let mut server = MjpegHttpServer::bind(("0.0.0.0", port))?;
    if let Some(t) = &token {
        server = server.gated(t.clone());
    }
    let status = state().status.clone();
    let mut stack = SERVER_STACK;
    if let Some(handler) = status {
        server = server.with_status(handler);
        stack += STATUS_STACK;
    }
    let addr = server.local_addr()?;
    let slot = Arc::new(Slot::default());
    let stats = Arc::new(Mutex::new(HttpStats::default()));

    // Streams play on a thread of their own, one at a time as they always
    // did; the accepting thread answers everything else. Until 2026-10-18
    // one thread did both, and a viewer with the page open held up
    // `/v1/status` until the tab closed -- long enough for a hub polling it
    // to call the camera offline (Apollo, on the XIAO).
    let (streams, queued) = mpsc::channel::<StreamConn>();
    let (worker_slot, worker_stats) = (Arc::clone(&slot), Arc::clone(&stats));
    thread::Builder::new()
        .name(format!("mjpeg-{port}-stream"))
        .stack_size(SERVER_STACK)
        .spawn(move || {
            let mut scratch = vec![0u8; MAX_JPEG];
            for conn in queued {
                let mut source = SlotSource {
                    slot: Arc::clone(&worker_slot),
                    last: 0,
                };
                let mut played = ServeStats::default();
                // A viewer leaving is not an error, and the slot source has
                // none to return.
                let _ = conn.run(&mut source, &mut scratch, &mut played);
                let mut s = worker_stats.lock().unwrap_or_else(PoisonError::into_inner);
                s.streams += played.streams;
                s.frames += played.frames;
            }
        })?;

    let accept_stats = Arc::clone(&stats);
    thread::Builder::new()
        .name(format!("mjpeg-{port}"))
        .stack_size(stack)
        .spawn(move || {
            let mut counted = ServeStats::default();
            loop {
                match server.accept_one(&mut counted) {
                    // the worker outlives this thread; a failed send means
                    // the program is ending
                    Ok(Accepted::Stream(conn)) => {
                        let _ = streams.send(conn);
                    }
                    Ok(Accepted::Answered(_)) => {}
                    // the listener refused an accept: do not spin on it
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
                let mut s = accept_stats.lock().unwrap_or_else(PoisonError::into_inner);
                s.connections = counted.connections;
                s.other = counted.other;
            }
        })?;
    state().server = Some(Server {
        addr,
        slot,
        stats,
        token,
    });
    Ok(())
}

/// Serve `GET /v1/status` alone at `port` (0 picks one; see
/// [`listen_addr`]), from the handler [`on_status`] set: a board with no
/// camera that a home's hub still has to find, check and adopt. Every other
/// path is a `404` and every other method a `405`; there is no page and no
/// token. So [`on_status`] comes first, and without it this is `false` and
/// [`crate::last_error`] says so. One server per program, as with
/// [`listen`].
pub fn listen_status(port: u16) -> bool {
    error::ok(listen_status_inner(port))
}

fn listen_status_inner(port: u16) -> Result<()> {
    let handler = state().status.clone().ok_or(Error::Missing(
        "a status to serve: on_status comes before listen_status",
    ))?;
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let addr = listener.local_addr()?;
    let stats = Arc::new(Mutex::new(HttpStats::default()));
    let counted = Arc::clone(&stats);
    thread::Builder::new()
        .name(format!("status-{port}"))
        .stack_size(SERVER_STACK + STATUS_STACK)
        .spawn(move || {
            loop {
                let Ok((conn, _)) = listener.accept() else {
                    // the listener refused an accept: do not spin on it
                    thread::sleep(Duration::from_millis(50));
                    continue;
                };
                answer_status(conn, &handler);
                let mut s = counted.lock().unwrap_or_else(PoisonError::into_inner);
                s.connections += 1;
                s.other += 1;
            }
        })?;
    state().server = Some(Server {
        addr,
        slot: Arc::default(),
        stats,
        token: None,
    });
    Ok(())
}

/// One request on a status-only server, answered and closed.
fn answer_status(conn: TcpStream, handler: &StatusHandler) {
    let _ = conn.set_nodelay(true);
    let _ = conn.set_read_timeout(Some(REQUEST_TIMEOUT));
    let mut sink = TcpSink(conn);
    let mut head = [0u8; MAX_REQUEST_BYTES];
    let mut n = 0;
    while n < head.len() && !http::head_complete(&head[..n]) {
        match sink.0.read(&mut head[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    let _ = match http::parse_request(&head[..n], None) {
        None => http::write_status(&mut sink, http::Status::BadRequest),
        Some(Request::Get {
            path: Path::Status(StatusQuery::Invalid),
            ..
        }) => http::write_status(&mut sink, http::Status::BadRequest),
        Some(Request::Get {
            path: Path::Status(query),
            ..
        }) => {
            let nonce = match query {
                StatusQuery::Nonce(n) => Some(n),
                _ => None,
            };
            http::write_json(&mut sink, 200, handler(nonce).as_bytes())
        }
        Some(Request::Get { .. }) => http::write_status(&mut sink, http::Status::NotFound),
        Some(Request::Put { .. } | Request::Post { .. } | Request::Other) => {
            http::write_status(&mut sink, http::Status::MethodNotAllowed)
        }
    };
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
