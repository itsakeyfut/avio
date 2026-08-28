//! Encode a video file using a predefined `ExportPreset`.
//!
//! [`ExportPreset`] bundles codec, bitrate, resolution, and audio settings
//! into a named preset ready for social media or archival workflows.
//! This example uses [`ExportPreset::youtube_1080p`] (H.264 CRF 18, AAC
//! 192 kbps) and demonstrates the `apply_video` / `apply_audio` builder
//! pattern that wires preset settings onto a [`VideoEncoder`].
//!
//! Available presets: `youtube_1080p`, `youtube_4k`, `twitter`,
//! `instagram_square`, `instagram_reels`, `bluray_1080p`, `podcast_mono`,
//! `lossless_rgb`, `web_h264`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example export_preset -- \
//!     --input  input.mp4     \
//!     --output output.mp4
//!
//! cargo run --example export_preset -- \
//!     --input  input.mp4     \
//!     --output output.mp4    \
//!     --preset twitter
//! ```

use std::process;

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_encode::{ExportPreset, VideoEncoder};
use ff_format::SampleFormat;

fn preset_by_name(name: &str) -> Option<ExportPreset> {
    match name {
        "youtube_1080p" => Some(ExportPreset::youtube_1080p()),
        "youtube_4k" => Some(ExportPreset::youtube_4k()),
        "twitter" => Some(ExportPreset::twitter()),
        "instagram_square" => Some(ExportPreset::instagram_square()),
        "instagram_reels" => Some(ExportPreset::instagram_reels()),
        "bluray_1080p" => Some(ExportPreset::bluray_1080p()),
        "podcast_mono" => Some(ExportPreset::podcast_mono()),
        "lossless_rgb" => Some(ExportPreset::lossless_rgb()),
        "web_h264" => Some(ExportPreset::web_h264()),
        _ => None,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut preset_name = "youtube_1080p".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--preset" | "-p" => {
                preset_name = args.next().unwrap_or_else(|| "youtube_1080p".to_string());
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: export_preset --input <file> --output <file> \
             [--preset youtube_1080p|youtube_4k|twitter|instagram_square|\
             instagram_reels|bluray_1080p|podcast_mono|lossless_rgb|web_h264]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let preset = preset_by_name(&preset_name).unwrap_or_else(|| {
        eprintln!("Unknown preset: {preset_name}");
        process::exit(1);
    });

    let expect_video = preset.video.is_some();

    println!("Preset:  {} (video={})", preset.name, expect_video);
    println!("Input:   {input}");
    println!("Output:  {output}");
    println!();

    // Probe source dimensions and frame rate

    let (width, height, fps) = if expect_video {
        match VideoDecoder::open(&input).build() {
            Ok(dec) => (dec.width(), dec.height(), dec.frame_rate()),
            Err(e) => {
                eprintln!("Error probing video: {e}");
                process::exit(1);
            }
        }
    } else {
        (0, 0, 0.0)
    };

    // Build encoder from preset
    //
    // apply_video() wires codec, bitrate mode, pixel format, and codec options.
    // apply_audio() wires sample rate, channel count, audio codec, and bitrate.
    // We override resolution after apply_video so the encoder uses the actual
    // source dimensions rather than the preset's native resolution.

    let builder = VideoEncoder::create(&output);
    let builder = preset.apply_video(builder);
    let builder = if expect_video {
        builder.video(width, height, fps)
    } else {
        builder
    };
    let builder = preset.apply_audio(builder);

    let mut encoder = match builder.build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    println!("Encoding…");

    // Push video frames

    if expect_video {
        let mut vdec = match VideoDecoder::open(&input).build() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error opening video decoder: {e}");
                process::exit(1);
            }
        };

        let mut frames: u64 = 0;
        loop {
            match vdec.decode_one() {
                Ok(Some(frame)) => {
                    if let Err(e) = encoder.push_video(&frame) {
                        eprintln!("Video encode error: {e}");
                        process::exit(1);
                    }
                    frames += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Video decode error: {e}");
                    process::exit(1);
                }
            }
        }
        println!("  Video: {frames} frame(s)");
    }

    // Push audio frames

    let sr = preset.audio.sample_rate;
    let ch = preset.audio.channels;

    let mut adec = match AudioDecoder::open(&input)
        .output_format(SampleFormat::F32)
        .output_sample_rate(sr)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening audio decoder: {e}");
            process::exit(1);
        }
    };

    let _ = ch; // sample_rate and channels are applied by apply_audio()
    let mut audio_frames: u64 = 0;
    loop {
        match adec.decode_one() {
            Ok(Some(frame)) => {
                if let Err(e) = encoder.push_audio(&frame) {
                    eprintln!("Audio encode error: {e}");
                    process::exit(1);
                }
                audio_frames += 1;
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("Audio decode error: {e}");
                process::exit(1);
            }
        }
    }
    println!("  Audio: {audio_frames} frame(s)");

    // Finalize

    if let Err(e) = encoder.finish() {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

    let size = match std::fs::metadata(&output) {
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

    println!("Done. {output}  {size}");
}
