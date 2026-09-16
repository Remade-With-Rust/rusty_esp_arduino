//! `mesh::begin` / `push_media`: the node comes up on the laptop with the
//! identity `identity::begin` minted, and a subscriber through the iroh
//! package's own client receives the frames the sketch pushes, in order,
//! byte for byte. One test per binary — the facade's state is global.

use std::thread;
use std::time::Duration;

use rusty_esp_arduino::board::Jpeg;
use rusty_esp_arduino::host::HostBoard;
use rusty_esp_arduino::{board, identity, last_error, mesh};
use rusty_esp_core::capability::{Capability, Chip, Declared};
use rusty_esp_core::time::Micros;
use rusty_esp_iroh_core::media::Subscribe;
use rusty_esp_iroh_core::mid::adoption::{AdoptionFields, CapList};
use rusty_esp_iroh_core::mid::key::DeviceKey;
use rusty_esp_iroh_core::ota::{MemorySlots, OtaManifest};
use rusty_esp_iroh_core::rpc::{Request, Response};
use rusty_esp_iroh_core::ticket::Ticket;
use rusty_esp_iroh_host::Client;
use rusty_esp_iroh_host::client::{OtaOutcome, endpoint_addr};
use rusty_esp_iroh_host::mjpeg::CODEC_MJPEG;

#[test]
fn pushed_frames_reach_a_subscriber_under_the_device_did() {
    let dir = std::env::temp_dir().join(format!("janus-mesh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // before identity: refused by name
    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(!mesh::begin(mesh::Config::new("janus/test", Chip::Esp32S3)));
    assert!(
        matches!(
            last_error(),
            Some(rusty_esp_arduino::Error::NotBegun("identity"))
        ),
        "{:?}",
        last_error()
    );
    assert!(!mesh::push_media(&frame(0)));

    // The maker the device trusts for updates: a deterministic bench key,
    // named at `identity::begin` the way a provisioned `maker` is.
    let maker = DeviceKey::from_seed_for_tests("maker", "bench");
    let maker_did = maker.did().to_did_string();
    assert!(identity::begin(Some(&maker_did)), "{:?}", last_error());
    let did = identity::did().unwrap();
    let config = mesh::Config::new("janus/test", Chip::Esp32S3)
        .declare(Declared::available(
            Capability::VideoMjpeg,
            "rusty_esp_video",
        ))
        .declare(Declared::available(Capability::MidDevice, "rusty_esp_mid"));
    assert!(mesh::begin(config), "{:?}", last_error());
    // Before it built a runtime, `begin` asked the board to prepare the
    // platform for one. It is a no-op on the laptop and it is the whole
    // difference between a mesh and no mesh on ESP-IDF, where tokio's I/O
    // driver opens an `eventfd` that answers `EACCES` until a VFS is
    // registered: C2's first boot on the XIAO, 2026-09-11.
    assert_eq!(
        rusty_esp_arduino::host::async_prepared(),
        vec![5],
        "begin prepares the platform once, before the runtime"
    );
    assert_eq!(
        mesh::did().as_deref(),
        Some(did.as_str()),
        "the node is the device"
    );
    // The device published itself, which is the only way anything finds it
    // without being handed a ticket first. A generated mesh cell advertised
    // nothing at all until 2026-09-11 and was reachable by ticket alone.
    let (hostname, instance, service, port, txt) =
        rusty_esp_arduino::host::advertised().expect("the board was asked to advertise");
    assert_eq!(service, "_mata-oem-sidecar._tcp.local.");
    assert_eq!(hostname, "test", "the model's last segment, as a DNS label");
    assert_eq!(instance, "Janus device");
    assert_eq!(port, mesh::port().expect("a port"), "the iroh UDP port");
    let keys: Vec<&str> = txt.iter().map(|(k, _)| k.as_str()).collect();
    for k in ["kind", "protocol", "iroh_node_id", "iroh_direct", "did"] {
        assert!(keys.contains(&k), "the TXT record needs {k}: {keys:?}");
    }
    assert_eq!(
        txt.iter().find(|(k, _)| k == "kind").map(|(_, v)| v.as_str()),
        Some("oem_sidecar"),
        "what makes the pair client render it as a box"
    );

    let ticket = mesh::ticket().expect("a ticket");
    assert!(mesh::port().is_some());
    // A maker and a slot together arm updates: the running image was marked
    // good exactly once, after the endpoint came up.
    assert_eq!(rusty_esp_arduino::host::ota_running_valid_calls(), 1);
    assert!(mesh::last_ota().is_none());
    // The endpoint id is the facade's to report, and it is the key the ticket
    // carries -- so a sketch prints a line it owns, not one borrowed from a
    // library's log (which a log level silenced once, for a whole trip).
    let endpoint_id = mesh::endpoint_id().expect("an endpoint id");
    let parsed = Ticket::parse_text(&ticket).unwrap();
    let hex: String = parsed.endpoint_id.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(endpoint_id, hex, "the endpoint id is the key inside the ticket");
    assert!(
        !mesh::begin(mesh::Config::new("janus/test", Chip::Esp32S3)),
        "twice is refused"
    );

    // the sketch's loop, pushing frames
    let frames: Vec<Jpeg> = (0..6u8).map(frame).collect();
    let pusher = {
        let frames = frames.clone();
        thread::spawn(move || {
            for f in frames.iter().cycle().take(60) {
                assert!(mesh::push_media(f));
                thread::sleep(Duration::from_millis(40));
            }
        })
    };

    // the iroh package's client, subscribing through the ticket
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (got, loss, any) = rt.block_on(async {
        let client = Client::bind(None, None, false).await.unwrap();
        let t = Ticket::parse_text(&ticket).unwrap();
        let addr = endpoint_addr(&t).unwrap();
        let mut got: Vec<(u32, Vec<u8>)> = Vec::new();
        let sub = Subscribe {
            codec: CODEC_MJPEG,
            max_fps: 30,
            max_kbps: 0,
        };
        let loss = client
            .subscribe(&addr, &sub, 8, Duration::from_secs(15), |h, bytes| {
                got.push((h.seq, bytes.to_vec()));
            })
            .await
            .unwrap();
        // And again as a subscriber that has not been told what this device
        // carries -- the `client` example, a home computer meeting it for the
        // first time. `any ` is the protocol's word for the device's default,
        // and asking with the explicit tag alone is how a green test hid a
        // board that sent nothing (C2's first trip, 2026-09-11).
        let mut any: Vec<Vec<u8>> = Vec::new();
        let wild = Subscribe {
            codec: mesh::CODEC_ANY,
            max_fps: 30,
            max_kbps: 0,
        };
        let _ = client
            .subscribe(&addr, &wild, 4, Duration::from_secs(15), |_h, bytes| {
                any.push(bytes.to_vec());
            })
            .await
            .unwrap();
        client.close().await;
        (got, loss, any)
    });
    pusher.join().unwrap();

    assert_eq!(got.len(), 8, "eight packets asked for, eight received");
    assert_eq!(
        any.len(),
        4,
        "a subscriber asking for the device's default gets the camera"
    );
    for bytes in &any {
        assert!(
            frames.iter().any(|f| &f.bytes == bytes),
            "the default channel served a frame that was pushed"
        );
    }
    assert_eq!(loss.lost, 0);
    for (i, (seq, bytes)) in got.iter().enumerate() {
        assert_eq!(*seq, i as u32, "the subscriber's own sequence, from 0");
        assert!(
            frames.iter().any(|f| &f.bytes == bytes),
            "packet {i} is a frame that was pushed"
        );
    }
    let stats = mesh::service();
    assert!(stats.subscribers >= 1, "{stats:?}");
    assert!(stats.frames >= 8, "{stats:?}");

    // Adoption must change what the device advertises. Before this, the
    // record was composed once at boot: a device that had an owner went on
    // advertising `pair_state=open`, and a home computer would have offered
    // to claim it (measured on the XIAO, 2026-09-12).
    assert!(
        rusty_esp_arduino::host::advertised_updates().is_empty(),
        "nothing to re-advertise before adoption"
    );
    let owner = DeviceKey::from_seed_for_tests("owner", "hub");
    let owner_did = {
        let mut buf = [0u8; 64];
        String::from(owner.did().write(&mut buf).unwrap())
    };
    let owner_did_obj = owner.did();
    let caps = ["media:subscribe@*", "telemetry:read@*"];
    let fields = AdoptionFields {
        device_did: &did,
        owner_did: &owner_did,
        owner_genesis_pubkey: owner_did_obj.pubkey(),
        hub_endpoint_id: &[0u8; 32],
        hub_relay: "",
        hub_host: "",
        caps: CapList::Slice(&caps),
        roster_version: 1,
        issued_at: 1_700_000_000,
        expires_at: 0,
    };
    let mut adoption = vec![0u8; 1024];
    let n = fields.sign_into(&owner, &mut adoption).unwrap();
    adoption.truncate(n);
    let rt2 = tokio::runtime::Runtime::new().unwrap();
    let (adopted, owner_client, addr) = rt2.block_on(async {
        let owner_client = Client::bind(None, Some(owner), false).await.unwrap();
        let t = Ticket::parse_text(&ticket).unwrap();
        let addr = endpoint_addr(&t).unwrap();
        let r = owner_client.rpc(&addr, &did, Request::Adopt(adoption)).await.unwrap();
        (r, owner_client, addr)
    });
    assert!(matches!(adopted, Response::Adopted { .. }), "{adopted:?}");
    // The mesh thread notices on its next poll; the sketch's next `service`
    // call hands the new record to the board.
    let mut updates = Vec::new();
    for _ in 0..40 {
        let _ = mesh::service();
        updates = rusty_esp_arduino::host::advertised_updates();
        if !updates.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(updates.len(), 1, "one replacement record after adoption");
    let pair = updates[0]
        .iter()
        .find(|(k, _)| k == "pair_state")
        .map(|(_, v)| v.as_str());
    assert_eq!(pair, Some("paired"), "the advertisement follows the pin: {:?}", updates[0]);

    // A signed update, from the owner, through the same client. The maker
    // signed the image and the device was told at `identity::begin` whom to
    // trust; the laptop board's two slots stand in for `esp-ota`'s. Before
    // this the facade handed the node no slot at all (`ota: None`), so every
    // image was `Unsupported` however well it was signed.
    let payload = vec![0x5Au8; 40_000];
    let image = MemorySlots::image("1.5.0", &payload);
    let manifest = OtaManifest::sign(
        "janus/test",
        "1.5.0",
        Chip::Esp32S3,
        &image,
        &maker_did,
        &maker,
    )
    .unwrap();
    let outcome = rt2.block_on(async {
        let r = owner_client.ota(&addr, &did, &manifest, &image).await.unwrap();
        owner_client.close().await;
        r
    });
    assert!(
        matches!(&outcome, OtaOutcome::Committed { firmware, .. } if firmware == "1.5.0"),
        "{outcome:?}"
    );
    // The sketch's cue: the mesh thread copies the node's verdict out on its
    // next poll, and `last_ota` is what a sketch restarts on.
    let mut last = None;
    for _ in 0..40 {
        let _ = mesh::service();
        last = mesh::last_ota();
        if last.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(last.as_deref(), Some("1.5.0"), "the sketch learns the firmware it will boot into");

    mesh::end();
    assert!(!mesh::begun());
    identity::end();
    board::uninstall();
    let _ = std::fs::remove_dir_all(&dir);
}

fn frame(i: u8) -> Jpeg {
    Jpeg {
        bytes: vec![0xFF, 0xD8, 0xFF, 0xE0, i, i, i, 0xFF, 0xD9],
        width: 320,
        height: 240,
        timestamp: Micros(u64::from(i) * 66_667),
    }
}
