//! `stream::listen_status` serves `/v1/status` alone, for a board with no
//! camera that a home's hub still has to find: the plain and the nonce
//! forms from the `on_status` handler, a bad query refused, and no page, no
//! stream and no other method. Its own binary: the facade keeps one server
//! per process.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use rusty_esp_arduino::prelude::*;

fn ask(target: &str, request: &str) -> String {
    let mut s = TcpStream::connect(target).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn get(target: &str, path: &str) -> String {
    ask(
        target,
        &format!("GET {path} HTTP/1.1\r\nHost: board\r\nConnection: close\r\n\r\n"),
    )
}

#[test]
fn only_the_status_is_served() {
    assert!(!stream::listen_status(0), "no handler, no server");
    assert!(matches!(last_error(), Some(Error::Missing(_))));

    stream::on_status(|nonce| match nonce {
        None => r#"{"pair":"ready"}"#.to_owned(),
        Some(n) => format!(r#"{{"nonce0":{}}}"#, n[0]),
    });
    assert!(stream::listen_status(0), "{:?}", last_error());
    // bound on every interface; dialled on this one
    let target = format!("127.0.0.1:{}", stream::listen_addr().unwrap().port());

    let plain = get(&target, "/v1/status");
    assert!(plain.starts_with("HTTP/1.1 200"), "{plain}");
    assert!(plain.contains("application/json"), "{plain}");
    assert!(plain.ends_with(r#"{"pair":"ready"}"#), "{plain}");

    let nonce = format!("/v1/status?nonce=ab{}", "0".repeat(62));
    let signed = get(&target, &nonce);
    assert!(signed.ends_with(r#"{"nonce0":171}"#), "{signed}");

    for (path, line) in [
        ("/v1/status?nonce=xyz", "HTTP/1.1 400"),
        ("/", "HTTP/1.1 404"),
        ("/stream", "HTTP/1.1 404"),
        ("/anything", "HTTP/1.1 404"),
    ] {
        let r = get(&target, path);
        assert!(r.starts_with(line), "{path}: {r}");
    }
    let put = ask(
        &target,
        "PUT /update HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(put.starts_with("HTTP/1.1 405"), "{put}");

    // a client that connects and says nothing is let go, and the next is answered
    let silent = TcpStream::connect(&target).unwrap();
    let after = get(&target, "/v1/status");
    assert!(after.starts_with("HTTP/1.1 200"), "{after}");
    drop(silent);

    assert!(stream::stats().http.connections >= 8);
}
