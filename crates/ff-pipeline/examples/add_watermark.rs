//! Overlay a PNG watermark onto a video using the `overlay` filter.
//!
//! The `overlay` filter requires **two** video input streams:
//! - slot 0: the main video frames (driven by `Pipeline` internally)
//! - slot 1: the watermark frame (fed via `secondary_input`)
//!
//! # Usage
//!
//! ```bash
//! cargo run --example add_watermark -- \
//!   --input     input.mp4   \
//!   --watermark logo.png    \
//!   --output    branded.mp4 \
//!   [--position bottom-right]  \
//!   [--margin   20]
//! ```

use std::{path::Path, process};

use ff_decode::{ImageDecoder, VideoDecoder};
use ff_filter::FilterGraphBuilder;
use ff_format::VideoCodec;
use ff_pipeline::{EncoderConfig, Pipeline};

// ── Position helpers ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Position {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Position {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "top-left" => Some(Self::TopLeft),
            "top-right" => Some(Self::TopRight),
            "bottom-left" => Some(Self::BottomLeft),
            "bottom-right" => Some(Self::BottomRight),
            "center" => Some(Self::Center),
            _ => None,
        }
    }

    fn compute(self, vid_w: u32, vid_h: u32, wm_w: u32, wm_h: u32, margin: u32) -> (i32, i32) {
        let m = margin.cast_signed();
        let x = match self {
            Self::TopLeft | Self::BottomLeft => m,
            Self::TopRight | Self::BottomRight => vid_w.cast_signed() - wm_w.cast_signed() - m,
            Self::Center => (vid_w.cast_signed() - wm_w.cast_signed()) / 2,
        };
        let y = match self {
            Self::TopLeft | Self::TopRight => m,
            Self::BottomLeft | Self::BottomRight => vid_h.cast_signed() - wm_h.cast_signed() - m,
            Self::Center => (vid_h.cast_signed() - wm_h.cast_signed()) / 2,
        };
        (x, y)
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut watermark = None::<String>;
    let mut output = None::<String>;
    let mut position = Position::BottomRight;
    let mut margin: u32 = 20;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--watermark" | "-w" => watermark = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--position" => {
                let s = args.next().unwrap_or_default();
                position = Position::from_str(&s).unwrap_or_else(|| {
                    eprintln!("Unknown position '{s}' (try top-left, top-right, bottom-left, bottom-right, center)");
                    process::exit(1);
                });
            }
            "--margin" => {
                let v = args.next().unwrap_or_default();
                margin = v.parse().unwrap_or(20);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: add_watermark --input <video> --watermark <logo.png> --output <branded.mp4> [--position top-left|top-right|bottom-left|bottom-right|center] [--margin N]");
        process::exit(1);
    });
    let watermark = watermark.unwrap_or_else(|| {
        eprintln!("--watermark is required");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Probe watermark dimensions ────────────────────────────────────────────

    let wm_dec = match ImageDecoder::open(&watermark).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening watermark: {e}");
            process::exit(1);
        }
    };
    let wm_frame = match wm_dec.decode() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error decoding watermark: {e}");
            process::exit(1);
        }
    };
    let wm_w = wm_frame.width();
    let wm_h = wm_frame.height();

    // ── Probe video dimensions ────────────────────────────────────────────────

    let vid_dec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening video: {e}");
            process::exit(1);
        }
    };
    let vid_w = vid_dec.width();
    let vid_h = vid_dec.height();

    // ── Compute overlay position ──────────────────────────────────────────────

    let (x, y) = position.compute(vid_w, vid_h, wm_w, wm_h, margin);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let wm_name = Path::new(&watermark)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&watermark);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:      {in_name}  {vid_w}×{vid_h}");
    println!("Watermark:  {wm_name}  {wm_w}×{wm_h}");
    println!("Position:   {position:?}  (x={x}, y={y})");
    println!("Output:     {out_name}");
    println!();

    // ── Build overlay filter graph ────────────────────────────────────────────

    let filter = match FilterGraphBuilder::new().overlay(x, y).build() {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    // ── Build and run pipeline ────────────────────────────────────────────────

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .crf(23)
        .build();

    let pipeline = match Pipeline::builder()
        .input(&input)
        .secondary_input(&watermark)
        .filter(filter)
        .output(&output, config)
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = pipeline.run() {
        eprintln!("Error: {e}");
        process::exit(1);
    }

    println!("Done. {out_name}");
}
