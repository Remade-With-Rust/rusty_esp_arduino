//! `setup` once, `loop` forever — and the bounded form a test needs.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::board::{self, BootRecord};
use crate::error::record;

/// How long a loop pass may take before the board reboots: the sketch's
/// promise to its watchdog. The board's own timeout applies where it is
/// configured; this is what the sketch asks for.
pub const LIVENESS_TIMEOUT_S: u32 = 30;

/// This boot's record, read once when `run` starts.
static BOOT: Mutex<Option<BootRecord>> = Mutex::new(None);

/// Before `setup`: the boot record, and the watchdog armed. Both only with a
/// board installed; without one there is nothing to ask and nothing to arm.
fn begin_run() {
    if !board::installed() {
        return;
    }
    if let Ok(Some(r)) = board::with(|b| b.boot_record()) {
        *BOOT.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
    }
    if let Err(e) = board::with(|b| b.liveness_begin(LIVENESS_TIMEOUT_S)) {
        record(e);
    }
}

/// After a pass: the watchdog fed.
fn end_pass() {
    if board::installed() {
        let _ = board::with(|b| b.liveness_feed());
    }
}

/// Run `setup`, then `loop_once` forever, feeding the board's liveness
/// watchdog after every pass. Never returns.
pub fn run(setup: fn(), loop_once: fn()) -> ! {
    begin_run();
    setup();
    loop {
        loop_once();
        end_pass();
    }
}

/// Run `setup`, then `loop_once` exactly `iterations` times, the way `run`
/// would.
pub fn run_for(setup: fn(), loop_once: fn(), iterations: u64) {
    begin_run();
    setup();
    for _ in 0..iterations {
        loop_once();
        end_pass();
    }
}

/// Why this boot happened and how many the board has counted, once `run`
/// has read it. The sketch prints it; `mesh::begin` advertises it.
#[must_use]
pub fn boot_record() -> Option<BootRecord> {
    *BOOT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sleep for `ms` milliseconds.
pub fn delay(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Restart the board, as the RESET button would.
///
/// A device that has just been given its network wants this: it comes back
/// in the configuration it will live in — reading the network from the
/// owner's settings, with no provisioning radio running and its memory
/// free — rather than carrying whatever provisioning left behind. On a chip
/// this never returns. `false` (and [`crate::last_error`]) when the board
/// cannot restart itself, and then the sketch keeps running.
pub fn restart() -> bool {
    crate::error::ok(crate::board::with(|b| b.restart()))
}

/// Milliseconds since the board started.
#[must_use]
pub fn millis() -> u64 {
    crate::board::millis()
}
