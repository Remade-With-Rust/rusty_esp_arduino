//! The plan's sketch, end to end on the laptop: colour bars and a tone out of
//! the facade, RTP/JPEG and PCM datagrams back in through
//! `rusty_esp_video-esp`'s own receivers, every frame and block counted.
//! Its own binary: the facade's state is per process.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use rusty_esp_arduino::prelude::*;
use rusty_esp_video_core::rtp::HEADER_RESERVE;
use rusty_esp_video_esp::udp_net::{Until, receive_raw, receive_rtp_jpeg};

const FRAMES: u64 = 12;

static GRABBED: AtomicU64 = AtomicU64::new(0);
static READ: AtomicU64 = AtomicU64::new(0);

fn setup() {
    board::install(HostBoard::new().unpaced());
    assert!(wifi::begin("home", "psk"));
    assert!(wifi::connected());
    assert!(cam::begin(cam::Config::qvga_jpeg()), "{:?}", last_error());
    assert!(mic::begin(mic::Config::pcm16_16k()), "{:?}", last_error());
}

fn loop_once() {
    if let Some(frame) = cam::grab() {
        GRABBED.fetch_add(1, Ordering::Relaxed);
        assert!(stream::push_jpeg(&frame), "{:?}", last_error());
    }
    if let Some(pcm) = mic::read() {
        READ.fetch_add(1, Ordering::Relaxed);
        assert!(stream::push_pcm(&pcm), "{:?}", last_error());
    }
}

#[test]
fn the_sketch_streams_every_frame_and_block_to_the_video_packages_receivers() {
    let rtp_rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let pcm_rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (rtp_addr, pcm_addr) = (rtp_rx.local_addr().unwrap(), pcm_rx.local_addr().unwrap());
    assert!(stream::rtp_to(rtp_addr), "{:?}", last_error());
    assert!(stream::pcm_to(pcm_addr), "{:?}", last_error());

    let rtp_thread = thread::spawn(move || {
        let mut buf = vec![0u8; HEADER_RESERVE + 256 * 1024];
        let mut sizes = Vec::new();
        let stats = receive_rtp_jpeg(
            &rtp_rx,
            &mut buf,
            Until {
                frames: FRAMES,
                for_at_most: Duration::from_secs(20),
            },
            |jpeg, _ts, w, h| sizes.push((jpeg.len(), w, h)),
        )
        .unwrap();
        (stats, sizes)
    });
    let pcm_thread = thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        let mut lens = Vec::new();
        let stats = receive_raw(
            &pcm_rx,
            &mut buf,
            Until {
                frames: FRAMES,
                for_at_most: Duration::from_secs(20),
            },
            |_header, payload| lens.push(payload.len()),
        )
        .unwrap();
        (stats, lens)
    });

    // the receivers poll; give them a moment to be listening, then run the
    // sketch, spacing frames so RTP fragments of one frame never race the
    // next on loopback
    thread::sleep(Duration::from_millis(100));
    sketch::run_for(
        setup,
        || {
            loop_once();
            thread::sleep(Duration::from_millis(5));
        },
        FRAMES,
    );

    let (rtp_stats, sizes) = rtp_thread.join().unwrap();
    let (pcm_stats, lens) = pcm_thread.join().unwrap();

    assert_eq!(GRABBED.load(Ordering::Relaxed), FRAMES);
    assert_eq!(READ.load(Ordering::Relaxed), FRAMES);
    let s = stream::stats();
    assert_eq!((s.jpeg_pushed, s.pcm_pushed), (FRAMES, FRAMES));
    assert_eq!(s.rtp.frames, FRAMES);
    assert_eq!(s.rtp.dropped, 0);
    assert_eq!(s.pcm.frames, FRAMES);

    assert_eq!(rtp_stats.frames, FRAMES, "{rtp_stats:?}");
    assert_eq!(rtp_stats.lost, 0);
    assert_eq!(rtp_stats.packets, s.rtp.packets, "every datagram arrived");
    assert!(
        sizes.iter().all(|&(_, w, h)| (w, h) == (320, 240)),
        "{sizes:?}"
    );
    // the depayloader rebuilds the JPEG with its own headers, so byte counts
    // differ from what the camera produced; the scan is the same
    assert!(sizes.iter().all(|&(n, _, _)| n > 1000), "{sizes:?}");

    assert_eq!(pcm_stats.frames, FRAMES, "{pcm_stats:?}");
    assert_eq!(pcm_stats.lost, 0);
    let block = mic::Config::pcm16_16k().block_bytes();
    assert!(lens.iter().all(|&n| n == block), "{lens:?} vs {block}");

    stream::stop_senders();
    board::uninstall();
}
