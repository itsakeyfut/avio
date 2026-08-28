//! Re-encode a video file using the high-level `Pipeline` API.
//!
//! Demonstrates pairing `VideoCodec` and `AudioCodec` together — both are
//! required to produce a valid audio+video output file.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example transcode -- \
//!   --input        input.mp4 \
//!   --output       output.mp4 \
//!   [--codec       h265]        # video codec: h264 | h265 | vp9 | av1 (default: h264)
//!   [--audio-codec opus]        # audio codec: aac | mp3 | opus | flac (default: aac)
//!   [--crf         28]          # default: 23
//!   [--width       1280]        # default: keep source
//!   [--height      720]         # default: keep source
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
    sync::{Arc, Mutex},
    time::Instant,
};

use ff_filter::FilterGraphBuilder;
use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, PipelineError, Progress};

// Argument parsing

struct Args {
    input: String,
    output: String,
    codec: VideoCodec,
    audio_codec: AudioCodec,
    crf: u32,
    width: Option<u32>,
    height: Option<u32>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "h264".to_string();
    let mut audio_codec_str = "aac".to_string();
    let mut crf: u32 = 23;
    let mut width = None::<u32>;
    let mut height = None::<u32>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => {
                input = Some(args.next().ok_or("--input requires a value")?);
            }
            "--output" | "-o" => {
                output = Some(args.next().ok_or("--output requires a value")?);
            }
            "--codec" | "-c" => {
                codec_str = args.next().ok_or("--codec requires a value")?;
            }
            "--audio-codec" => {
                audio_codec_str = args.next().ok_or("--audio-codec requires a value")?;
            }
            "--crf" => {
                let v = args.next().ok_or("--crf requires a value")?;
                crf = v
                    .parse()
                    .map_err(|_| format!("--crf: invalid number '{v}'"))?;
            }
            "--width" => {
                let v = args.next().ok_or("--width requires a value")?;
                width = Some(
                    v.parse()
                        .map_err(|_| format!("--width: invalid number '{v}'"))?,
                );
            }
            "--height" => {
                let v = args.next().ok_or("--height requires a value")?;
                height = Some(
                    v.parse()
                        .map_err(|_| format!("--height: invalid number '{v}'"))?,
                );
            }
            other => return Err(format!("Unknown flag: {other}")),
        }
    }

    let codec = match codec_str.to_lowercase().as_str() {
        "h264" | "avc" => VideoCodec::H264,
        "h265" | "hevc" => VideoCodec::H265,
        "vp9" => VideoCodec::Vp9,
        "av1" => VideoCodec::Av1,
        other => {
            return Err(format!(
                "Unknown codec: '{other}' (try h264, h265, vp9, av1)"
            ));
        }
    };

    let audio_codec = match audio_codec_str.to_lowercase().as_str() {
        "aac" => AudioCodec::Aac,
        "mp3" => AudioCodec::Mp3,
        "opus" => AudioCodec::Opus,
        "flac" => AudioCodec::Flac,
        other => {
            return Err(format!(
                "Unknown audio codec: '{other}' (try aac, mp3, opus, flac)"
            ));
        }
    };

    Ok(Args {
        input: input.ok_or("--input is required")?,
        output: output.ok_or("--output is required")?,
        codec,
        audio_codec,
        crf,
        width,
        height,
    })
}

// Progress rendering

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let s = elapsed.as_secs();
    let m = s / 60;
    let h = m / 60;
    if h > 0 {
        format!("{h:02}:{:02}:{:02}", m % 60, s % 60)
    } else {
        format!("{:02}:{:02}", m, s % 60)
    }
}

fn render_progress(p: &Progress) {
    let elapsed = format_elapsed(p.elapsed);

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
            let bar: String = "=".repeat(filled) + &" ".repeat(bar_width - filled);

            let remaining = if pct > 0.0 {
                let total_secs = p.elapsed.as_secs_f64() / (pct / 100.0);
                let rem_secs = (total_secs - p.elapsed.as_secs_f64()).max(0.0);
                let rem = std::time::Duration::from_secs_f64(rem_secs);
                format!("  ~{} remaining", format_elapsed(rem))
            } else {
                String::new()
            };

            print!("\r{pct:5.1}%  [{bar}]  {elapsed} elapsed{remaining}    ");
        }
        None => {
            print!("\r{} frames  {elapsed} elapsed    ", p.frames_processed);
        }
    }

    // Flush stdout so the line appears immediately (no newline → stays in place)
    let _ = io::stdout().flush();
}

// Main

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {e}");
            eprintln!(
                "Usage: transcode --input <file> --output <file> \
                 [--codec h264|h265|vp9|av1] [--audio-codec aac|mp3|opus|flac] \
                 [--crf N] [--width W] [--height H]"
            );
            process::exit(1);
        }
    };

    // Print header

    let in_name = Path::new(&args.input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&args.input);
    let out_name = Path::new(&args.output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&args.output);

    let codec_name = match args.codec {
        VideoCodec::H264 => "H.264",
        VideoCodec::H265 => "H.265",
        VideoCodec::Vp9 => "VP9",
        VideoCodec::Av1 => "AV1",
        _ => "unknown",
    };

    let audio_codec_name = match args.audio_codec {
        AudioCodec::Aac => "AAC",
        AudioCodec::Mp3 => "MP3",
        AudioCodec::Opus => "Opus",
        AudioCodec::Flac => "FLAC",
        _ => "unknown",
    };

    let res_str = match (args.width, args.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        (Some(w), None) => format!("{w}×(auto)"),
        (None, Some(h)) => format!("(auto)×{h}"),
        (None, None) => "(source)".to_string(),
    };

    println!("Input:  {in_name}");
    println!(
        "Output: {out_name}  video={codec_name}  audio={audio_codec_name}  crf={}  resolution={res_str}",
        args.crf
    );
    println!();

    // Build optional scale filter

    let filter = match (args.width, args.height) {
        (Some(w), Some(h)) => match FilterGraphBuilder::new()
            .scale(w, h, ff_filter::ScaleAlgorithm::Fast)
            .build()
        {
            Ok(fg) => Some(fg),
            Err(e) => {
                eprintln!("Error: failed to build filter graph: {e}");
                process::exit(1);
            }
        },
        _ => None,
    };

    // Build encoder config

    let config = EncoderConfig::builder()
        .video_codec(args.codec)
        .audio_codec(args.audio_codec)
        .crf(args.crf)
        .build();

    // Assemble pipeline

    let start = Instant::now();
    // Store last_frames inside an Arc<Mutex> so we can print final count after run().
    let last_frames: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let last_frames_cb = Arc::clone(&last_frames);

    let pipeline = match Pipeline::builder()
        .input(&args.input)
        .output(&args.output, config)
        .filter_opt(filter)
        .on_progress(move |p: &Progress| {
            render_progress(p);
            if let Ok(mut f) = last_frames_cb.lock() {
                *f = p.frames_processed;
            }
            true // always continue
        })
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    // Run

    match pipeline.run() {
        Ok(()) | Err(PipelineError::Cancelled) => {}
        Err(e) => {
            println!(); // end progress line
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }

    println!(); // end progress line

    // Final summary

    let elapsed = format_elapsed(start.elapsed());
    let frames = *last_frames.lock().unwrap_or_else(|e| e.into_inner());

    let size_str = match std::fs::metadata(&args.output) {
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

    println!("Done.  {out_name}  {size_str}  {frames} frames  {elapsed}");
}
