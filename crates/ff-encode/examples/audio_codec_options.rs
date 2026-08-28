//! Encode audio with per-codec options via `AudioCodecOptions`.
//!
//! Demonstrates the `AudioCodecOptions` enum and the codec-specific option
//! structs added in v0.7.0:
//!
//! - `OpusOptions` — application mode (`Voip` / `Audio` / `LowDelay`),
//!   frame duration; stored in an OGG container via `OutputContainer::Ogg`
//! - `AacOptions` — profile (`LC` / `HE` / `HEv2`), VBR quality mode
//! - `Mp3Options` — VBR quality (`V0`–`V9`) or fixed bitrate
//! - `FlacOptions` — compression level (`0` = fastest / largest …
//!   `12` = slowest / smallest); stored in a FLAC container via
//!   `OutputContainer::Flac`
//!
//! Also demonstrates `AudioEncoder::create().container()` — explicit
//! container selection for audio-only formats (`OutputContainer::Flac`,
//! `OutputContainer::Ogg`).
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_codec_options -- \
//!   --input   input.mp3   \
//!   --output  output.opus  \
//!   --codec   opus          # opus | aac | mp3 | flac  (default: opus)
//! ```

use std::{path::Path, process};

use ff_decode::AudioDecoder;
use ff_encode::{
    AacOptions, AacProfile, AudioCodecOptions, AudioEncoder, FlacOptions, Mp3Options, Mp3Quality,
    OpusApplication, OpusOptions, OutputContainer,
};
use ff_format::AudioCodec;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "opus".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "opus".to_string()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: audio_codec_options --input <file> --output <file> \
             [--codec opus|aac|mp3|flac]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // Probe source

    let probe = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };
    let sample_rate = probe.sample_rate();
    let channels = probe.channels();
    drop(probe);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:  {in_name}  {sample_rate} Hz  {channels} ch");

    // Select codec + AudioCodecOptions
    //
    // AudioCodecOptions is the v0.7.0 API for per-codec audio configuration.
    // Each variant holds a typed options struct specific to that codec.

    // Each arm returns (codec, options, bitrate, container, description).
    // container — Some(c) sets the muxer explicitly; None infers from the
    //   output file extension.  Audio-only containers (OutputContainer::Flac,
    //   OutputContainer::Ogg) are typically paired with their native codec.
    let (audio_codec, codec_options, bitrate, container, description) =
        match codec_str.to_lowercase().as_str() {
            "opus" => {
                // OpusOptions: application mode controls the psychoacoustic model.
                //   Audio    — general music and voice (default)
                //   Voip     — optimised for low-latency voice
                //   LowDelay — minimal algorithmic delay
                //
                // Opus is stored natively in an OGG container.
                // OutputContainer::Ogg is set explicitly here so that the muxer is
                // always correct regardless of the output file extension.
                let opts = OpusOptions {
                    application: OpusApplication::Audio,
                    frame_duration_ms: Some(20),
                };
                (
                    AudioCodec::Opus,
                    AudioCodecOptions::Opus(opts),
                    128_000u64,
                    Some(OutputContainer::Ogg),
                    "Opus, Audio application, 128 kbps, 20 ms frames, OGG container",
                )
            }
            "aac" => {
                // AacOptions: profile selects the AAC variant.
                //   Lc   — Low Complexity, compatible with all devices (default)
                //   He   — HE-AAC v1 (SBR) — better quality at low bitrates
                //   Hev2 — HE-AAC v2 (SBR + PS) — stereo only, very low bitrates
                let opts = AacOptions {
                    profile: AacProfile::Lc,
                    vbr_quality: None, // None = CBR at the specified bitrate
                };
                (
                    AudioCodec::Aac,
                    AudioCodecOptions::Aac(opts),
                    192_000u64,
                    None, // infer container from extension (e.g. .m4a, .mp4)
                    "AAC-LC, CBR 192 kbps",
                )
            }
            "mp3" => {
                // Mp3Options: VBR quality scale 0 (best) … 9 (smallest).
                // Vbr(2) corresponds to libmp3lame -V2, ~190 kbps average.
                // Use Mp3Quality::Cbr(bitrate) for a fixed bitrate instead.
                let opts = Mp3Options {
                    quality: Mp3Quality::Vbr(2), // ~190 kbps average
                };
                (
                    AudioCodec::Mp3,
                    AudioCodecOptions::Mp3(opts),
                    0u64, // irrelevant in VBR mode
                    None, // infer container from .mp3 extension
                    "MP3, VBR quality V2 (~190 kbps average)",
                )
            }
            "flac" => {
                // FlacOptions: compression level 0 (fastest encode, largest file)
                // through 12 (slowest encode, smallest lossless file).
                // Default is 5 — a good balance for most use cases.
                //
                // FLAC has a dedicated container (OutputContainer::Flac) that wraps
                // the raw FLAC stream — the same format produced by a standalone
                // FLAC encoder.  OutputContainer::Ogg can also hold FLAC streams in
                // an Ogg wrapper, but the native FLAC container is more common.
                let opts = FlacOptions {
                    compression_level: 6,
                };
                (
                    AudioCodec::Flac,
                    AudioCodecOptions::Flac(opts),
                    0u64, // lossless — bitrate is determined by content
                    Some(OutputContainer::Flac),
                    "FLAC, compression level 6, FLAC container",
                )
            }
            other => {
                eprintln!("Unknown codec '{other}' (try opus, aac, mp3, flac)");
                process::exit(1);
            }
        };

    println!("Codec:  {description}");
    println!("Output: {out_name}");
    println!();

    // Build encoder
    //
    // .codec_options() applies the per-codec option struct.
    // .audio_bitrate() sets the target bitrate (ignored by lossless codecs and
    // VBR-quality modes such as Mp3Quality::Vbr).
    // .container() overrides the muxer; None lets FFmpeg infer from the path.

    let mut enc_builder = AudioEncoder::create(&output)
        .audio(sample_rate, channels)
        .audio_codec(audio_codec)
        .audio_bitrate(bitrate)
        .codec_options(codec_options);

    if let Some(c) = container {
        enc_builder = enc_builder.container(c);
    }

    let mut encoder = match enc_builder.build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    // Decode + encode loop

    let mut decoder = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    let mut chunks: u64 = 0;

    loop {
        let chunk = match decoder.decode_one() {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = encoder.push(&chunk) {
            eprintln!("Encode error: {e}");
            process::exit(1);
        }

        chunks += 1;
    }

    if let Err(e) = encoder.finish() {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

    #[allow(clippy::cast_precision_loss)]
    let kb = std::fs::metadata(&output).map_or(0, |m| m.len()) as f64 / 1024.0;
    let size_str = if kb < 1024.0 {
        format!("{kb:.0} KB")
    } else {
        format!("{:.1} MB", kb / 1024.0)
    };

    println!("Done. {out_name}  {size_str}  {chunks} audio chunks");
}
