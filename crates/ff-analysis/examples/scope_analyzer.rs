//! Analyse colour scope data for every sampled frame in a video file.
//!
//! Uses [`ScopeAnalyzer::vectorscope`] and [`ScopeAnalyzer::rgb_parade`] to
//! compute chroma scatter (Cb/Cr) and per-channel RGB waveform data.  Frames
//! are sampled at a configurable interval and a summary is printed.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example scope_analyzer -- --input video.mp4
//! cargo run --example scope_analyzer -- --input video.mp4 --interval 60
//! ```

use std::process;

use ff_analysis::{RgbParade, ScopeAnalyzer};
use ff_decode::VideoDecoder;

fn mean_f32(vals: &[f32]) -> f32 {
    if vals.is_empty() {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let len = vals.len() as f32;
    vals.iter().sum::<f32>() / len
}

fn print_vectorscope_summary(frame_idx: u64, scatter: &[(f32, f32)]) {
    if scatter.is_empty() {
        println!("  [{frame_idx:5}]  vectorscope: (unsupported format)");
        return;
    }
    let mean_cb = mean_f32(&scatter.iter().map(|(cb, _)| *cb).collect::<Vec<_>>());
    let mean_cr = mean_f32(&scatter.iter().map(|(_, cr)| *cr).collect::<Vec<_>>());
    let max_cb = scatter
        .iter()
        .map(|(cb, _)| cb.abs())
        .fold(0.0_f32, f32::max);
    let max_cr = scatter
        .iter()
        .map(|(_, cr)| cr.abs())
        .fold(0.0_f32, f32::max);
    println!(
        "  [{frame_idx:5}]  vectorscope: samples={:5}  \
         mean_cb={mean_cb:+.3}  mean_cr={mean_cr:+.3}  \
         max_cb={max_cb:.3}  max_cr={max_cr:.3}",
        scatter.len()
    );
}

fn print_rgb_parade_summary(frame_idx: u64, parade: &RgbParade) {
    if parade.r.is_empty() {
        println!("  [{frame_idx:5}]  rgb_parade: (unsupported format)");
        return;
    }
    let all_r: Vec<f32> = parade.r.iter().flatten().copied().collect();
    let all_g: Vec<f32> = parade.g.iter().flatten().copied().collect();
    let all_b: Vec<f32> = parade.b.iter().flatten().copied().collect();
    let avg_r = mean_f32(&all_r);
    let avg_g = mean_f32(&all_g);
    let avg_b = mean_f32(&all_b);
    println!(
        "  [{frame_idx:5}]  rgb_parade:  cols={:4}  \
         avg_r={avg_r:.3}  avg_g={avg_g:.3}  avg_b={avg_b:.3}",
        parade.r.len()
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut interval_frames: u64 = 30;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--interval" | "-n" => {
                let raw = args.next().unwrap_or_default();
                interval_frames = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid interval: {raw}");
                    process::exit(1);
                });
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: scope_analyzer --input <video> [--interval <frames>]");
        process::exit(1);
    });

    let mut decoder = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening video: {e}");
            process::exit(1);
        }
    };

    println!("Scope analysis: {input}");
    println!("Sampling every {interval_frames} frame(s)");
    println!();

    let mut frame_count: u64 = 0;
    let mut sample_count: u64 = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        if frame_count.is_multiple_of(interval_frames) {
            let scatter = ScopeAnalyzer::vectorscope(&frame);
            let parade = ScopeAnalyzer::rgb_parade(&frame);
            print_vectorscope_summary(frame_count, &scatter);
            print_rgb_parade_summary(frame_count, &parade);
            sample_count += 1;
        }

        frame_count += 1;
    }

    println!();
    println!("Decoded {frame_count} frame(s), sampled {sample_count}.");
}
