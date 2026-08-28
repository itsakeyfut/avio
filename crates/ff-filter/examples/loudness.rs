//! Measure EBU R128 integrated loudness, loudness range, and true peak.
//!
//! Uses [`LoudnessMeter`] to run the `ebur128` filter over an entire audio or
//! video file and report the perceptual loudness metrics used in broadcast
//! normalization workflows.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example loudness --features decode,filter -- --input audio.mp3
//! cargo run --example loudness --features decode,filter -- --input video.mp4
//! ```

use std::process;

use ff_filter::LoudnessMeter;

fn fmt_db(db: f32) -> String {
    if db == f32::NEG_INFINITY {
        "-inf".to_string()
    } else {
        format!("{db:+.1}")
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: loudness --input <audio-or-video>");
        process::exit(1);
    });

    println!("Measuring loudness: {input}");
    println!();

    let result = LoudnessMeter::new(&input).measure().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    println!("EBU R128 Loudness Results");
    println!("{}", "-".repeat(32));
    println!(
        "  Integrated loudness: {} LUFS",
        fmt_db(result.integrated_lufs)
    );
    println!("  Loudness range (LRA): {:.1} LU", result.lra);
    println!(
        "  True peak:            {} dBTP",
        fmt_db(result.true_peak_dbtp)
    );
    println!();

    // Indicate compliance with common broadcast targets.
    let integrated = result.integrated_lufs;
    if integrated != f32::NEG_INFINITY {
        println!("Compliance:");
        let youtube_ok = (-14.5..=-13.5).contains(&integrated);
        let spotify_ok = (-14.5..=-13.5).contains(&integrated);
        let broadcast_ok = (-24.0..=-22.0).contains(&integrated);
        println!(
            "  YouTube / Spotify target (-14 LUFS): {}",
            if youtube_ok || spotify_ok {
                "✓ within ±0.5 LU"
            } else {
                "outside target"
            }
        );
        println!(
            "  EBU R128 broadcast target (-23 LUFS): {}",
            if broadcast_ok {
                "✓ within ±1 LU"
            } else {
                "outside target"
            }
        );
    }
}
