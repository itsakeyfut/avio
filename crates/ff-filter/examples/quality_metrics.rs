//! Compute SSIM and PSNR quality metrics between two video files.
//!
//! Uses [`QualityMetrics`] to compare a reference video against a distorted
//! version (e.g., after compression or transcoding).  Both metrics are computed
//! frame-by-frame using `FFmpeg`'s `ssim` and `psnr` filter graphs.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example quality_metrics -- \
//!     --reference original.mp4 --distorted compressed.mp4
//!
//! # Compare a video against itself — should give SSIM ≈ 1.0 and PSNR = ∞
//! cargo run --example quality_metrics -- \
//!     --reference video.mp4 --distorted video.mp4
//! ```

use std::process;

use ff_filter::QualityMetrics;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut reference = None::<String>;
    let mut distorted = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--reference" | "-r" => reference = Some(args.next().unwrap_or_default()),
            "--distorted" | "-d" => distorted = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let reference = reference.unwrap_or_else(|| {
        eprintln!("Usage: quality_metrics --reference <video> --distorted <video>");
        process::exit(1);
    });

    let distorted = distorted.unwrap_or_else(|| {
        eprintln!("Usage: quality_metrics --reference <video> --distorted <video>");
        process::exit(1);
    });

    println!("Reference:  {reference}");
    println!("Distorted:  {distorted}");
    println!();

    // SSIM
    print!("Computing SSIM… ");
    let ssim = QualityMetrics::ssim(&reference, &distorted).unwrap_or_else(|e| {
        eprintln!("\nSSIM error: {e}");
        process::exit(1);
    });

    // PSNR
    print!("Computing PSNR… ");
    let psnr = QualityMetrics::psnr(&reference, &distorted).unwrap_or_else(|e| {
        eprintln!("\nPSNR error: {e}");
        process::exit(1);
    });

    println!();
    println!("Video Quality Metrics");
    println!("{}", "-".repeat(32));

    // SSIM: 1.0 = identical, 0.0 = no similarity
    println!("  SSIM: {ssim:.6}");
    let ssim_quality = match ssim {
        s if s >= 0.99 => "excellent (near-lossless)",
        s if s >= 0.95 => "good",
        s if s >= 0.90 => "acceptable",
        _ => "poor",
    };
    println!("        Quality: {ssim_quality}");
    println!();

    // PSNR: higher is better; infinity means identical inputs
    if psnr == f32::INFINITY {
        println!("  PSNR: ∞ dB  (inputs are identical)");
    } else {
        println!("  PSNR: {psnr:.2} dB");
        let psnr_quality = match psnr {
            p if p >= 50.0 => "excellent",
            p if p >= 40.0 => "good",
            p if p >= 30.0 => "acceptable",
            _ => "poor",
        };
        println!("        Quality: {psnr_quality}");
    }
}
