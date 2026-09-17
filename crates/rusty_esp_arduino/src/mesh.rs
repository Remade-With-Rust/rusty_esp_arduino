//! The mesh behind the facade: `rusty_esp_iroh`'s node in a background
//! thread, fed by `push_media` — the device as a MATA node, in the sketch's
//! three verbs. Thin by law: the node, its ALPNs, adoption, the sidecar
//! contract and OTA are the iroh package's; this module owns a thread, two
//! frame slots and a subscriber factory, nothing more.
//!
//! `identity::begin` must have run: the node presents that key, from the
//! same store, and is the same `did:mata`.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use rusty_esp_core::capability::{Capability, Chip, Declared, Manifest};
use rusty_esp_iroh_core::media::{FLAG_KEY, PacketHeader, Subscribe};
pub use rusty_esp_iroh_core::ota::OtaSink;
use rusty_esp_iroh_host::mjpeg::CODEC_MJPEG;
use rusty_esp_iroh_host::{Extras, MediaSource, Node, NodeConfig, NodeIdentity};

use crate::board::{self, Jpeg, Pcm};
use crate::error::{Error, Result, record};
use crate::identity::{self, DynKv, DynRng};

/// How many `eventfd`s tokio's I/O driver may hold at once. The driver opens
/// one per runtime; 5 is what n0 uses, and the board registers the VFS that
/// serves them in [`Board::prepare_async`](crate::board::Board::prepare_async).
const ASYNC_MAX_FDS: usize = 5;

/// The mesh thread's stack on ESP-IDF, where the default pthread stack is
/// 8 KiB and tokio's current-thread runtime runs the whole node on it — the
/// QUIC handshake, rustls, the resolver. J3's hand-written firmware measured
/// n0's DNS path at ~111 KiB and gives its runtime 114,688 bytes
/// (rusty_esp_iroh/docs/LEDGER.md); the same number, asked for where the
/// thread is made rather than in one board's sdkconfig, so every board that
/// runs the mesh gets it.
#[cfg(target_os = "espidf")]
const MESH_STACK_BYTES: Option<usize> = Some(114_688);

/// Everywhere else the platform's own default is already far larger — 2 MiB
/// on Windows and Linux — and a debug build of the node needs it: 114,688
/// overflowed the thread on the laptop. Ask for nothing and keep the default.
#[cfg(not(target_os = "espidf"))]
const MESH_STACK_BYTES: Option<usize> = None;

/// The codec tag PCM blocks carry on `janus/media/1` (the JPEG tag is the
/// iroh package's `CODEC_MJPEG`).
pub const CODEC_PCM: [u8; 4] = *b"pcm ";

/// The media codec tag of a telemetry stream: the payload is opaque to
/// the transport (a `rusty_esp_signal_core::radar::presence::Presence`
/// today), the way the bridge already carries a neighbour's telemetry.
pub const CODEC_TELEMETRY: [u8; 4] = *b"tlm ";

/// What the node advertises: the capability manifest the home computer
/// catalogs, signed by the device key.
#[derive(Debug, Clone)]
pub struct Config {
    /// `model=` for the sidecar advertisement, e.g. `janus/porch-cam`.
    pub model: String,
    /// The firmware identifier in the manifest.
    pub firmware: String,
    /// The chip the manifest names.
    pub chip: Chip,
    /// Every capability, with its status and backing crate.
    pub declared: Vec<Declared>,
    /// Relay and pkarr discovery (the PSRAM tier); off is LAN-direct.
    pub relay: bool,
    /// Media subscribers served at once; one more is refused and counted.
    /// `0` is no cap. A subscription costs 17,000-19,088 B of internal RAM
    /// on the XIAO ESP32-S3, which served two and panicked at four
    /// (2026-09-16); two is the ESP-IDF default, none is the laptop's.
    pub max_subscribers: u32,
}

impl Config {
    /// A LAN-direct node for `model` on `chip`, declaring nothing yet.
    #[must_use]
    pub fn new(model: &str, chip: Chip) -> Self {
        Config {
            model: model.to_owned(),
            firmware: format!("rusty_esp_arduino {}", env!("CARGO_PKG_VERSION")),
            chip,
            declared: Vec::new(),
            relay: false,
            max_subscribers: if cfg!(target_os = "espidf") { 2 } else { 0 },
        }
    }

    /// Add a declaration.
    #[must_use]
    pub fn declare(mut self, declared: Declared) -> Self {
        self.declared.push(declared);
        self
    }

    /// Ask for relay reachability.
    #[must_use]
    pub fn with_relay(mut self, relay: bool) -> Self {
        self.relay = relay;
        self
    }

    /// How many media subscribers to serve at once (`0`: no cap). Set it
    /// from what the board measured, not from what it should manage.
    #[must_use]
    pub fn with_max_subscribers(mut self, max: u32) -> Self {
        self.max_subscribers = max;
        self
    }
}

/// The latest frame of one codec, and how many have been pushed.
struct Slot {
    seq: u32,
    frame: Option<(u64, Arc<Vec<u8>>)>,
    closed: bool,
}

struct Channel {
    codec: [u8; 4],
    slot: Mutex<Slot>,
    cv: Condvar,
}

impl Channel {
    fn new(codec: [u8; 4]) -> Self {
        Channel {
            codec,
            slot: Mutex::new(Slot {
                seq: 0,
                frame: None,
                closed: false,
            }),
            cv: Condvar::new(),
        }
    }

    fn push(&self, timestamp_us: u64, bytes: &[u8]) {
        let mut g = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        g.seq = g.seq.wrapping_add(1);
        g.frame = Some((timestamp_us, Arc::new(bytes.to_vec())));
        self.cv.notify_all();
    }

    fn close(&self) {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.cv.notify_all();
    }
}

struct Shared {
    video: Channel,
    audio: Channel,
    telemetry: Channel,
    subscribers: AtomicU32,
    // 32-bit counters on purpose: a 32-bit RISC-V chip (the C6, the C3)
    // has no 64-bit atomic, and the mesh runs on those. `Stats` still
    // reports `u64`; these wrap after 4.29 billion, which is 13 years of
    // pushing at 10 a second.
    frames: AtomicU32,
    blocks: AtomicU32,
    readings: AtomicU32,
    /// The node's own counters, copied here every 100 ms by the mesh thread
    /// so the sketch can read them without holding the node.
    node_sent: AtomicU32,
    node_send_errors: AtomicU32,
    node_subscribers: AtomicU32,
    /// A TXT record the node wants re-advertised, set by the mesh thread when
    /// the owner pin changes and taken by `service()` on the sketch's thread,
    /// so the board is only ever touched from one thread.
    readvertise: Mutex<Option<Vec<(String, String)>>>,
    /// Whether the node was adopted at the last poll, to notice the change.
    adopted_seen: AtomicU32,
    /// The firmware string of an update the node accepted into the boot
    /// slot, copied out of the node by the mesh thread.
    last_ota: Mutex<Option<String>>,
}

/// Which channel a subscriber joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Video,
    Audio,
    Telemetry,
}

/// One subscriber's view of a channel: every frame pushed after it joined,
/// the newest when it fell behind, never a duplicate.
struct LatestFrames {
    shared: Arc<Shared>,
    which: Which,
    seen: u32,
    out_seq: u32,
}

impl MediaSource for LatestFrames {
    fn next_packet(&mut self) -> Option<(PacketHeader, Vec<u8>)> {
        let shared = Arc::clone(&self.shared);
        let ch = match self.which {
            Which::Video => &shared.video,
            Which::Audio => &shared.audio,
            Which::Telemetry => &shared.telemetry,
        };
        let mut g = ch.slot.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if g.closed {
                return None;
            }
            if g.seq != self.seen
                && let Some((timestamp_us, bytes)) = &g.frame
            {
                self.seen = g.seq;
                let header = PacketHeader {
                    seq: self.out_seq,
                    timestamp_us: *timestamp_us,
                    codec: ch.codec,
                    flags: FLAG_KEY,
                    len: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                };
                self.out_seq = self.out_seq.wrapping_add(1);
                return Some((header, bytes.to_vec()));
            }
            g = ch
                .cv
                .wait_timeout(g, Duration::from_secs(1))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn interval(&self) -> Duration {
        Duration::ZERO
    }
}

/// The mDNS instance name a Janus device publishes under. The pair client
/// renders a device by its TXT `kind=`, not by this, so it is a label for a
/// person reading a network browser.
const SIDECAR_INSTANCE: &str = "Janus device";

/// An mDNS hostname from a model: `janus/mesh-cam` is a model, `mesh-cam` is
/// a hostname. Everything that is not a letter, a digit or a hyphen becomes a
/// hyphen, because a label that breaks the rules is one a resolver drops.
fn mdns_hostname(model: &str) -> String {
    let tail = model.rsplit('/').next().unwrap_or(model);
    let mut out: String = tail
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    out.truncate(63); // one DNS label
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "janus".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The codec tag a subscriber sends to mean "whatever this device makes":
/// `rusty_esp_iroh_core::media::Subscribe` documents it as the device's
/// default, and every source in the family honours it. A subscriber that has
/// not been told what a device carries — a home computer meeting it for the
/// first time, the `client` example — sends this.
pub const CODEC_ANY: [u8; 4] = *b"any ";

/// A subscriber for a codec nothing here produces: over before it starts.
struct Nothing;

impl MediaSource for Nothing {
    fn next_packet(&mut self) -> Option<(PacketHeader, Vec<u8>)> {
        None
    }

    fn interval(&self) -> Duration {
        Duration::ZERO
    }
}

/// A subscriber factory, as the node takes it.
type Factory = Arc<dyn Fn(&Subscribe) -> Box<dyn MediaSource> + Send + Sync>;

struct State {
    shared: Arc<Shared>,
    did: String,
    ticket: String,
    port: u16,
    endpoint_id: String,
    stop: mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}

static MESH: Mutex<Option<State>> = Mutex::new(None);

/// The facade's view of the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Media subscribers that joined since `begin`.
    pub subscribers: u32,
    /// JPEG frames pushed.
    pub frames: u64,
    /// PCM blocks pushed.
    pub blocks: u64,
    /// Telemetry readings pushed.
    pub readings: u64,
    /// Media packets the **node** put on the wire, which is not the same
    /// number as the frames the sketch pushed: a subscriber takes the latest
    /// frame, and a device with no subscriber sends nothing at all.
    pub sent: u32,
    /// Media packets the node could not send.
    pub send_errors: u32,
    /// Subscriptions the node accepted.
    pub node_subscribers: u32,
}

fn try_begin(config: Config) -> Result<()> {
    if MESH
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
    {
        return Err(Error::Io("mesh: begin was already called".into()));
    }
    let (kv, rng) = identity::take_store().ok_or(Error::NotBegun("identity"))?;
    // Before a runtime, not after: on ESP-IDF tokio's I/O driver opens an
    // `eventfd`, and building the runtime is what fails if the VFS that
    // serves it was never registered.
    board::with(|b| b.prepare_async(ASYNC_MAX_FDS))?;
    let maker = identity::maker();
    // Signed updates take a maker to trust and a slot to write, together.
    // Without the maker every image is refused by name (`NoMaker`), so the
    // slot is not even asked for; without the slot the manifest does not
    // promise `ota`, and the node refuses at the manifest instead of at the
    // flash. Until 2026-09-16 the facade handed the node neither.
    let ota_sink = if maker.is_some() {
        board::with(|b| Ok(b.ota_sink()))?
    } else {
        None
    };
    let ota_armed = ota_sink.is_some();
    let mut config = config;
    if ota_armed
        && !config
            .declared
            .iter()
            .any(|d| d.capability == Capability::Ota)
    {
        config
            .declared
            .push(Declared::available(Capability::Ota, "rusty_esp_iroh"));
    }
    let model_for_host = config.model.clone();
    let mut ips: Vec<IpAddr> = Vec::new();
    if let Some(ip) = board::with(|b| Ok(b.local_ip()))? {
        ips.push(ip);
    }
    ips.push(IpAddr::V4(Ipv4Addr::LOCALHOST));

    let shared = Arc::new(Shared {
        video: Channel::new(CODEC_MJPEG),
        audio: Channel::new(CODEC_PCM),
        telemetry: Channel::new(CODEC_TELEMETRY),
        subscribers: AtomicU32::new(0),
        frames: AtomicU32::new(0),
        blocks: AtomicU32::new(0),
        readings: AtomicU32::new(0),
        node_sent: AtomicU32::new(0),
        node_send_errors: AtomicU32::new(0),
        node_subscribers: AtomicU32::new(0),
        readvertise: Mutex::new(None),
        adopted_seen: AtomicU32::new(0),
        last_ota: Mutex::new(None),
    });
    let factory_shared = Arc::clone(&shared);
    let counter_shared = Arc::clone(&shared);
    let factory: Factory = Arc::new(move |sub: &Subscribe| {
        factory_shared.subscribers.fetch_add(1, Ordering::Relaxed);
        let which = match sub.codec {
            c if c == CODEC_MJPEG => Some(Which::Video),
            c if c == CODEC_PCM => Some(Which::Audio),
            c if c == CODEC_TELEMETRY => Some(Which::Telemetry),
            // "The device's default" is what the device is actually making,
            // asked at the moment someone subscribes: the camera if it has
            // pushed a frame, else the microphone, else the sensor. A device
            // that has produced nothing yet answers with its video channel,
            // which is where a subscriber should wait for a camera that has
            // not warmed up. C2's first trip received nothing because this
            // arm did not exist (2026-09-11).
            c if c == CODEC_ANY => Some(
                if factory_shared.frames.load(Ordering::Relaxed) > 0 {
                    Which::Video
                } else if factory_shared.blocks.load(Ordering::Relaxed) > 0 {
                    Which::Audio
                } else if factory_shared.readings.load(Ordering::Relaxed) > 0 {
                    Which::Telemetry
                } else {
                    Which::Video
                },
            ),
            _ => None,
        };
        if let Some(which) = which {
            Box::new(LatestFrames {
                shared: Arc::clone(&factory_shared),
                which,
                seen: 0,
                out_seq: 0,
            }) as Box<dyn MediaSource>
        } else {
            Box::new(Nothing)
        }
    });

    type Ready = (String, String, u16, Vec<(String, String)>, String);
    let (ready_tx, ready_rx) = mpsc::channel::<Result<Ready>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let mut builder = thread::Builder::new().name("janus-mesh".into());
    if let Some(bytes) = MESH_STACK_BYTES {
        builder = builder.stack_size(bytes);
    }
    let thread = builder
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(Error::Io(format!("mesh: runtime: {e}"))));
                    return;
                }
            };
            rt.block_on(async move {
                let mut kv = DynKv(kv);
                let mut rng = DynRng(rng);
                let identity =
                    match NodeIdentity::load_or_create(&mut kv, &mut rng, identity::DEVICE_ID) {
                        Ok(i) => i,
                        Err(e) => {
                            let _ = ready_tx.send(Err(Error::Io(format!("mesh: identity: {e:?}"))));
                            return;
                        }
                    };
                let manifest = Manifest {
                    model: &config.model,
                    firmware: &config.firmware,
                    chip: config.chip,
                    declared: &config.declared,
                };
                let node_config = NodeConfig {
                    relay: config.relay,
                    model: config.model.clone(),
                    firmware: config.firmware.clone(),
                    max_media_subscribers: config.max_subscribers,
                };
                let extras = Extras {
                    maker_did: maker,
                    ota: ota_sink,
                    neighbours: None,
                };
                let node = match Node::bind_with(
                    identity,
                    kv.0,
                    &manifest,
                    Some(factory),
                    node_config,
                    extras,
                )
                .await
                {
                    Ok(n) => n,
                    Err(e) => {
                        let _ = ready_tx.send(Err(Error::Io(format!("mesh: bind: {e}"))));
                        return;
                    }
                };
                let _ = node.refresh_ticket(&ips);
                // The TXT record travels back with everything else: the node
                // lives on this thread, the board does not.
                let _ = ready_tx.send(Ok((
                    node.did().to_owned(),
                    node.ticket_text(),
                    node.port(),
                    node.sidecar_txt(&ips),
                    node.endpoint().id().to_string(),
                )));
                let ips_for_txt = ips.clone();
                // Only a CHANGE re-advertises. A device that boots already
                // adopted composed its boot record from the pin, and the
                // first poll must not replace it with itself (it did, at
                // 3.4 s, on the XIAO on 2026-09-16).
                counter_shared
                    .adopted_seen
                    .store(u32::from(node.is_adopted()), Ordering::Relaxed);
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    // What the NODE did, beside what the sketch pushed. A
                    // subscriber that receives nothing is a different defect
                    // depending on whether the node sent and failed, sent
                    // nothing, or never saw the subscription -- and until
                    // 2026-09-11 the sketch could not tell those apart.
                    {
                        let c = &node.state().counters;
                        counter_shared
                            .node_sent
                            .store(c.media_packets.load(Ordering::Relaxed), Ordering::Relaxed);
                        counter_shared.node_send_errors.store(
                            c.media_send_errors.load(Ordering::Relaxed),
                            Ordering::Relaxed,
                        );
                        counter_shared.node_subscribers.store(
                            c.media_subscribers.load(Ordering::Relaxed),
                            Ordering::Relaxed,
                        );
                    }
                    // The advertisement has to follow the pin. When adoption
                    // flips, recompute the record here -- the node is on this
                    // thread -- and leave it for `service()` to hand to the
                    // board on the sketch's thread.
                    let adopted_now = u32::from(node.is_adopted());
                    if adopted_now != counter_shared.adopted_seen.swap(adopted_now, Ordering::Relaxed) {
                        *counter_shared
                            .readvertise
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) =
                            Some(node.sidecar_txt(&ips_for_txt));
                    }
                    // An accepted update: the node has written and verified
                    // it and told the owner so. The sketch reads this and
                    // restarts; the node itself never reboots anything.
                    if let Some(fw) = node.last_ota() {
                        let mut slot = counter_shared
                            .last_ota
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        if slot.as_deref() != Some(fw.as_str()) {
                            *slot = Some(fw);
                        }
                    }
                    match stop_rx.try_recv() {
                        Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                }
                node.shutdown().await;
            });
        })
        .map_err(|e| Error::Io(format!("mesh: thread: {e}")))?;

    let (did, ticket, port, txt, endpoint_id) = ready_rx
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| Error::Io("mesh: the node did not come up within 30 s".into()))??;
    // Publish it. A board that cannot answers Ok and the device is reachable
    // by ticket alone; a board that can is one a pair client will list.
    let hostname = mdns_hostname(&model_for_host);
    if let Err(e) = board::with(|b| {
        b.advertise(
            &hostname,
            SIDECAR_INSTANCE,
            rusty_esp_iroh_core::sidecar::SERVICE_TYPE,
            port,
            &txt,
        )
    }) {
        // Not fatal: the node is up and serving, it is only unannounced.
        record(Error::Io(format!("mesh: advertise: {e:?}")));
    }
    // The endpoint is up, so the running image is good: a bootloader with a
    // rollback pending cancels it here, and an image that boots but never
    // reaches the mesh is the one it drops. Only a board that handed over a
    // slot has such a bootloader to tell.
    if ota_armed {
        if let Err(e) = board::with(|b| b.ota_running_valid()) {
            record(Error::Io(format!("mesh: ota_running_valid: {e:?}")));
        }
    }
    *MESH.lock().unwrap_or_else(PoisonError::into_inner) = Some(State {
        shared,
        did,
        ticket,
        port,
        endpoint_id,
        stop: stop_tx,
        thread: Some(thread),
    });
    Ok(())
}

/// Bind the node and start serving: adoption, the manifest RPC, the media
/// ALPN. `false` — and [`crate::last_error`] says why — without
/// `identity::begin`, or when the endpoint cannot bind.
pub fn begin(config: Config) -> bool {
    match try_begin(config) {
        Ok(()) => true,
        Err(e) => {
            record(e);
            false
        }
    }
}

fn with_state<R>(f: impl FnOnce(&State) -> R) -> Option<R> {
    MESH.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(f)
}

/// Offer `frame` to every media subscriber (the newest frame wins when a
/// subscriber is slower than the camera). `false` before `begin`.
pub fn push_media(frame: &Jpeg) -> bool {
    match with_state(|s| {
        s.shared.video.push(frame.timestamp.0, &frame.bytes);
        s.shared.frames.fetch_add(1, Ordering::Relaxed);
    }) {
        Some(()) => true,
        None => {
            record(Error::NotBegun("mesh"));
            false
        }
    }
}

/// Offer `pcm` to every PCM subscriber. `false` before `begin`.
pub fn push_media_pcm(pcm: &Pcm) -> bool {
    match with_state(|s| {
        s.shared.audio.push(pcm.timestamp.0, &pcm.bytes);
        s.shared.blocks.fetch_add(1, Ordering::Relaxed);
    }) {
        Some(()) => true,
        None => {
            record(Error::NotBegun("mesh"));
            false
        }
    }
}

/// Offer `reading` to every telemetry subscriber. The bytes are opaque to
/// the mesh; encode them with
/// `rusty_esp_signal_core::radar::presence::Presence::encode`, which is what
/// a home computer decodes them with. `false` before `begin`.
pub fn push_telemetry(reading: &[u8]) -> bool {
    let at = crate::sketch::millis().saturating_mul(1000);
    match with_state(|s| {
        s.shared.telemetry.push(at, reading);
        s.shared.readings.fetch_add(1, Ordering::Relaxed);
    }) {
        Some(()) => true,
        None => {
            record(Error::NotBegun("mesh"));
            false
        }
    }
}

/// Give the node its turn. The node runs on its own thread, so this is the
/// sketch's place to read what happened; it returns the counters. It is also
/// where a changed advertisement reaches the board: the mesh thread queues
/// the record when the owner pin changes, and this call, on the sketch's
/// thread, is the one that publishes it -- one thread owns the board.
#[must_use]
pub fn service() -> Stats {
    let pending: Option<Vec<(String, String)>> = with_state(|s| {
        s.shared
            .readvertise
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    })
    .flatten();
    if let Some(txt) = pending {
        if let Err(e) = board::with(|b| {
            b.advertise_txt(rusty_esp_iroh_core::sidecar::SERVICE_TYPE, &txt[..])
        }) {
            record(Error::Io(format!("mesh: advertise_txt: {e:?}")));
        }
    }
    with_state(|s| Stats {
        subscribers: s.shared.subscribers.load(Ordering::Relaxed),
        frames: u64::from(s.shared.frames.load(Ordering::Relaxed)),
        blocks: u64::from(s.shared.blocks.load(Ordering::Relaxed)),
        readings: u64::from(s.shared.readings.load(Ordering::Relaxed)),
        sent: s.shared.node_sent.load(Ordering::Relaxed),
        send_errors: s.shared.node_send_errors.load(Ordering::Relaxed),
        node_subscribers: s.shared.node_subscribers.load(Ordering::Relaxed),
    })
    .unwrap_or(Stats {
        subscribers: 0,
        frames: 0,
        blocks: 0,
        readings: 0,
        sent: 0,
        send_errors: 0,
        node_subscribers: 0,
    })
}

/// The node's DID (the device's, from `identity`).
#[must_use]
pub fn did() -> Option<String> {
    with_state(|s| s.did.clone())
}

/// The adoption ticket a home computer scans, as text.
#[must_use]
pub fn ticket() -> Option<String> {
    with_state(|s| s.ticket.clone())
}

/// The firmware string of an update the node accepted into the boot slot,
/// once there is one: the bytes are written and verified, the owner was told
/// `Committed`, and the bootloader will try the new image next. This is the
/// sketch's cue to restart; the facade never restarts anything itself.
#[must_use]
pub fn last_ota() -> Option<String> {
    with_state(|s| {
        s.shared
            .last_ota
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    })
    .flatten()
}

/// The node's endpoint id, once `begin` succeeded: what a ticket carries and
/// what persists across a reflash. The sketch prints it itself so an offline
/// run reads a line the sketch owns, not one borrowed from a library's log.
#[must_use]
pub fn endpoint_id() -> Option<String> {
    with_state(|s| s.endpoint_id.clone())
}

/// The UDP port the endpoint bound.
#[must_use]
pub fn port() -> Option<u16> {
    with_state(|s| s.port)
}

/// Whether `begin` succeeded.
#[must_use]
pub fn begun() -> bool {
    with_state(|_| ()).is_some()
}

/// Stop the node and release the port (tests; a sketch that ends).
pub fn end() {
    let state = MESH.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(mut s) = state {
        s.shared.video.close();
        s.shared.audio.close();
        s.shared.telemetry.close();
        let _ = s.stop.send(());
        if let Some(t) = s.thread.take() {
            let _ = t.join();
        }
    }
}
