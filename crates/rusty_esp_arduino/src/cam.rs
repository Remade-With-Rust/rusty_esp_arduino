//! The camera, Arduino-shaped: `begin` once, `grab` in the loop.

use std::sync::{Mutex, PoisonError};

use crate::board::{self, Jpeg};
use crate::error;

/// What the camera is asked for. JPEG out, always: the function packages
/// carry raw frames, a sketch carries pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// Frames per second the board should pace at.
    pub fps: u32,
    /// JPEG quality, 1–100.
    pub quality: u8,
}

impl Config {
    /// 320×240 at 15 fps, quality 80: the frame every Janus board can do.
    #[must_use]
    pub const fn qvga_jpeg() -> Self {
        Config {
            width: 320,
            height: 240,
            fps: 15,
            quality: 80,
        }
    }

    /// 640×480 at 10 fps, quality 80.
    #[must_use]
    pub const fn vga_jpeg() -> Self {
        Config {
            width: 640,
            height: 480,
            fps: 10,
            quality: 80,
        }
    }

    /// The same frame at another rate.
    #[must_use]
    pub const fn at_fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

/// Start the camera with `config`. `false` (and [`crate::last_error`]) when
/// the board refuses or none is installed.
pub fn begin(config: Config) -> bool {
    let r = board::with(|b| b.cam_begin(&config));
    if r.is_ok() {
        *CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config);
    }
    error::ok(r)
}

/// The next frame, if one is ready. `None` with no error recorded means
/// "not yet"; `None` with [`crate::last_error`] set means something is wrong.
pub fn grab() -> Option<Jpeg> {
    error::some(board::with(|b| b.cam_grab()))
}

/// The configuration `begin` was given, once it succeeded.
#[must_use]
pub fn config() -> Option<Config> {
    *CONFIG.lock().unwrap_or_else(PoisonError::into_inner)
}
