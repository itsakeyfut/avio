//! Transcode a video using hardware-accelerated encoding via `Pipeline`.
//!
//! Hardware encoding is orders of magnitude faster than software encoding on
//! supported hardware, making it the recommended path for production workloads.
//!
//! Available backends (via `--hw`):
//!   `cuda`         — NVIDIA CUDA / NVENC
//!   `videotoolbox` — Apple `VideoToolbox` (macOS)
//!   `vaapi`        — VA-API (Linux)
//!   `none`         — software encoding only (useful for comparison)
//!
//! The pipeline falls back to software encoding if the requested hardware
//! backend is unavailable on the current system.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example hw_transcode --features pipeline -- \
//!   --input   input.mp4   \
//!   --output  output.mp4  \
//!   --hw      cuda         \  # cuda | videotoolbox | vaapi | none
//!   [--codec  h264]           # h264 | h265 (default: h264)
//!   [--crf    23]             # quality (default: 23)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
    sync::{Arc, Mutex},
    time::Instant,
};

use ff_filter::HwAccel;
use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, PipelineError, Progress};

fn format_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    let m = s / 60;
    let h = m / 60;
    if h > 0 {
        format!("{h:02}:{:02}:{:02}", m % 60, s % 60)
    } else {
        format!("{:02}:{:02}", m, s % 60)
    }
}

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
            let elapsed = format_elapsed(p.elapsed);
            print!("\r{pct:5.1}%  [{bar}]  {elapsed}    ");
        }
        None => {
            print!(
                "\r{} frames  {}    ",
                p.frames_processed,
                format_elapsed(p.elapsed)
            );
        }
    }
    let _ = io::stdout().flush();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut hw_str = None::<String>;
    let mut codec_str = "h264".to_string();
    let mut crf: u32 = 23;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--hw" => hw_str = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "h264".to_string()),
            "--crf" => {
                let v = args.next().unwrap_or_default();
                crf = v.parse().unwrap_or(23);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: hw_transcode --input <file> --output <file> \
             --hw cuda|videotoolbox|vaapi|none [--codec h264|h265] [--crf N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let hw_str = hw_str.unwrap_or_else(|| {
        eprintln!("--hw is required (cuda|videotoolbox|vaapi|none)");
        process::exit(1);
    });

    // Map --hw to HwAccel. `cuda` covers NVIDIA NVENC hardware via the CUDA
    // device context; `none` disables hardware acceleration entirely.
    let hw_accel: Option<HwAccel> = match hw_str.to_lowercase().as_str() {
        "cuda" | "nvenc" => Some(HwAccel::Cuda),
        "videotoolbox" => Some(HwAccel::VideoToolbox),
        "vaapi" => Some(HwAccel::Vaapi),
        "none" | "software" => None,
        other => {
            eprintln!("Unknown hw backend '{other}' (try cuda, videotoolbox, vaapi, none)");
            process::exit(1);
        }
    };

    let video_codec = match codec_str.to_lowercase().as_str() {
        "h264" | "avc" => VideoCodec::H264,
        "h265" | "hevc" => VideoCodec::H265,
        other => {
            eprintln!("Unknown codec '{other}' (try h264, h265)");
            process::exit(1);
        }
    };

    let hw_label = hw_accel.map_or("software", |hw| match hw {
        HwAccel::Cuda => "CUDA (NVENC)",
        HwAccel::VideoToolbox => "VideoToolbox",
        HwAccel::Vaapi => "VA-API",
        _ => "hardware",
    });

    let codec_label = match video_codec {
        VideoCodec::H264 => "H.264",
        VideoCodec::H265 => "H.265",
        _ => "unknown",
    };

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:   {in_name}");
    println!("Output:  {out_name}  {codec_label}  {hw_label}  crf={crf}");
    println!();

    // ── Build pipeline with hardware backend ──────────────────────────────────

    let mut b = EncoderConfig::builder()
        .video_codec(video_codec)
        .audio_codec(AudioCodec::Aac)
        .crf(crf);
    if let Some(hw) = hw_accel {
        b = b.hardware(hw);
    }
    let config = b.build();

    let start = Instant::now();
    let last_frames: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let last_frames_cb = Arc::clone(&last_frames);

    let pipeline = match Pipeline::builder()
        .input(&input)
        .output(&output, config)
        .on_progress(move |p: &Progress| {
            render_progress(p);
            if let Ok(mut f) = last_frames_cb.lock() {
                *f = p.frames_processed;
            }
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

    match pipeline.run() {
        Ok(()) | Err(PipelineError::Cancelled) => {}
        Err(e) => {
            println!();
            eprintln!("Error: {e}");
            // Hardware encoder unavailable — suggest fallback.
            if hw_accel.is_some() {
                eprintln!(
                    "Hint: hardware backend may be unavailable on this system. \
                     Try --hw none to use software encoding."
                );
            }
            process::exit(1);
        }
    }

    println!();

    let elapsed = format_elapsed(start.elapsed());
    let frames = *last_frames.lock().unwrap_or_else(|e| e.into_inner());

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

    println!("Done. {out_name}  {size_str}  {frames} frames  {elapsed}");
}
