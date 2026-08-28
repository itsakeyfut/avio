//! Ingest a local video file and publish it as a live HLS stream.
//!
//! Demonstrates:
//! - `VideoDecoder` — decode frames from a source file
//! - `LiveHlsOutput` — accept decoded frames and write a sliding-window
//!   `index.m3u8` playlist backed by `.ts` segment files
//! - `StreamOutput::finish()` — flush encoders and write the HLS trailer
//!
//! `LiveHlsOutput` is intended for sources where frames arrive one at a time
//! (camera, network ingest, synthetic generator). For transcoding a file to
//! a static HLS package, use `HlsOutput` instead.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example live_hls_output --features stream -- \
//!   --input    input.mp4   \
//!   --output   ./live-hls/ \
//!   [--segment 6]          \
//!   [--playlist-size 5]    \
//!   [--bitrate  2000000]
//! ```
//!
//! Serve the output directory with any HTTP server that supports byte-range
//! requests, for example:
//!
//! ```bash
//! npx serve ./live-hls/
//! # open http://localhost:3000/index.m3u8 in an HLS-capable player
//! ```

use std::{path::Path, process, time::Duration};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_stream::{LiveHlsOutput, StreamOutput};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut segment_secs: u64 = 6;
    let mut playlist_size: u32 = 5;
    let mut bitrate: u64 = 2_000_000;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--segment" | "-s" => {
                let v = args.next().unwrap_or_default();
                segment_secs = v.parse().unwrap_or(6);
            }
            "--playlist-size" | "-p" => {
                let v = args.next().unwrap_or_default();
                playlist_size = v.parse().unwrap_or(5);
            }
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().unwrap_or(2_000_000);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: live_hls_output --input <file> --output <dir> \
             [--segment N] [--playlist-size N] [--bitrate N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Open source decoders ──────────────────────────────────────────────────

    let mut video_dec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot open video decoder: {e}");
            process::exit(1);
        }
    };

    let mut audio_dec = AudioDecoder::open(&input).build().ok();

    let width = video_dec.width();
    let height = video_dec.height();
    let fps = video_dec.frame_rate();
    let fps_display = if fps > 0.0 { fps } else { 30.0 };

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input:         {in_name}  ({width}×{height}  {fps_display:.2} fps)");
    println!("Output:        {output}");
    println!("Segment:       {segment_secs} s");
    println!("Playlist size: {playlist_size}");
    println!("Bitrate:       {bitrate} bps");
    println!();

    // ── Open LiveHlsOutput ────────────────────────────────────────────────────

    let mut builder = LiveHlsOutput::new(&output)
        .video(width, height, fps_display)
        .segment_duration(Duration::from_secs(segment_secs))
        .playlist_size(playlist_size)
        .video_bitrate(bitrate);

    if audio_dec.is_some() {
        builder = builder.audio(44100, 2);
    }

    let mut hls = match builder.build() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: cannot open LiveHlsOutput: {e}");
            process::exit(1);
        }
    };

    // ── Frame loop ────────────────────────────────────────────────────────────

    println!("Encoding frames...");
    let start = std::time::Instant::now();
    let mut video_frames: u64 = 0;
    let mut audio_frames: u64 = 0;

    loop {
        match video_dec.decode_one() {
            Ok(Some(frame)) => {
                video_frames += 1;
                if let Err(e) = hls.push_video(&frame) {
                    eprintln!("Error: push_video: {e}");
                    process::exit(1);
                }
                if video_frames.is_multiple_of(300) {
                    let elapsed = start.elapsed().as_secs_f64();
                    println!("  {video_frames} video frames  ({elapsed:.1} s elapsed)");
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("Error: video decode: {e}");
                process::exit(1);
            }
        }
    }

    if let Some(ref mut adec) = audio_dec {
        loop {
            match adec.decode_one() {
                Ok(Some(frame)) => {
                    audio_frames += 1;
                    if let Err(e) = hls.push_audio(&frame) {
                        eprintln!("Error: push_audio: {e}");
                        process::exit(1);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Error: audio decode: {e}");
                    process::exit(1);
                }
            }
        }
    }

    if let Err(e) = Box::new(hls).finish() {
        eprintln!("Error: finish: {e}");
        process::exit(1);
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!();
    println!("Done in {elapsed:.2} s — {video_frames} video frames, {audio_frames} audio frames");
    println!();

    // ── List output files ─────────────────────────────────────────────────────

    let entries = match std::fs::read_dir(&output) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Warning: cannot list output: {e}");
            return;
        }
    };

    let mut files: Vec<(String, u64)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let size = e.metadata().ok()?.len();
            Some((name, size))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    println!("Output directory ({} files):", files.len());
    for (name, size) in &files {
        #[allow(clippy::cast_precision_loss)]
        let kb = *size as f64 / 1024.0;
        println!("  {name:<40}  ({kb:.1} KB)");
    }
    println!();
    println!("Serve with: npx serve {output}  (open http://localhost:3000/index.m3u8)");
}
