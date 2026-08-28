//! Adjust audio volume or apply an equalizer using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `volume`     — adjust loudness by a gain in dB (`+` = louder, `-` = quieter)
//!   `equalizer`  — boost or cut a specific peak frequency band
//!   `low-shelf`  — boost or cut all frequencies below a corner frequency
//!   `high-shelf` — boost or cut all frequencies above a corner frequency
//!   `afade-in`   — fade in audio from silence at the start of the clip
//!   `afade-out`  — fade out audio to silence at the end of the clip
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_filters --features pipeline -- \
//!   --input   input.mp4     \
//!   --output  filtered.mp4  \
//!   --effect  volume        \
//!   [--db    6.0]            # gain in dB for volume (default: 6.0)
//!   [--freq  1000.0]         # center/corner frequency in Hz (default: 1000.0)
//!   [--gain  3.0]            # gain in dB for equalizer/shelf (default: 3.0)
//!   [--start 0.0]            # afade: start time in seconds (default: 0.0)
//!   [--duration 1.0]         # afade: fade duration in seconds (default: 1.0)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::{EqBand, FilterGraphBuilder};
use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, Progress};

fn render_progress(p: &Progress) {
    match p.percent() {
        Some(pct) => {
            let bar_width = 20usize;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let bar = "=".repeat(filled) + &" ".repeat(bar_width - filled);
            print!("\r{pct:5.1}%  [{bar}]    ");
        }
        None => {
            print!("\r{} frames    ", p.frames_processed);
        }
    }
    let _ = io::stdout().flush();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut effect = None::<String>;
    let mut db: f64 = 6.0;
    let mut freq: f64 = 1000.0;
    let mut gain: f64 = 3.0;
    let mut start: f64 = 0.0;
    let mut duration: f64 = 1.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--db" => {
                let v = args.next().unwrap_or_default();
                db = v.parse().unwrap_or(6.0);
            }
            "--freq" => {
                let v = args.next().unwrap_or_default();
                freq = v.parse().unwrap_or(1000.0);
            }
            "--gain" => {
                let v = args.next().unwrap_or_default();
                gain = v.parse().unwrap_or(3.0);
            }
            "--start" => start = args.next().unwrap_or_default().parse().unwrap_or(0.0),
            "--duration" => {
                duration = args.next().unwrap_or_default().parse().unwrap_or(1.0);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: audio_filters --input <file> --output <file> \
             --effect volume|equalizer|low-shelf|high-shelf|afade-in|afade-out [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!(
            "--effect is required \
             (volume|equalizer|low-shelf|high-shelf|afade-in|afade-out)"
        );
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    // ── Build filter graph ────────────────────────────────────────────────────

    let filter_result = match effect.as_str() {
        "volume" => {
            let sign = if db >= 0.0 { "+" } else { "" };
            println!("Input:   {in_name}");
            println!("Effect:  volume  ({sign}{db} dB)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().volume(db).build()
        }
        "equalizer" => {
            println!("Input:   {in_name}");
            println!("Effect:  equalizer  (freq={freq} Hz  gain={gain:+.1} dB)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .equalizer(vec![EqBand::Peak {
                    freq_hz: freq,
                    gain_db: gain,
                    q: 1.0,
                }])
                .build()
        }
        "low-shelf" => {
            println!("Input:   {in_name}");
            println!("Effect:  low_shelf  (freq={freq} Hz  gain={gain:+.1} dB)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .equalizer(vec![EqBand::LowShelf {
                    freq_hz: freq,
                    gain_db: gain,
                    slope: 0.5,
                }])
                .build()
        }
        "high-shelf" => {
            println!("Input:   {in_name}");
            println!("Effect:  high_shelf  (freq={freq} Hz  gain={gain:+.1} dB)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .equalizer(vec![EqBand::HighShelf {
                    freq_hz: freq,
                    gain_db: gain,
                    slope: 0.5,
                }])
                .build()
        }
        "afade-in" => {
            println!("Input:   {in_name}");
            println!("Effect:  afade_in  (start={start:.1}s  duration={duration:.1}s)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().afade_in(start, duration).build()
        }
        "afade-out" => {
            println!("Input:   {in_name}");
            println!("Effect:  afade_out  (start={start:.1}s  duration={duration:.1}s)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().afade_out(start, duration).build()
        }
        other => {
            eprintln!(
                "Unknown effect '{other}' \
                 (try volume, equalizer, low-shelf, high-shelf, afade-in, afade-out)"
            );
            process::exit(1);
        }
    };

    let filter = match filter_result {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    println!();

    // ── Assemble pipeline ─────────────────────────────────────────────────────

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .crf(23)
        .build();

    let pipeline = match Pipeline::builder()
        .input(&input)
        .filter(filter)
        .output(&output, config)
        .on_progress(|p: &Progress| {
            render_progress(p);
            true
        })
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = pipeline.run() {
        println!();
        eprintln!("Error: {e}");
        process::exit(1);
    }

    println!();

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

    println!("Done. {out_name}  {size_str}");
}
