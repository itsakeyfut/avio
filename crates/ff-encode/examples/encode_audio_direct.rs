//! Encode audio directly using `AudioEncoder` without the pipeline abstraction.
//!
//! Demonstrates the low-level encode loop:
//! `AudioDecoder` → `AudioEncoder::create()` → `push()` → `finish()`.
//! This gives full frame-level control over the encode process.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example encode_audio_direct -- \
//!   --input   input.mp3   \
//!   --output  output.aac  \
//!   [--codec  aac]        # aac | mp3 | opus | flac (default: aac)
//!   [--bitrate 192000]    # target bitrate in bps (default: 192000)
//! ```

use std::{path::Path, process};

use ff_decode::AudioDecoder;
use ff_encode::AudioEncoder;
use ff_format::AudioCodec;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "aac".to_string();
    let mut bitrate: u64 = 192_000;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "aac".to_string()),
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().unwrap_or(192_000);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: encode_audio_direct --input <file> --output <file> \
             [--codec aac|mp3|opus|flac] [--bitrate N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let codec = match codec_str.to_lowercase().as_str() {
        "aac" => AudioCodec::Aac,
        "mp3" => AudioCodec::Mp3,
        "opus" => AudioCodec::Opus,
        "flac" => AudioCodec::Flac,
        other => {
            eprintln!("Unknown codec '{other}' (try aac, mp3, opus, flac)");
            process::exit(1);
        }
    };

    // Probe source parameters

    let probe_dec = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };

    let sample_rate = probe_dec.sample_rate();
    let channels = probe_dec.channels();
    let in_codec = probe_dec.stream_info().codec_name().to_string();
    drop(probe_dec);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!(
        "Input:    {in_name}  codec={in_codec}  sample_rate={sample_rate}  channels={channels}"
    );
    println!("Output:   {out_name}  codec={codec_str}  bitrate={bitrate}");
    println!();

    // Build encoder directly
    //
    // AudioEncoder::create() is the low-level entry point.
    // .audio() sets the output sample rate and channel count.
    // .audio_codec() chooses the codec.
    // .audio_bitrate() sets the target bitrate in bits per second.

    let mut encoder = match AudioEncoder::create(&output)
        .audio(sample_rate, channels)
        .audio_codec(codec)
        .audio_bitrate(bitrate)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    println!("Encoding...");

    // Manual decode → encode loop
    //
    // Open a second decoder for the actual decode loop (the first was probe-only).
    // push() feeds one decoded frame into the encoder.

    let mut decoder = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    let mut frames: u64 = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break, // end of stream
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = encoder.push(&frame) {
            eprintln!("Encode error: {e}");
            process::exit(1);
        }

        frames += 1;
    }

    // Flush and finalise
    //
    // finish() flushes buffered frames and writes the container trailer.

    if let Err(e) = encoder.finish() {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

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

    println!("Done. {out_name}  {size_str}  {frames} frames");
}
