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

/// Milliseconds since the board started.
#[must_use]
pub fn millis() -> u64 {
    crate::board::millis()
}
