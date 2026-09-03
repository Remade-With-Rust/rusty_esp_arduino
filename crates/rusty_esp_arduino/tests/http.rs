//! `stream::listen` serves the MJPEG page a browser (or ffmpeg) opens. Its own
//! binary: the facade's state is per process.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use rusty_esp_arduino::prelude::*;

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// Read until the peer closes, a timeout passes with nothing new, or `done`
/// says enough has arrived.
fn read_while(
    stream: &mut TcpStream,
    budget: Duration,
    mut done: impl FnMut(&[u8]) -> bool,
) -> Vec<u8> {
    let mut body = Vec::new();
    let started = Instant::now();
    let mut chunk = [0u8; 16 * 1024];
    while started.elapsed() < budget && !done(&body) {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => panic!("{e}"),
        }
    }
    body
}

#[test]
fn the_page_and_the_stream_are_served_from_the_frames_the_sketch_pushes() {
    board::install(HostBoard::new().unpaced());
    assert!(
        cam::begin(cam::Config::qvga_jpeg().at_fps(30)),
        "{:?}",
        last_error()
    );
    assert!(stream::listen(0), "{:?}", last_error());
    let addr = stream::listen_addr().expect("listening");
    assert_eq!(addr.ip().to_string(), "0.0.0.0");
    let target = format!("127.0.0.1:{}", addr.port());

    // the index page
    let mut index = TcpStream::connect(&target).unwrap();
    index
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    index
        .write_all(b"GET / HTTP/1.1\r\nHost: sketch\r\nConnection: close\r\n\r\n")
        .unwrap();
    let page = read_while(&mut index, Duration::from_secs(5), |b| {
        count(b, b"/stream") >= 1 && b.ends_with(b">\n") || b.ends_with(b">")
    });
    drop(index);
    assert!(
        page.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&page[..40.min(page.len())])
    );
    assert!(count(&page, b"/stream") >= 1, "the page links the stream");

    // a pusher: the loop half of the sketch, 40 frames at 30 fps
    let pusher = thread::spawn(|| {
        for _ in 0..40 {
            if let Some(frame) = cam::grab() {
                stream::push_jpeg(&frame);
            }
            thread::sleep(Duration::from_millis(33));
        }
    });

    // the stream: read until three JPEGs have gone by
    let mut client = TcpStream::connect(&target).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    client
        .write_all(b"GET /stream HTTP/1.1\r\nHost: sketch\r\n\r\n")
        .unwrap();
    let body = read_while(&mut client, Duration::from_secs(10), |b| {
        count(b, &[0xFF, 0xD8, 0xFF]) >= 3
    });
    drop(client);
    pusher.join().unwrap();

    assert!(
        body.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&body[..40.min(body.len())])
    );
    assert_eq!(
        count(&body, b"multipart/x-mixed-replace"),
        1,
        "an MJPEG response"
    );
    let jpegs = count(&body, &[0xFF, 0xD8, 0xFF]);
    assert!(jpegs >= 3, "{jpegs} JPEG starts in {} bytes", body.len());
    assert!(stream::stats().jpeg_pushed >= 3);
    // the server folds its counters in when a connection ends
    thread::sleep(Duration::from_millis(300));
    let http = stream::stats().http;
    assert!(http.connections >= 2, "{http:?}");
    assert!(http.other >= 1 && http.streams >= 1, "{http:?}");
    assert!(http.frames >= 3, "{http:?}");
    board::uninstall();
}
