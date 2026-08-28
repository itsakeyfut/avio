//! Apply color grading effects using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `lut`          — apply a 3D LUT from a `.cube` or `.3dl` file
//!   `eq`           — adjust brightness, contrast, and saturation
//!   `curves`       — apply per-channel RGB color curves
//!   `white-balance` — correct white balance by color temperature (Kelvin)
//!   `hue`          — rotate hue by an angle in degrees
//!   `gamma`        — apply per-channel gamma correction
//!   `three-way-cc` — three-way color corrector (lift / gamma / gain)
//!
//! # Usage
//!
//! ```bash
//! cargo run --example color_grade --features pipeline -- \
//!   --input   input.mp4   \
//!   --output  graded.mp4  \
//!   --effect  eq          \
//!   [--lut-file grade.cube]        # path to .cube/.3dl file (lut effect)
//!   [--brightness  0.05]           # eq: −1.0–1.0  (default: 0.0)
//!   [--contrast    1.2]            # eq: 0.0–3.0   (default: 1.0)
//!   [--saturation  1.5]            # eq: 0.0–3.0   (default: 1.0)
//!   [--temp  5500]                 # white-balance: Kelvin 1000–40000 (default: 6500)
//!   [--tint  0.0]                  # white-balance: −1.0–1.0         (default: 0.0)
//!   [--hue-degrees  30.0]          # hue rotation in degrees          (default: 0.0)
//!   [--gamma-r 1.0 --gamma-g 1.0 --gamma-b 1.0]  # per-channel gamma (default: 1.0)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::{FilterGraphBuilder, Rgb};
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
    let mut lut_file = None::<String>;
    let mut brightness: f32 = 0.0;
    let mut contrast: f32 = 1.0;
    let mut saturation: f32 = 1.0;
    let mut temp: u32 = 6500;
    let mut tint: f32 = 0.0;
    let mut hue_degrees: f32 = 0.0;
    let mut gamma_r: f32 = 1.0;
    let mut gamma_g: f32 = 1.0;
    let mut gamma_b: f32 = 1.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--lut-file" => lut_file = Some(args.next().unwrap_or_default()),
            "--brightness" => brightness = args.next().unwrap_or_default().parse().unwrap_or(0.0),
            "--contrast" => contrast = args.next().unwrap_or_default().parse().unwrap_or(1.0),
            "--saturation" => saturation = args.next().unwrap_or_default().parse().unwrap_or(1.0),
            "--temp" => temp = args.next().unwrap_or_default().parse().unwrap_or(6500),
            "--tint" => tint = args.next().unwrap_or_default().parse().unwrap_or(0.0),
            "--hue-degrees" => {
                hue_degrees = args.next().unwrap_or_default().parse().unwrap_or(0.0);
            }
            "--gamma-r" => gamma_r = args.next().unwrap_or_default().parse().unwrap_or(1.0),
            "--gamma-g" => gamma_g = args.next().unwrap_or_default().parse().unwrap_or(1.0),
            "--gamma-b" => gamma_b = args.next().unwrap_or_default().parse().unwrap_or(1.0),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: color_grade --input <file> --output <file> \
             --effect lut|eq|curves|white-balance|hue|gamma|three-way-cc [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (lut|eq|curves|white-balance|hue|gamma|three-way-cc)");
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
        "lut" => {
            let path = lut_file.unwrap_or_else(|| {
                eprintln!("--lut-file is required for lut effect");
                process::exit(1);
            });
            println!("Input:   {in_name}");
            println!(
                "Effect:  lut3d  (file={})",
                Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path)
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().lut3d(&path).build()
        }
        "eq" => {
            println!("Input:   {in_name}");
            println!(
                "Effect:  eq  (brightness={brightness:+.2}  contrast={contrast:.2}  \
                 saturation={saturation:.2})"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .eq(brightness, contrast, saturation)
                .build()
        }
        "curves" => {
            // Mild S-curve on each channel for cinematic contrast.
            println!("Input:   {in_name}");
            println!("Effect:  curves  (S-curve on R/G/B)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .curves(
                    vec![],
                    vec![(0.0, 0.0), (0.25, 0.20), (0.75, 0.80), (1.0, 1.0)],
                    vec![(0.0, 0.0), (0.25, 0.20), (0.75, 0.80), (1.0, 1.0)],
                    vec![(0.0, 0.0), (0.25, 0.20), (0.75, 0.80), (1.0, 1.0)],
                )
                .build()
        }
        "white-balance" => {
            println!("Input:   {in_name}");
            println!("Effect:  white_balance  (temp={temp} K  tint={tint:+.2})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().white_balance(temp, tint).build()
        }
        "hue" => {
            println!("Input:   {in_name}");
            println!("Effect:  hue  (degrees={hue_degrees:.1}°)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().hue(hue_degrees).build()
        }
        "gamma" => {
            println!("Input:   {in_name}");
            println!("Effect:  gamma  (R={gamma_r:.2}  G={gamma_g:.2}  B={gamma_b:.2})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .gamma(gamma_r, gamma_g, gamma_b)
                .build()
        }
        "three-way-cc" => {
            // Slight lift in shadows, neutral gamma, gentle gain in highlights.
            println!("Input:   {in_name}");
            println!("Effect:  three_way_cc  (lift=0.95  gamma=neutral  gain=1.05)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .three_way_cc(
                    Rgb {
                        r: 0.95,
                        g: 0.95,
                        b: 0.95,
                    },
                    Rgb::NEUTRAL,
                    Rgb {
                        r: 1.05,
                        g: 1.05,
                        b: 1.05,
                    },
                )
                .build()
        }
        other => {
            eprintln!(
                "Unknown effect '{other}' \
                 (try lut, eq, curves, white-balance, hue, gamma, three-way-cc)"
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
