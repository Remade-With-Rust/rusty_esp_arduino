//! `identity::with_store`: the boot-time steps that write beside the device
//! key get the same store `begin` loaded it from, and what they write is
//! there on the next boot. One test per binary — the facade's state is
//! global.

use rusty_esp_arduino::host::HostBoard;
use rusty_esp_arduino::{board, identity};

#[test]
fn the_store_begin_took_is_lent_and_keeps_what_is_written() {
    let dir = std::env::temp_dir().join(format!("janus-identity-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // before `begin`: nothing to lend
    assert!(identity::with_store(|_, _| ()).is_none());

    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(identity::begin(None));
    let did = identity::did().expect("a did");

    let seen = identity::with_store(|kv, d| {
        kv.put("mid.test", b"written at boot").expect("put");
        d.to_owned()
    });
    assert_eq!(
        seen.as_deref(),
        Some(did.as_str()),
        "the DID comes with the store"
    );

    // the next boot, from the same store: the key and what was written
    identity::end();
    board::uninstall();
    board::install(HostBoard::new().unpaced().with_store(&dir));
    assert!(identity::begin(None));
    assert_eq!(identity::did().as_deref(), Some(did.as_str()));
    let read = identity::with_store(|kv, _| {
        let mut buf = [0u8; 32];
        let n = kv.get("mid.test", &mut buf).expect("get").expect("present");
        buf[..n].to_vec()
    });
    assert_eq!(read.as_deref(), Some(&b"written at boot"[..]));

    identity::end();
    board::uninstall();
    let _ = std::fs::remove_dir_all(&dir);
}
