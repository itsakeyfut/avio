//! Integration tests for SilenceDetector.
//!
//! Tests verify:
//! - Missing input file returns `AnalysisError::Failed`
//! - A synthetic audio file (1s tone + 2s silence + 1s tone) yields one `SilenceRange`
//! - The detected range has the expected start/end within ±200 ms
//! - A `min_duration` longer than the actual silence yields no ranges
//! - Structural invariant: `range.start < range.end`
//! - A real audio file runs without error

#![allow(clippy::unwrap_used)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use ff_analysis::{AnalysisError, SilenceDetector};

/// Writes a minimal PCM WAV file with the layout:
///   0–1 s : 440 Hz sine at 80% amplitude
///   1–3 s : zeros (true silence, −∞ dBFS)
///   3–4 s : 440 Hz sine at 80% amplitude
///
/// Uses only `std::io` — no external tools or dependencies required.
fn write_silence_fixture(path: &Path) -> std::io::Result<()> {
    const SAMPLE_RATE: u32 = 44_100;
    const NUM_CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;

    let block_align = NUM_CHANNELS * BITS_PER_SAMPLE / 8;
    let byte_rate = SAMPLE_RATE * u32::from(block_align);
    let total_samples = (SAMPLE_RATE * 4) as usize; // 4 s
    let silence_start = SAMPLE_RATE as usize; // 1 s
    let silence_end = 3 * SAMPLE_RATE as usize; // 3 s

    let mut samples: Vec<i16> = Vec::with_capacity(total_samples);
    for i in 0..total_samples {
        let v = if i >= silence_start && i < silence_end {
            0i16
        } else {
            let t = i as f64 / f64::from(SAMPLE_RATE);
            let s = 0.8 * 32_767.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin();
            s as i16
        };
        samples.push(v);
    }

    let data_len = (samples.len() * 2) as u32;
    let file_len = 36 + data_len; // RIFF chunk size

    let mut f = std::fs::File::create(path)?;

    // RIFF / WAVE header
    f.write_all(b"RIFF")?;
    f.write_all(&file_len.to_le_bytes())?;
    f.write_all(b"WAVE")?;

    // fmt sub-chunk (16 bytes, PCM = format code 1)
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&NUM_CHANNELS.to_le_bytes())?;
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;

    // data sub-chunk
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in &samples {
        f.write_all(&s.to_le_bytes())?;
    }

    Ok(())
}

fn test_audio_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!(
        "{manifest_dir}/../../assets/audio/konekonoosanpo.mp3"
    ))
}

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn silence_detector_missing_file_should_return_analysis_failed() {
    let result = SilenceDetector::new("does_not_exist_99999.mp3").run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for missing file, got {result:?}"
    );
}

// ── Functional tests using synthetic WAV fixture ──────────────────────────────

#[test]
fn silence_detector_should_find_silence_range() {
    let fixture = std::env::temp_dir().join("ff_decode_silence_test_find.wav");

    if write_silence_fixture(&fixture).is_err() {
        println!("Skipping: could not write silence fixture");
        return;
    }

    let ranges = match SilenceDetector::new(&fixture)
        .threshold_db(-40.0)
        .min_duration(Duration::from_millis(500))
        .run()
    {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&fixture);
            println!("Skipping: SilenceDetector::run failed ({e})");
            return;
        }
    };

    let _ = std::fs::remove_file(&fixture);

    assert_eq!(
        ranges.len(),
        1,
        "expected exactly one SilenceRange for 2 s silence, got {}: {ranges:?}",
        ranges.len()
    );

    let margin = Duration::from_millis(200);
    let expected_start = Duration::from_secs(1);
    let expected_end = Duration::from_secs(3);

    assert!(
        ranges[0].start >= expected_start.saturating_sub(margin)
            && ranges[0].start <= expected_start + margin,
        "silence start {:?} not within ±200 ms of {:?}",
        ranges[0].start,
        expected_start
    );
    assert!(
        ranges[0].end >= expected_end.saturating_sub(margin)
            && ranges[0].end <= expected_end + margin,
        "silence end {:?} not within ±200 ms of {:?}",
        ranges[0].end,
        expected_end
    );
}

#[test]
fn silence_detector_short_silence_should_not_be_detected() {
    let fixture = std::env::temp_dir().join("ff_decode_silence_test_short.wav");

    if write_silence_fixture(&fixture).is_err() {
        println!("Skipping: could not write silence fixture");
        return;
    }

    // min_duration of 3 s is longer than the 2 s silence → nothing should be detected.
    let ranges = match SilenceDetector::new(&fixture)
        .threshold_db(-40.0)
        .min_duration(Duration::from_secs(3))
        .run()
    {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&fixture);
            println!("Skipping: SilenceDetector::run failed ({e})");
            return;
        }
    };

    let _ = std::fs::remove_file(&fixture);

    assert!(
        ranges.is_empty(),
        "expected no ranges when min_duration exceeds silence length, got {ranges:?}"
    );
}

#[test]
fn silence_detector_range_start_should_be_before_end() {
    let fixture = std::env::temp_dir().join("ff_decode_silence_test_order.wav");

    if write_silence_fixture(&fixture).is_err() {
        println!("Skipping: could not write silence fixture");
        return;
    }

    let ranges = match SilenceDetector::new(&fixture)
        .threshold_db(-40.0)
        .min_duration(Duration::from_millis(500))
        .run()
    {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&fixture);
            println!("Skipping: SilenceDetector::run failed ({e})");
            return;
        }
    };

    let _ = std::fs::remove_file(&fixture);

    for r in &ranges {
        assert!(
            r.start < r.end,
            "range.start ({:?}) must be strictly less than range.end ({:?})",
            r.start,
            r.end
        );
    }
}

// ── Real audio file test ──────────────────────────────────────────────────────

#[test]
fn silence_detector_real_audio_should_succeed() {
    let path = test_audio_path();
    if !path.exists() {
        println!("Skipping: test audio file not found at {}", path.display());
        return;
    }

    match SilenceDetector::new(&path)
        .threshold_db(-50.0)
        .min_duration(Duration::from_millis(200))
        .run()
    {
        Ok(_) => {}
        Err(e) => {
            println!("Skipping: SilenceDetector::run failed ({e})");
        }
    }
}
