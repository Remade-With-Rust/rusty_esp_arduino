//! The sketch from the plan, on the laptop.
//!
//! ```sh
//! cargo run -p rusty_esp_arduino --example cam_mic            # http://127.0.0.1:8080/
//! JANUS_CAM_DIR=./frames JANUS_WAV=./voice.wav cargo run -p rusty_esp_arduino --example cam_mic
//! JANUS_RTP_DEST=127.0.0.1:5004 JANUS_PCM_DEST=127.0.0.1:5006 cargo run -p rusty_esp_arduino --example cam_mic
//! ```
//!
//! Then open the page in a browser, or `ffmpeg -i http://127.0.0.1:8080/stream
//! -frames:v 30 -f null -`. With `JANUS_RTP_DEST` set, the RTP/JPEG receive
//! recipe in `rusty_esp_video`'s README plays the same frames.

use rusty_esp_arduino::prelude::*;

fn setup() {
    board::install(HostBoard::from_env());
    wifi::begin("home", "not-needed-on-a-laptop");
    if !cam::begin(cam::Config::qvga_jpeg()) {
        eprintln!("camera: {:?}", last_error());
    }
    if !mic::begin(mic::Config::pcm16_16k()) {
        eprintln!("microphone: {:?}", last_error());
    }
    let port = std::env::var("JANUS_HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    if stream::listen(port) {
        println!(
            "serving http://127.0.0.1:{port}/  (ip {:?})",
            wifi::local_ip()
        );
    } else {
        eprintln!("listen: {:?}", last_error());
    }
    if let Ok(dest) = std::env::var("JANUS_RTP_DEST") {
        println!("rtp/jpeg -> {dest}: {}", stream::rtp_to(dest.as_str()));
    }
    if let Ok(dest) = std::env::var("JANUS_PCM_DEST") {
        println!("pcm -> {dest}: {}", stream::pcm_to(dest.as_str()));
    }
}

fn loop_once() {
    if let Some(frame) = cam::grab() {
        stream::push_jpeg(&frame);
    }
    if let Some(pcm) = mic::read() {
        stream::push_pcm(&pcm);
    }
    let s = stream::stats();
    if s.jpeg_pushed % 150 == 0 && s.jpeg_pushed > 0 {
        println!(
            "{} s: {} frames, {} blocks, http {:?}",
            millis() / 1000,
            s.jpeg_pushed,
            s.pcm_pushed,
            s.http
        );
    }
}

fn main() {
    sketch::run(setup, loop_once);
}
