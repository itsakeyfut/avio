//! Encode a video file into a live multi-rendition ABR ladder.
//!
//! Demonstrates:
//! - `LiveAbrLadder` — fan decoded frames to multiple encoders at different
//!   resolutions and bitrates in one pass
//! - `AbrRendition` — per-rendition width, height, and bitrate settings
//! - `LiveAbrFormat` — choose HLS (`index.m3u8` per rendition + `master.m3u8`)
//!   or DASH (`manifest.mpd` per rendition + top-level `manifest.mpd`)
//! - `StreamOutput::finish()` — flush all encoders and write the master playlist
//!
//! Each rendition is placed in a subdirectory named after its resolution
//! (`{width}x{height}` by default). The master playlist at the root of the
//! output directory references all renditions.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example live_abr_ladder -- \
//!   --input   input.mp4  \
//!   --output  ./abr-live/ \
//!   [--format hls]        \
//!   [--segment 6]         \
//!   [--ladder 1080,4000000:720,2000000:480,1000000]
//! ```
//!
//! Serve with:
//!
//! ```bash
//! npx serve ./abr-live/
//! # HLS:  open http://localhost:3000/master.m3u8
//! # DASH: open http://localhost:3000/manifest.mpd
//! ```

use std::{path::Path, process, time::Duration};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_stream::{AbrRendition, LiveAbrFormat, LiveAbrLadder, StreamOutput};

/// Parse `"H,BPS:H,BPS:..."` into `Vec<(height, bitrate)>`.
fn parse_ladder(s: &str) -> Result<Vec<(u32, u64)>, String> {
    s.split(':')
        .map(|pair| {
            let mut parts = pair.splitn(2, ',');
            let height: u32 = parts
                .next()
                .ok_or_else(|| format!("missing height in '{pair}'"))?
                .parse()
                .map_err(|_| format!("invalid height in '{pair}'"))?;
            let bitrate: u64 = parts
                .next()
                .ok_or_else(|| format!("missing bitrate in '{pair}'"))?
                .parse()
                .map_err(|_| format!("invalid bitrate in '{pair}'"))?;
            Ok((height, bitrate))
        })
        .collect()
}

const DEFAULT_LADDER: &[(u32, u64)] = &[(1080, 4_000_000), (720, 2_000_000), (480, 1_000_000)];

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut format_str = "hls".to_string();
    let mut segment_secs: u64 = 6;
    let mut ladder_str = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--format" | "-f" => format_str = args.next().unwrap_or_else(|| "hls".to_string()),
            "--segment" | "-s" => {
                let v = args.next().unwrap_or_default();
                segment_secs = v.parse().unwrap_or(6);
            }
            "--ladder" => ladder_str = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: live_abr_ladder --input <file> --output <dir> \
             [--format hls|dash] [--segment N] [--ladder H,BPS:H,BPS]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let format_lower = format_str.to_lowercase();
    if format_lower != "hls" && format_lower != "dash" {
        eprintln!("Error: --format must be 'hls' or 'dash'");
        process::exit(1);
    }

    let ladder_pairs = match ladder_str {
        Some(ref s) => parse_ladder(s).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        }),
        None => DEFAULT_LADDER.to_vec(),
    };

    // ── Open source decoders ──────────────────────────────────────────────────

    let mut video_dec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot open video decoder: {e}");
            process::exit(1);
        }
    };

    let mut audio_dec = AudioDecoder::open(&input).build().ok();

    let src_width = video_dec.width();
    let src_height = video_dec.height();
    let fps = video_dec.frame_rate();
    let fps_display = if fps > 0.0 { fps } else { 30.0 };

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input:   {in_name}  ({src_width}×{src_height}  {fps_display:.2} fps)");
    println!("Output:  {output}");
    println!("Format:  {}", format_lower.to_uppercase());
    println!("Segment: {segment_secs} s");
    println!();

    // ── Build rendition list ──────────────────────────────────────────────────

    let aspect = if src_height > 0 {
        f64::from(src_width) / f64::from(src_height)
    } else {
        16.0 / 9.0
    };

    println!("Renditions:");
    let mut renditions = Vec::new();
    for (i, &(height, video_bitrate)) in ladder_pairs.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = ((f64::from(height) * aspect).round() as u32 / 2) * 2; // ensure even
        let audio_bitrate = if video_bitrate >= 4_000_000 {
            192_000
        } else {
            128_000
        };
        println!("  #{i}  {width}×{height}  video={video_bitrate} bps  audio={audio_bitrate} bps");
        renditions.push(AbrRendition {
            width,
            height,
            video_bitrate,
            audio_bitrate,
            name: None,
        });
    }
    println!();

    // ── Open LiveAbrLadder ────────────────────────────────────────────────────

    let fmt = if format_lower == "dash" {
        LiveAbrFormat::Dash
    } else {
        LiveAbrFormat::Hls
    };

    let mut builder = LiveAbrLadder::new(&output)
        .fps(fps_display)
        .segment_duration(Duration::from_secs(segment_secs))
        .format(fmt);

    for r in renditions {
        builder = builder.add_rendition(r);
    }

    if audio_dec.is_some() {
        builder = builder.audio(44100, 2);
    }

    let mut ladder = match builder.build() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Error: cannot open LiveAbrLadder: {e}");
            process::exit(1);
        }
    };

    // ── Frame loop ────────────────────────────────────────────────────────────

    println!("Encoding...");
    let start = std::time::Instant::now();
    let mut video_frames: u64 = 0;
    let mut audio_frames: u64 = 0;

    loop {
        match video_dec.decode_one() {
            Ok(Some(frame)) => {
                video_frames += 1;
                if let Err(e) = ladder.push_video(&frame) {
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
                    if let Err(e) = ladder.push_audio(&frame) {
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

    if let Err(e) = Box::new(ladder).finish() {
        eprintln!("Error: finish: {e}");
        process::exit(1);
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!();
    println!("Done in {elapsed:.2} s — {video_frames} video frames, {audio_frames} audio frames");
    println!();

    // ── Show master playlist location ─────────────────────────────────────────

    if format_lower == "hls" {
        println!("Master playlist: {output}/master.m3u8");
        println!("Serve with: npx serve {output}  (open http://localhost:3000/master.m3u8)");
    } else {
        println!("Manifest: {output}/manifest.mpd");
        println!("Serve with: npx serve {output}  (open http://localhost:3000/manifest.mpd)");
    }
}
