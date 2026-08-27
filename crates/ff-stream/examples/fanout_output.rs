//! Fan a video source out to multiple outputs simultaneously.
//!
//! Demonstrates:
//! - `FanoutOutput` — deliver frames to multiple `StreamOutput` targets at once
//! - Combining `LiveHlsOutput` and `LiveDashOutput` targets in one fan-out
//! - Graceful error reporting: all targets still receive each frame even when
//!   one fails, and errors are aggregated into `StreamError::FanoutFailure`
//!
//! # Usage
//!
//! ```bash
//! cargo run --example fanout_output --features stream -- \
//!   --input     input.mp4    \
//!   --hls-dir   ./fanout-hls/ \
//!   --dash-dir  ./fanout-dash/ \
//!   [--segment  6]            \
//!   [--bitrate  2000000]
//! ```
//!
//! Both output directories will be populated in a single pass through the
//! source file. You can extend the example by adding an `RtmpOutput` target
//! to simultaneously push to a live ingest endpoint.

use std::{path::Path, process, time::Duration};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_stream::{FanoutOutput, LiveDashOutput, LiveHlsOutput, StreamOutput};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut hls_dir = None::<String>;
    let mut dash_dir = None::<String>;
    let mut segment_secs: u64 = 6;
    let mut bitrate: u64 = 2_000_000;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--hls-dir" => hls_dir = Some(args.next().unwrap_or_default()),
            "--dash-dir" => dash_dir = Some(args.next().unwrap_or_default()),
            "--segment" | "-s" => {
                let v = args.next().unwrap_or_default();
                segment_secs = v.parse().unwrap_or(6);
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
            "Usage: fanout_output --input <file> --hls-dir <dir> --dash-dir <dir> \
             [--segment N] [--bitrate N]"
        );
        process::exit(1);
    });
    let hls_dir = hls_dir.unwrap_or_else(|| {
        eprintln!("--hls-dir is required");
        process::exit(1);
    });
    let dash_dir = dash_dir.unwrap_or_else(|| {
        eprintln!("--dash-dir is required");
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

    println!("Input:    {in_name}  ({width}×{height}  {fps_display:.2} fps)");
    println!("HLS out:  {hls_dir}");
    println!("DASH out: {dash_dir}");
    println!("Segment:  {segment_secs} s");
    println!("Bitrate:  {bitrate} bps");
    println!();

    // ── Build individual targets ──────────────────────────────────────────────

    let has_audio = audio_dec.is_some();
    let seg_dur = Duration::from_secs(segment_secs);

    let mut hls_builder = LiveHlsOutput::new(&hls_dir)
        .video(width, height, fps_display)
        .segment_duration(seg_dur)
        .video_bitrate(bitrate);
    if has_audio {
        hls_builder = hls_builder.audio(44100, 2);
    }
    let hls = match hls_builder.build() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: cannot open LiveHlsOutput: {e}");
            process::exit(1);
        }
    };

    let mut dash_builder = LiveDashOutput::new(&dash_dir)
        .video(width, height, fps_display)
        .segment_duration(seg_dur)
        .video_bitrate(bitrate);
    if has_audio {
        dash_builder = dash_builder.audio(44100, 2);
    }
    let dash = match dash_builder.build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot open LiveDashOutput: {e}");
            process::exit(1);
        }
    };

    // ── Wrap in FanoutOutput ──────────────────────────────────────────────────

    let mut fanout = FanoutOutput::new(vec![Box::new(hls), Box::new(dash)]);

    // ── Frame loop ────────────────────────────────────────────────────────────

    println!("Encoding and fanning out...");
    let start = std::time::Instant::now();
    let mut video_frames: u64 = 0;
    let mut audio_frames: u64 = 0;

    loop {
        match video_dec.decode_one() {
            Ok(Some(frame)) => {
                video_frames += 1;
                if let Err(e) = fanout.push_video(&frame) {
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
                    if let Err(e) = fanout.push_audio(&frame) {
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

    if let Err(e) = Box::new(fanout).finish() {
        eprintln!("Error: finish: {e}");
        process::exit(1);
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!();
    println!("Done in {elapsed:.2} s — {video_frames} video frames, {audio_frames} audio frames");
    println!();
    println!("HLS:  npx serve {hls_dir}  (open http://localhost:3000/index.m3u8)");
    println!("DASH: npx serve {dash_dir}  (open http://localhost:3000/manifest.mpd)");
}
