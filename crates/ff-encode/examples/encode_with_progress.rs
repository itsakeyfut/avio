//! Encode video with a structured progress callback and frame-limit cancellation.
//!
//! Demonstrates:
//! - `EncodeProgressCallback` trait — implement `on_progress()` on a custom struct
//! - `EncodeProgressCallback::should_cancel()` — signal cancellation from within
//!   the callback (no external flag needed)
//! - `EncodeProgress::frames_encoded` / `current_fps` / `elapsed` — progress fields
//! - `VideoEncoder::create().progress_callback()` — attach a trait object to the encoder
//!
//! The simpler closure form (`on_progress(|p| …)`) is shown in `transcode.rs`.
//! This example shows the trait-based form which supports state and cancellation.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example encode_with_progress --features "decode encode" -- \
//!   --input       input.mp4   \
//!   --output      output.mp4  \
//!   [--max-frames 100]        # stop after N frames (default: encode all)
//! ```

use std::{
    io::Write as _,
    path::Path,
    process,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use ff_decode::VideoDecoder;
use ff_encode::{EncodeError, EncodeProgress, EncodeProgressCallback, VideoEncoder};
use ff_format::VideoCodec;

// ── Custom progress handler ───────────────────────────────────────────────────
//
// Implement EncodeProgressCallback on a concrete struct so that state
// (the frame counter, the optional limit) travels with the callback.
//
// FrameLimitProgress shows the simplest form: print progress, no cancellation.

#[allow(dead_code)]
struct FrameLimitProgress {
    /// Maximum number of frames to encode, or None for no limit.
    max_frames: Option<u64>,
}

impl EncodeProgressCallback for FrameLimitProgress {
    /// Called by the encoder after each encoded frame.
    ///
    /// Prints the live progress line with frame count, encoding speed (fps),
    /// and wall-clock elapsed time.
    fn on_progress(&mut self, progress: &EncodeProgress) {
        print!(
            "\rframes={:>6}  fps={:>6.1}  elapsed={:.1}s",
            progress.frames_encoded,
            progress.current_fps,
            progress.elapsed.as_secs_f64(),
        );
        // Flush the line so the counter updates in-place.
        let _ = std::io::stdout().flush();
    }

    /// Return `true` to cancel encoding at the next frame boundary.
    ///
    /// The encoder calls this after every `on_progress` invocation.
    /// When `true` is returned the encoder stops and `push_video` / `finish`
    /// return `EncodeError::Cancelled`.
    fn should_cancel(&self) -> bool {
        // We use frames_encoded from the last on_progress call — but since
        // should_cancel() doesn't receive progress, we track it ourselves.
        // For this example we re-implement the counter inside should_cancel
        // using a shared atomic, which is the idiomatic pattern.
        false // cancellation is via the atomic counter below
    }
}

// A second callback that actually cancels via an atomic frame counter.
// This shows the common production pattern: hold a cancel flag in the struct.

struct CancellableProgress {
    max_frames: Option<u64>,
    frames_seen: Arc<AtomicU64>,
    cancel_flag: Arc<AtomicBool>,
}

impl EncodeProgressCallback for CancellableProgress {
    fn on_progress(&mut self, progress: &EncodeProgress) {
        self.frames_seen
            .store(progress.frames_encoded, Ordering::Relaxed);

        print!(
            "\rframes={:>6}  fps={:>6.1}  elapsed={:.1}s",
            progress.frames_encoded,
            progress.current_fps,
            progress.elapsed.as_secs_f64(),
        );
        let _ = std::io::stdout().flush();

        if let Some(max) = self.max_frames
            && progress.frames_encoded >= max
        {
            self.cancel_flag.store(true, Ordering::Relaxed);
        }
    }

    fn should_cancel(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut max_frames: Option<u64> = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--max-frames" => {
                let v = args.next().unwrap_or_default();
                max_frames = v.parse().ok();
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: encode_with_progress --input <file> --output <file> [--max-frames N]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Probe source ──────────────────────────────────────────────────────────

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };
    let width = probe.width();
    let height = probe.height();
    let fps = probe.frame_rate();
    drop(probe);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:  {in_name}  {width}×{height}  {fps:.2} fps");
    match max_frames {
        Some(n) => println!("Output: {out_name}  limit={n} frames"),
        None => println!("Output: {out_name}  (all frames)"),
    }
    println!();

    // ── Shared state for cancellation ─────────────────────────────────────────

    let frames_seen = Arc::new(AtomicU64::new(0));
    let cancel_flag = Arc::new(AtomicBool::new(false));

    let callback = CancellableProgress {
        max_frames,
        frames_seen: Arc::clone(&frames_seen),
        cancel_flag: Arc::clone(&cancel_flag),
    };

    // ── Build encoder with trait-based progress callback ──────────────────────
    //
    // progress_callback() accepts any type implementing EncodeProgressCallback.
    // Use on_progress() (the simpler form) when you only need a closure.

    let mut encoder = match VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(VideoCodec::H264)
        .progress_callback(callback)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    // ── Decode loop ───────────────────────────────────────────────────────────

    let mut decoder = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("\nDecode error: {e}");
                process::exit(1);
            }
        };

        match encoder.push_video(&frame) {
            Ok(()) => {}
            // EncodeError::Cancelled is returned when should_cancel() fired.
            Err(EncodeError::Cancelled) => break,
            Err(e) => {
                eprintln!("\nEncode error: {e}");
                process::exit(1);
            }
        }
    }

    // ── Finalise ──────────────────────────────────────────────────────────────

    let cancelled = match encoder.finish() {
        Ok(()) => false,
        Err(EncodeError::Cancelled) => true,
        Err(e) => {
            eprintln!("\nError finalising: {e}");
            process::exit(1);
        }
    };

    println!(); // end the progress line

    let final_frames = frames_seen.load(Ordering::Relaxed);

    if cancelled {
        println!("Encoding cancelled after {final_frames} frames.");
    } else {
        let size_str = match std::fs::metadata(&output) {
            Ok(m) => {
                #[allow(clippy::cast_precision_loss)]
                let kb = m.len() as f64 / 1024.0;
                if kb < 1024.0 {
                    format!("{kb:.0} KB")
                } else {
                    format!("{:.1} MB", kb / 1024.0)
                }
            }
            Err(_) => "(unknown size)".to_string(),
        };
        println!("Done. {out_name}  {size_str}  {final_frames} frames");
    }
}
