//! A `/stream` viewer that gets no frames does not hold the server: the
//! connection ends after [`stream::STREAM_STALL`] and the next request is
//! answered. Its own binary: the facade keeps one server per process.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use rusty_esp_arduino::prelude::*;

#[test]
fn a_stream_with_no_frames_ends_and_the_server_moves_on() {
    board::install(HostBoard::new().unpaced());
    // no cam::begin, no pusher: the slot never fills
    assert!(stream::listen(0), "{:?}", last_error());
    let addr = stream::listen_addr().expect("listening");
    let target = format!("127.0.0.1:{}", addr.port());

    let started = Instant::now();
    let mut viewer = TcpStream::connect(&target).unwrap();
    viewer
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    viewer
        .write_all(b"GET /stream HTTP/1.1\r\nHost: cam\r\n\r\n")
        .unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let closed = loop {
        match viewer.read(&mut buf) {
            Ok(0) => break true,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if started.elapsed() > Duration::from_secs(20) {
                    break false;
                }
            }
            Err(_) => break true,
        }
    };
    let waited = started.elapsed();
    assert!(closed, "the server never let go of a frameless viewer");
    assert!(
        got.starts_with(b"HTTP/1.1 200 OK"),
        "the head is sent before the wait: {}",
        String::from_utf8_lossy(&got[..got.len().min(40)])
    );
    assert!(
        waited >= stream::STREAM_STALL && waited < stream::STREAM_STALL + Duration::from_secs(4),
        "given up after the stall, not before and not much after: {waited:?}"
    );

    // …and the next request is served at once
    let mut next = TcpStream::connect(&target).unwrap();
    next.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    next.write_all(b"GET / HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut page = Vec::new();
    let _ = next.read_to_end(&mut page);
    assert!(
        page.starts_with(b"HTTP/1.1 200 OK"),
        "{}",
        String::from_utf8_lossy(&page[..page.len().min(40)])
    );
    // the counters are written when a connection is done, a moment after
    // the client sees it close
    let settled = Instant::now();
    let st = loop {
        let st = stream::stats();
        if st.http.other >= 1 || settled.elapsed() > Duration::from_secs(2) {
            break st;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(st.http.streams, 1, "{st:?}");
    assert!(st.http.other >= 1, "{st:?}");
    board::uninstall();
}
