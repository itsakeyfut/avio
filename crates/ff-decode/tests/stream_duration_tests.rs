//! A decoder reports the duration of the stream it opened, not the container's (#1861).
//!
//! A container's duration is its **longest stream**, so reading it as one stream's meant
//! that a file whose audio outran its video said more time than the video held. That is
//! routine rather than exotic: an AAC encoder emits whole 1024-sample frames, so 15 video
//! frames at 30 fps (0.5s) sit beside 24 audio packets (0.512s).
//!
//! The two reference files are the same content with the stream lengths swapped, half a
//! second against two seconds, so each one is a case where the container says four times
//! what one of its streams holds. The gap is deliberately that wide: at 12ms the defect
//! is indistinguishable from the rounding an encoder leaves behind, and a test that could
//! not tell them apart would not have found this.
//!
//! Both were generated once by `tools/gen_test_assets.rs` and committed, so `ff-decode`
//! does not take a dev-dependency on `ff-encode` (`docs/rules/test.md`).

use std::path::PathBuf;
use std::time::Duration;

use ff_decode::{AudioDecoder, VideoDecoder};

/// One AAC frame is 1024 samples, about 21ms at 48 kHz, and neither encoder nor muxer can
/// place a stream boundary more finely than that. Two frames of slack keeps the
/// assertions honest while leaving the 1.5s error they guard against nowhere to hide.
const TOLERANCE_SECS: f64 = 0.05;

fn asset(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("assets/test")
        .join(name)
}

/// `None` (with a printed reason) when the asset is absent, matching
/// `negative_pts_tests.rs`: a missing committed fixture is a checkout that needs the
/// generator run, not a regression.
fn asset_or_skip(name: &str) -> Option<PathBuf> {
    let path = asset(name);
    if path.exists() {
        return Some(path);
    }
    println!(
        "Skipping: reference asset not found at {} \
         (run `cargo run --manifest-path tools/Cargo.toml` to regenerate)",
        path.display()
    );
    None
}

#[test]
fn video_duration_should_report_the_video_stream_not_the_container() {
    let Some(path) = asset_or_skip("audio_longer_than_video.mp4") else {
        return;
    };
    let decoder = match VideoDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: this build cannot open the asset: {e}");
            return;
        }
    };
    let secs = decoder.duration().as_secs_f64();
    assert!(
        (secs - 0.5).abs() < TOLERANCE_SECS,
        "the video stream holds 0.5s; the container says about 2s because the audio \
         does. Got {secs}s"
    );
}

#[test]
fn audio_duration_should_report_the_audio_stream_not_the_container() {
    let Some(path) = asset_or_skip("video_longer_than_audio.mp4") else {
        return;
    };
    let decoder = match AudioDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: this build cannot open the asset: {e}");
            return;
        }
    };
    let secs = decoder.duration().as_secs_f64();
    assert!(
        (secs - 0.512).abs() < TOLERANCE_SECS,
        "the audio stream holds 0.512s; the container says 2s because the video does. \
         Got {secs}s"
    );
}

/// The container's duration is the documented fallback, not dead code: Matroska leaves
/// `AVStream.duration` unset, so this file has no per-stream reading at all and the
/// container's 0.533s is the only one available. Without the fallback a file that
/// reported a length before would report none now, which is the regression the fallback
/// exists to prevent.
#[test]
fn a_stream_without_its_own_duration_should_fall_back_to_the_container() {
    let Some(path) = asset_or_skip("negative_first_pts.mkv") else {
        return;
    };
    let decoder = match AudioDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: this build cannot open the asset: {e}");
            return;
        }
    };
    let reported = decoder.duration_opt();
    assert!(
        reported.is_some(),
        "a stream carrying no duration must still report the container's, not nothing"
    );
    let secs = reported.unwrap_or(Duration::ZERO).as_secs_f64();
    assert!(
        (secs - 0.533).abs() < TOLERANCE_SECS,
        "the fallback is the container's 0.533s, got {secs}s"
    );
}
