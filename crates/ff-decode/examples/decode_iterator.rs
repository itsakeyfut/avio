//! Decode video, audio, and images using the iterator API.
//!
//! Demonstrates:
//! - `VideoDecoder` — `for result in &mut vdec`
//! - `AudioDecoder` — `for result in &mut adec`
//! - `ImageDecoder::frames()` — `for result in idec.frames()`
//!
//! `VideoDecoder` and `AudioDecoder` implement `Iterator` directly, so they
//! integrate naturally with iterator adaptors (`take`, `filter_map`, `enumerate`,
//! etc.) and also implement `FusedIterator` (no further items after an error).
//! `ImageDecoder` still uses the `.frames()` method.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example decode_iterator --features decode -- \
//!   --input input.mp4   \
//!   [--image photo.png]  # optional: also decode a still image
//! ```

use std::{path::Path, process};

use ff_decode::{AudioDecoder, ImageDecoder, VideoDecoder};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut image = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--image" => image = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: decode_iterator --input <file> [--image <file>]");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input: {in_name}");
    println!();

    // ── VideoDecoder ──────────────────────────────────────────────────────────
    //
    // VideoDecoder implements Iterator directly. Item = Result<VideoFrame, DecodeError>.
    //
    // The `for` loop terminates when the iterator returns None (EOF).
    // Errors are surfaced as Err variants inside the loop body.
    // FusedIterator guarantees None after the first error.

    println!("=== Video frames ===");

    match VideoDecoder::open(&input).build() {
        Ok(mut vdec) => {
            println!(
                "Source: {}×{}  {:.2} fps  codec={}",
                vdec.width(),
                vdec.height(),
                vdec.frame_rate(),
                vdec.stream_info().codec_name(),
            );

            let mut video_frames: u64 = 0;

            // Iterator form: VideoDecoder implements Iterator directly.
            for result in &mut vdec {
                match result {
                    Ok(_frame) => video_frames += 1,
                    Err(e) => {
                        eprintln!("Video decode error: {e}");
                        break;
                    }
                }
            }

            println!("Decoded {video_frames} video frames");
        }
        Err(e) => println!("Skipping video: {e}"),
    }

    println!();

    // ── AudioDecoder ──────────────────────────────────────────────────────────
    //
    // AudioDecoder implements Iterator directly. Item = Result<AudioFrame, DecodeError>.

    println!("=== Audio frames ===");

    match AudioDecoder::open(&input).build() {
        Ok(mut adec) => {
            println!(
                "Source: {} Hz  channels={}  codec={}",
                adec.sample_rate(),
                adec.channels(),
                adec.stream_info().codec_name(),
            );

            let mut audio_frames: u64 = 0;
            let mut total_samples: u64 = 0;

            for result in &mut adec {
                match result {
                    Ok(frame) => {
                        audio_frames += 1;
                        total_samples += frame.samples() as u64;
                    }
                    Err(e) => {
                        eprintln!("Audio decode error: {e}");
                        break;
                    }
                }
            }

            println!("Decoded {audio_frames} audio frames  ({total_samples} samples)");
        }
        Err(e) => println!("Skipping audio (no audio stream): {e}"),
    }

    // ── ImageFrameIterator ────────────────────────────────────────────────────
    //
    // ImageDecoder::frames() returns an iterator with
    // Item = Result<VideoFrame, DecodeError>.
    // For a still image exactly one frame is expected.

    if let Some(ref img_path) = image {
        println!();
        let img_name = Path::new(img_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(img_path.as_str());

        println!("=== Image frame ({img_name}) ===");

        match ImageDecoder::open(img_path).build() {
            Ok(mut idec) => {
                for result in idec.frames() {
                    match result {
                        Ok(frame) => {
                            println!(
                                "Frame: {}×{}  format={}",
                                frame.width(),
                                frame.height(),
                                frame.format(),
                            );
                        }
                        Err(e) => {
                            eprintln!("Image decode error: {e}");
                            break;
                        }
                    }
                }
            }
            Err(e) => println!("Skipping image: {e}"),
        }
    }
}
