#![forbid(unsafe_code)]
//! `rusty_esp_arduino` — the sketch a maker types, over the Janus function
//! packages.
//!
//! ```no_run
//! use rusty_esp_arduino::prelude::*;
//!
//! fn setup() {
//!     board::install(HostBoard::from_env());
//!     wifi::begin("ssid", "psk");
//!     cam::begin(cam::Config::qvga_jpeg());
//!     mic::begin(mic::Config::pcm16_16k());
//!     stream::listen(80);
//! }
//!
//! fn loop_once() {
//!     if let Some(frame) = cam::grab() {
//!         stream::push_jpeg(&frame);
//!     }
//!     if let Some(pcm) = mic::read() {
//!         stream::push_pcm(&pcm);
//!     }
//! }
//!
//! fn main() {
//!     sketch::run(setup, loop_once);
//! }
//! ```
//!
//! **Thin by law.** Every function here is one call through the [`board::Board`]
//! seam into a function package: the camera and microphone are the board's,
//! the MJPEG server and the RTP/JPEG and PCM senders are
//! `rusty_esp_video-esp`'s, the WAV reader is `rusty_esp_audio-core`'s. The
//! facade owns no codec, no socket format and no pixel loop. What it adds is
//! the Arduino shape: free functions, a board installed once, `begin` before
//! use, `Option` out, and the reason for a `None` one call away in
//! [`last_error`].
//!
//! The same sketch runs on the laptop against [`host::HostBoard`] (feature
//! `host`, on by default) and on a chip against that chip's board, which is
//! the board half of this package's plan. Nothing in a sketch names the chip.

pub mod board;
pub mod cam;
pub mod error;
#[cfg(feature = "host")]
pub mod host;
pub mod mic;
pub mod prelude;
pub mod sketch;
pub mod stream;
pub mod wifi;

pub use error::{Error, last_error};

/// Crate version, for logs and capability manifests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
