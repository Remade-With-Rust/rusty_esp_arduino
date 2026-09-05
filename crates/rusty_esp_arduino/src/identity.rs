//! The device's own `did:mata`: minted once from the board's entropy, kept
//! in the board's store, the same on every boot — `rusty_esp_mid` behind
//! the facade's thin-by-law rule. `begin` is the one call a sketch makes.
//!
//! The key lives under the same store key (`mid.devkey`) the mesh node
//! reads, so a sketch that later calls `mesh::begin` presents the identity
//! minted here, not a second one.

use std::sync::{Mutex, PoisonError};

use rusty_esp_core::hal::{Kv, Rng};
use rusty_esp_mid_core::did::{Did, MAX_DID_LEN};
use rusty_esp_mid_core::key::DeviceKey;

use crate::board;
use crate::error::{Error, Result, record};

/// The label the device key is generated under.
pub const DEVICE_ID: &str = "janus";

/// A boxed [`Kv`] as a sized one, for the seams that take `&mut impl Kv`.
pub(crate) struct DynKv(pub(crate) Box<dyn Kv + Send>);

impl Kv for DynKv {
    fn get(&self, key: &str, out: &mut [u8]) -> rusty_esp_core::error::Result<Option<usize>> {
        self.0.get(key, out)
    }

    fn put(&mut self, key: &str, value: &[u8]) -> rusty_esp_core::error::Result<()> {
        self.0.put(key, value)
    }

    fn remove(&mut self, key: &str) -> rusty_esp_core::error::Result<bool> {
        self.0.remove(key)
    }
}

/// A boxed [`Rng`] as a sized one.
pub(crate) struct DynRng(pub(crate) Box<dyn Rng + Send>);

impl Rng for DynRng {
    fn fill(&mut self, buf: &mut [u8]) -> rusty_esp_core::error::Result<()> {
        self.0.fill(buf)
    }
}

struct State {
    did: String,
    maker: Option<String>,
    /// The store and entropy `begin` took from the board, until the mesh
    /// takes them (the node owns the store; the key in it is the same).
    #[cfg_attr(not(feature = "mesh"), allow(dead_code))]
    store: Option<(Box<dyn Kv + Send>, Box<dyn Rng + Send>)>,
}

static IDENTITY: Mutex<Option<State>> = Mutex::new(None);

fn try_begin(maker: Option<&str>) -> Result<String> {
    if let Some(m) = maker {
        Did::parse(m).map_err(|_| Error::Io(format!("maker {m:?} is not a did:mata")))?;
    }
    let (kv, rng) = board::with(|b| {
        let kv = b
            .take_kv()
            .ok_or(Error::Missing("store for the device key"))?;
        let rng = b.take_rng().ok_or(Error::Missing("entropy source"))?;
        Ok((kv, rng))
    })?;
    let mut kv = DynKv(kv);
    let mut rng = DynRng(rng);
    let key = DeviceKey::load_or_generate(&mut kv, &mut rng, DEVICE_ID)
        .map_err(|e| Error::Io(format!("device key: {e:?}")))?;
    let mut buf = [0u8; MAX_DID_LEN];
    let did = key
        .did()
        .write(&mut buf)
        .map_err(|e| Error::Io(format!("did: {e:?}")))?
        .to_owned();
    *IDENTITY.lock().unwrap_or_else(PoisonError::into_inner) = Some(State {
        did: did.clone(),
        maker: maker.map(str::to_owned),
        store: Some((kv.0, rng.0)),
    });
    Ok(did)
}

/// Mint or load the device's `did:mata` from the board's store. `maker` is
/// the DID allowed to sign this device's updates, if any. `false` — and
/// [`crate::last_error`] says why — without a board, without a store or an
/// entropy source on it, or with a maker that is not a `did:mata`.
pub fn begin(maker: Option<&str>) -> bool {
    match try_begin(maker) {
        Ok(_) => true,
        Err(e) => {
            record(e);
            false
        }
    }
}

/// The device's DID once `begin` succeeded.
#[must_use]
pub fn did() -> Option<String> {
    IDENTITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|s| s.did.clone())
}

/// The maker DID `begin` was given, if any.
#[must_use]
pub fn maker() -> Option<String> {
    IDENTITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|s| s.maker.clone())
}

/// Whether `begin` succeeded.
#[must_use]
pub fn begun() -> bool {
    did().is_some()
}

/// The store and entropy `begin` took from the board, handed over once.
#[cfg_attr(not(feature = "mesh"), allow(dead_code))]
pub(crate) fn take_store() -> Option<(Box<dyn Kv + Send>, Box<dyn Rng + Send>)> {
    IDENTITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
        .and_then(|s| s.store.take())
}

/// Forget the identity (tests; a sketch that ends). The store keeps the key,
/// so the next `begin` on the same store yields the same DID.
pub fn end() {
    *IDENTITY.lock().unwrap_or_else(PoisonError::into_inner) = None;
}
