//! Integration tests for streams whose first timestamp is negative (#1819).
//!
//! The reference file (`assets/test/negative_first_pts.mkv`) is half a second of
//! silent AAC in Matroska.  The AAC encoder's priming delay reaches the decoder
//! as a negative timestamp on the first frame, which used to abort the process
//! inside `Duration::from_secs_f64`.  It was generated once by
//! `tools/gen_test_assets.rs` and committed to avoid a dev-dependency on
//! `ff-encode`.

use std::time::Duration;

use ff_decode::AudioDecoder;
use ff_format::SampleFormat;

fn negative_first_pts_path() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/ff-decode  →  ../../assets/test/
    manifest_dir
        .join("../..")
        .join("assets/test/negative_first_pts.mkv")
}

#[test]
fn decoding_a_negative_first_pts_should_report_a_position_instead_of_aborting() {
    let path = negative_first_pts_path();

    if !path.exists() {
        println!(
            "Skipping: reference asset not found at {}  \
             (run `cargo run --manifest-path tools/Cargo.toml` to regenerate)",
            path.display()
        );
        return;
    }

    // This build may lack the AAC decoder or the Matroska demuxer.
    let mut decoder = match AudioDecoder::open(&path)
        .output_format(SampleFormat::F32)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: AudioDecoder::build failed: {e}");
            return;
        }
    };

    let mut frames = 0usize;
    let mut previous = Duration::ZERO;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => {
                let position = decoder.position();
                assert!(
                    position >= previous,
                    "position went backwards: {previous:?} then {position:?}"
                );
                previous = position;
                frames += 1;
            }
            Ok(None) => break,
            Err(e) => panic!("decode_one returned an error after {frames} frames: {e}"),
        }
    }

    assert!(frames > 0, "the fixture decoded no frames at all");
    // The negative first timestamp clamps to zero rather than aborting, so the
    // stream still advances past its start.
    assert!(
        previous > Duration::ZERO,
        "the position never advanced past zero after {frames} frames"
    );
}
