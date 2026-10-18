//! `stream::on_status` serves `/v1/status` ungated beside the gated page,
//! and a document signed with `identity::sign_prehash` verifies under the
//! device's own DID. Its own binary: the facade keeps one server per process.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusty_esp_arduino::prelude::*;
use rusty_esp_mid_core::status::{StatusDocument, StatusProof};

fn get(target: &str, path: &str) -> String {
    let mut s = TcpStream::connect(target).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: cam\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn body(resp: &str) -> serde_json::Value {
    let at = resp.find("\r\n\r\n").expect("a head") + 4;
    serde_json::from_str(&resp[at..]).expect("JSON")
}

#[test]
fn the_status_is_served_ungated_and_its_signature_is_the_devices() {
    let dir = std::env::temp_dir().join(format!("janus-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(
        identity::sign_prehash(&[0; 32]).is_none(),
        "no key before begin"
    );
    assert!(identity::begin(None), "{:?}", last_error());
    let did = identity::did().unwrap();

    let device_did = did.clone();
    stream::on_status(move |nonce| {
        let doc = StatusDocument {
            device_did: &device_did,
            maker_did: None,
            model: "host camera",
            node_id: None,
            adoption: None,
            extra: serde_json::Map::new(),
        };
        match nonce {
            None => doc.plain().to_string(),
            Some(n) => doc
                .signed(&n, |h| identity::sign_prehash(h).unwrap())
                .unwrap()
                .to_string(),
        }
    });
    assert!(stream::listen_gated(0), "{:?}", last_error());
    let target = format!("127.0.0.1:{}", stream::listen_addr().unwrap().port());

    let plain = get(&target, "/v1/status");
    assert!(plain.starts_with("HTTP/1.1 200"), "{plain}");
    let plain = body(&plain);
    assert_eq!(plain["pair"], "ready");
    assert_eq!(plain["device_did"], did.as_str());
    assert!(plain.get("device_sig").is_none());

    let nonce: [u8; 32] = core::array::from_fn(|i| 0xa0 ^ i as u8);
    let hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let signed = body(&get(&target, &format!("/v1/status?nonce={hex}")));
    let sig: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signed["device_sig"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(StatusProof::unadopted(&nonce, &did).verify(&sig), Ok(()));
    // another nonce's proof does not take this signature
    assert!(StatusProof::unadopted(&[0; 32], &did).verify(&sig).is_err());

    assert!(get(&target, "/v1/status?nonce=abc").starts_with("HTTP/1.1 400"));
    // the page is still gated
    assert!(get(&target, "/").starts_with("HTTP/1.1 403"));
    let _ = std::fs::remove_dir_all(&dir);
}
