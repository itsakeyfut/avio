//! Apply video enhancement and noise reduction using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `blur`         — Gaussian blur with configurable sigma
//!   `sharpen`      — unsharp mask (luma + chroma strength)
//!   `hqdn3d`       — High Quality 3D noise reduction
//!   `nlmeans`      — non-local means noise reduction (high quality, CPU-intensive)
//!   `deinterlace`  — deinterlace using yadif
//!
//! # Usage
//!
//! ```bash
//! cargo run --example video_enhance --features pipeline -- \
//!   --input    input.mp4    \
//!   --output   enhanced.mp4 \
//!   --effect   blur         \
//!   [--sigma       2.0]      # blur: Gaussian sigma (default: 2.0)
//!   [--luma        0.5]      # sharpen: luma strength −1.5–1.5   (default: 0.5)
//!   [--chroma      0.3]      # sharpen: chroma strength −1.5–1.5 (default: 0.3)
//!   [--nlmeans-str 5.0]      # nlmeans: denoising strength 1–30  (default: 5.0)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::{FilterGraphBuilder, YadifMode};
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
    let mut sigma: f32 = 2.0;
    let mut luma_strength: f32 = 0.5;
    let mut chroma_strength: f32 = 0.3;
    let mut nlmeans_str: f32 = 5.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--sigma" => sigma = args.next().unwrap_or_default().parse().unwrap_or(2.0),
            "--luma" => luma_strength = args.next().unwrap_or_default().parse().unwrap_or(0.5),
            "--chroma" => {
                chroma_strength = args.next().unwrap_or_default().parse().unwrap_or(0.3);
            }
            "--nlmeans-str" => {
                nlmeans_str = args.next().unwrap_or_default().parse().unwrap_or(5.0);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: video_enhance --input <file> --output <file> \
             --effect blur|sharpen|hqdn3d|nlmeans|deinterlace [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (blur|sharpen|hqdn3d|nlmeans|deinterlace)");
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
        "blur" => {
            println!("Input:   {in_name}");
            println!("Effect:  gblur  (sigma={sigma:.1})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().gblur(sigma).build()
        }
        "sharpen" => {
            println!("Input:   {in_name}");
            println!("Effect:  unsharp  (luma={luma_strength:+.2}  chroma={chroma_strength:+.2})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .unsharp(luma_strength, chroma_strength)
                .build()
        }
        "hqdn3d" => {
            // Typical values from FFmpeg docs: luma_spatial=4, chroma_spatial=3,
            // luma_tmp=6, chroma_tmp=4.5.
            println!("Input:   {in_name}");
            println!(
                "Effect:  hqdn3d  (luma_spatial=4.0  chroma_spatial=3.0  luma_tmp=6.0  chroma_tmp=4.5)"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().hqdn3d(4.0, 3.0, 6.0, 4.5).build()
        }
        "nlmeans" => {
            println!("Input:   {in_name}");
            println!("Effect:  nlmeans  (strength={nlmeans_str:.1})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().nlmeans(nlmeans_str).build()
        }
        "deinterlace" => {
            println!("Input:   {in_name}");
            println!("Effect:  yadif  (mode=frame — one output frame per input frame)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().yadif(YadifMode::Frame).build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try blur, sharpen, hqdn3d, nlmeans, deinterlace)");
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
