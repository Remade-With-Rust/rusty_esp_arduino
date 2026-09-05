//! `stream::listen_gated` serves the page and the stream only to whoever
//! has the device's page token — the URL printed at boot. Its own binary:
//! the facade keeps one server per process.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use rusty_esp_arduino::prelude::*;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// One request; the response until the peer closes, `budget` passes, or
/// `done` says enough arrived.
fn request(
    target: &str,
    head: &str,
    budget: Duration,
    mut done: impl FnMut(&[u8]) -> bool,
) -> Vec<u8> {
    let mut s = TcpStream::connect(target).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    s.write_all(head.as_bytes()).unwrap();
    let mut body = Vec::new();
    let started = Instant::now();
    let mut chunk = [0u8; 16 * 1024];
    while started.elapsed() < budget && !done(&body) {
        match s.read(&mut chunk) {
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
fn the_page_and_the_stream_need_the_token_the_identity_minted() {
    let dir = std::env::temp_dir().join(format!("janus-gated-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(
        cam::begin(cam::Config::qvga_jpeg().at_fps(30)),
        "{:?}",
        last_error()
    );

    // no identity yet: nothing to gate on, and the refusal says so
    assert!(!stream::listen_gated(0));
    assert!(
        matches!(last_error(), Some(Error::Missing(m)) if m.contains("identity::begin")),
        "{:?}",
        last_error()
    );
    assert!(stream::page_url("10.0.0.5").is_none());

    assert!(identity::begin(None), "{:?}", last_error());
    let token = identity::page_token().expect("minted with the key");
    assert!(stream::listen_gated(0), "{:?}", last_error());
    assert_eq!(stream::token().as_deref(), Some(token.as_str()));
    let addr = stream::listen_addr().expect("listening");
    let target = format!("127.0.0.1:{}", addr.port());
    assert_eq!(
        stream::page_url("10.0.0.5").as_deref(),
        Some(format!("http://10.0.0.5:{}/?t={token}", addr.port()).as_str()),
        "the line the sketch prints"
    );

    let full = |b: &[u8]| find(b, b"\r\n\r\n").is_some();

    // bare, wrong, a prefix: refused
    for head in [
        "GET / HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n".to_owned(),
        "GET /stream HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n".to_owned(),
        format!(
            "GET /?t={}x HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n",
            token
        ),
        format!(
            "GET /?t={} HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n",
            &token[..token.len() - 1]
        ),
        format!(
            "GET /stream HTTP/1.1\r\nHost: cam\r\nCookie: t={}x\r\nConnection: close\r\n\r\n",
            token
        ),
    ] {
        let resp = request(&target, &head, Duration::from_secs(5), full);
        assert!(
            resp.starts_with(b"HTTP/1.1 403"),
            "{head:?} -> {}",
            String::from_utf8_lossy(&resp[..resp.len().min(40)])
        );
    }

    // the printed URL: the page, which sets the cookie its <img> needs
    let page = request(
        &target,
        &format!("GET /?t={token} HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n"),
        Duration::from_secs(5),
        |b| find(b, b"</html>").is_some(),
    );
    let page_text = String::from_utf8_lossy(&page);
    assert!(page_text.starts_with("HTTP/1.1 200"), "{page_text}");
    assert!(page_text.contains(&format!(
        "Set-Cookie: t={token}; Path=/; SameSite=Strict; HttpOnly"
    )));
    assert!(page_text.contains("<img src=\"/stream\""));

    // a pusher: the loop half of the sketch
    let pusher = thread::spawn(|| {
        for _ in 0..40 {
            if let Some(frame) = cam::grab() {
                stream::push_jpeg(&frame);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });

    // the browser's /stream with the cookie: frames
    let stream = request(
        &target,
        &format!("GET /stream HTTP/1.1\r\nHost: cam\r\nCookie: t={token}\r\n\r\n"),
        Duration::from_secs(5),
        |b| find(b, b"\xff\xd9").is_some(),
    );
    assert!(
        stream.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&stream[..stream.len().min(40)])
    );
    assert!(find(&stream, b"multipart/x-mixed-replace").is_some());
    assert!(
        find(&stream, b"\xff\xd8").is_some(),
        "a JPEG arrived through the gate"
    );
    pusher.join().unwrap();

    let stats = stream::stats();
    assert!(stats.http.streams >= 1, "{stats:?}");
    assert!(
        stats.http.other >= 6,
        "the refusals and the page are counted: {stats:?}"
    );

    identity::end();
    board::uninstall();
    let _ = std::fs::remove_dir_all(&dir);
}
