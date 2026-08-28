//! Push a video file to an RTMP ingest endpoint in real time.
//!
//! Demonstrates:
//! - `VideoDecoder` — decode frames from a source file
//! - `RtmpOutput` — encode frames and push them to an `rtmp://` URL
//! - `StreamOutput::finish()` — flush encoders and close the RTMP connection
//!
//! RTMP/FLV requires H.264 video and AAC audio. `RtmpOutput` enforces this at
//! `build()` time and returns `StreamError::UnsupportedCodec` for any other
//! codec selection.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example rtmp_output -- \
//!   --input   input.mp4                              \
//!   --url     rtmp://ingest.example.com/live/key     \
//!   [--bitrate 4000000]
//! ```
//!
//! To test locally with a self-hosted RTMP server (e.g. nginx-rtmp or SRS):
//!
//! ```bash
//! # nginx-rtmp on localhost:1935, application "live"
//! cargo run --example rtmp_output -- \
//!   --input input.mp4  --url rtmp://127.0.0.1/live/test
//! ```

use std::{path::Path, process};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_stream::{RtmpOutput, StreamOutput};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut url = None::<String>;
    let mut bitrate: u64 = 4_000_000;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--url" | "-u" => url = Some(args.next().unwrap_or_default()),
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().unwrap_or(4_000_000);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: rtmp_output --input <file> --url <rtmp://...> [--bitrate N]");
        process::exit(1);
    });
    let url = url.unwrap_or_else(|| {
        eprintln!("--url is required (must start with rtmp://)");
        process::exit(1);
    });

    // Open source decoders

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

    println!("Input:   {in_name}  ({width}×{height}  {fps_display:.2} fps)");
    println!("URL:     {url}");
    println!("Bitrate: {bitrate} bps");
    println!();
    println!("Connecting...");

    // Open RtmpOutput

    let mut builder = RtmpOutput::new(&url)
        .video(width, height, fps_display)
        .video_bitrate(bitrate);

    if audio_dec.is_some() {
        builder = builder.audio(44100, 2);
    }

    let mut rtmp = match builder.build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: cannot open RtmpOutput: {e}");
            process::exit(1);
        }
    };

    println!("Connected. Streaming frames...");
    println!();

    // Frame loop

    let start = std::time::Instant::now();
    let mut video_frames: u64 = 0;
    let mut audio_frames: u64 = 0;

    loop {
        match video_dec.decode_one() {
            Ok(Some(frame)) => {
                video_frames += 1;
                if let Err(e) = rtmp.push_video(&frame) {
                    eprintln!("Error: push_video: {e}");
                    process::exit(1);
                }
                if video_frames.is_multiple_of(150) {
                    let elapsed = start.elapsed().as_secs_f64();
                    println!("  {video_frames} frames sent  ({elapsed:.1} s elapsed)");
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
                    if let Err(e) = rtmp.push_audio(&frame) {
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

    if let Err(e) = Box::new(rtmp).finish() {
        eprintln!("Error: finish: {e}");
        process::exit(1);
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!();
    println!(
        "Done in {elapsed:.2} s — {video_frames} video frames, {audio_frames} audio frames sent"
    );
}
