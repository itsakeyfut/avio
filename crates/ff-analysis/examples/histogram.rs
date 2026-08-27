//! Extract per-channel color histograms from a video file.
//!
//! Uses [`HistogramExtractor`] to compute R, G, B, and luma histograms at
//! configurable frame intervals.  Prints a summary of each sampled frame's
//! dominant color bin and mean luma.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example histogram --features decode -- --input video.mp4
//! cargo run --example histogram --features decode -- --input video.mp4 --interval 30
//! ```

use std::process;

use ff_analysis::{FrameHistogram, HistogramExtractor};

/// Returns the bin index with the highest count (dominant intensity level).
fn dominant_bin(channel: &[u32; 256]) -> usize {
    channel
        .iter()
        .enumerate()
        .max_by_key(|&(_, count)| count)
        .map_or(0, |(i, _)| i)
}

/// Computes mean value from a histogram (weighted average of bin indices).
fn mean_from_histogram(channel: &[u32; 256]) -> f64 {
    let total: u64 = channel.iter().map(|&c| u64::from(c)).sum();
    if total == 0 {
        return 0.0;
    }
    let weighted_sum: u64 = channel
        .iter()
        .enumerate()
        .map(|(i, &c)| i as u64 * u64::from(c))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let result = weighted_sum as f64 / total as f64;
    result
}

fn print_histogram_summary(i: usize, h: &FrameHistogram) {
    let secs = h.timestamp.as_secs_f64();
    let mean_r = mean_from_histogram(&h.r);
    let mean_g = mean_from_histogram(&h.g);
    let mean_b = mean_from_histogram(&h.b);
    let mean_luma = mean_from_histogram(&h.luma);
    println!(
        "  [{i:4}] t={secs:7.3}s  R={mean_r:5.1}  G={mean_g:5.1}  B={mean_b:5.1}  luma={mean_luma:5.1}  \
         dominant_r={:3}  dominant_g={:3}  dominant_b={:3}",
        dominant_bin(&h.r),
        dominant_bin(&h.g),
        dominant_bin(&h.b),
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut interval_frames = 30u32;

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
        eprintln!("Usage: histogram --input <video> [--interval <frames>]");
        process::exit(1);
    });

    println!("Extracting color histograms from: {input}");
    println!("Sampling every {interval_frames} frame(s)");
    println!();

    let histograms: Vec<FrameHistogram> = HistogramExtractor::new(&input)
        .interval_frames(interval_frames)
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    println!("Extracted {} histogram(s).", histograms.len());
    println!();
    println!(
        "  {:^6}  {:^8}  {:^6}  {:^6}  {:^6}  {:^6}  {:^11}  {:^11}  {:^11}",
        "Index", "Time (s)", "Mean R", "Mean G", "Mean B", "Luma", "Dom. R", "Dom. G", "Dom. B"
    );
    println!("{}", "-".repeat(95));

    let display_count = histograms.len().min(30);
    for (i, h) in histograms.iter().take(display_count).enumerate() {
        print_histogram_summary(i, h);
    }
    if histograms.len() > display_count {
        println!("  … ({} more histograms)", histograms.len() - display_count);
    }
}
