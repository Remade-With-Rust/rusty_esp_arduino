//! `setup` once, `loop` forever — and the bounded form a test needs.

use std::time::Duration;

/// Run `setup`, then `loop_once` forever. Never returns.
pub fn run(setup: fn(), loop_once: fn()) -> ! {
    setup();
    loop {
        loop_once();
    }
}

/// Run `setup`, then `loop_once` exactly `iterations` times.
pub fn run_for(setup: fn(), loop_once: fn(), iterations: u64) {
    setup();
    for _ in 0..iterations {
        loop_once();
    }
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
