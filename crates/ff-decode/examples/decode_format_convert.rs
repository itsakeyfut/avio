//! Decode video and audio with explicit output format conversion.
//!
//! Demonstrates:
//! - `VideoDecoderBuilder::output_format()` — request a specific pixel format
//!   (e.g. `PixelFormat::Rgb24` for image-processing pipelines)
//! - `AudioDecoderBuilder::output_format()` — request a specific sample format
//!   (e.g. `SampleFormat::I16` for audio mixing / playback)
//! - `AudioDecoderBuilder::output_sample_rate()` — resample to a target rate
//!
//! `FFmpeg` performs the conversion automatically inside the decoder; the caller
//! simply requests the desired format and receives already-converted frames.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example decode_format_convert -- \
//!   --input input.mp4
//! ```

use std::{path::Path, process};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_format::{PixelFormat, SampleFormat};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: decode_format_convert --input <file>");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input: {in_name}");
    println!();

    // Video: decode to RGB24
    //
    // output_format() instructs FFmpeg to convert the decoded frame to the
    // requested pixel format before returning it. This avoids a separate
    // swscale call in user code.
    //
    // Common targets:
    //   PixelFormat::Rgb24   — packed RGB for image processing / display
    //   PixelFormat::Rgba    — packed RGBA for compositing
    //   PixelFormat::Yuv420p — planar YUV for software encoders

    println!("=== Video: output_format(PixelFormat::Rgb24) ===");

    let mut vdec = match VideoDecoder::open(&input)
        .output_format(PixelFormat::Rgb24)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping video: {e}");
            return;
        }
    };

    let src_w = vdec.width();
    let src_h = vdec.height();
    println!(
        "Source:   {src_w}×{src_h}  codec={}",
        vdec.stream_info().codec_name()
    );

    match vdec.decode_one() {
        Ok(Some(frame)) => {
            println!(
                "Frame:    {}×{}  format={}  planes={}  total_bytes={}",
                frame.width(),
                frame.height(),
                frame.format(),
                frame.num_planes(),
                frame.total_size(),
            );
            // With Rgb24, all pixel data is in a single packed plane.
            let plane = frame.plane(0);
            println!(
                "Plane[0]: stride={}  data_len={}",
                frame.stride(0).unwrap_or(0),
                plane.map_or(0, |p| p.len()),
            );
        }
        Ok(None) => println!("No frames in file"),
        Err(e) => println!("Decode error: {e}"),
    }
    println!();

    // Audio: decode to I16 at 44 100 Hz
    //
    // output_format(SampleFormat::I16) converts the decoded samples to
    // signed 16-bit interleaved PCM — the standard format for CD audio,
    // WAV files, and most playback APIs.
    //
    // output_sample_rate() adds a resampler so frames arrive at exactly
    // the requested rate, regardless of the source file's native rate.
    //
    // Common targets:
    //   SampleFormat::I16  — 16-bit signed PCM (compact, wide support)
    //   SampleFormat::F32  — 32-bit float (lossless mixing, audio editing)
    //   SampleFormat::F32p — 32-bit float planar (common FFmpeg decoder output)

    println!("=== Audio: output_format(SampleFormat::I16) + output_sample_rate(44100) ===");

    let mut adec = match AudioDecoder::open(&input)
        .output_format(SampleFormat::I16)
        .output_sample_rate(44_100)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping audio: {e}");
            return;
        }
    };

    println!(
        "Source:   sample_rate={}  channels={}  codec={}",
        adec.sample_rate(),
        adec.channels(),
        adec.stream_info().codec_name()
    );

    match adec.decode_one() {
        Ok(Some(frame)) => {
            println!(
                "Frame:    samples={}  channels={}  sample_rate={}  format={}",
                frame.samples(),
                frame.channels(),
                frame.sample_rate(),
                frame.format(),
            );
            let plane = frame.data();
            println!("Plane[0]: data_len={}  bytes_per_sample=2", plane.len());
        }
        Ok(None) => println!("No audio frames in file"),
        Err(e) => println!("Decode error: {e}"),
    }
}
