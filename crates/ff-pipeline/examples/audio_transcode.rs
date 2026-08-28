//! Decode an audio file and re-encode it to a different codec or bitrate.
//!
//! Demonstrates the audio-only decode → encode pipeline using `AudioPipeline`
//! — a high-level builder that wraps the manual decode/encode loop.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_transcode -- \
//!   --input   input.mp3  \
//!   --output  output.aac \
//!   [--codec  aac]        # aac | mp3 | opus | flac (default: aac)
//!   [--bitrate 128000]    # target bitrate in bps (default: 128000)
//! ```

use std::{path::Path, process, time::Duration};

use ff_decode::AudioDecoder;
use ff_format::AudioCodec;
use ff_pipeline::AudioPipeline;

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    format!("{h:02}:{m:02}:{sec:02}")
}

fn codec_label(c: AudioCodec) -> &'static str {
    match c {
        AudioCodec::Aac => "AAC",
        AudioCodec::Mp3 => "MP3",
        AudioCodec::Opus => "Opus",
        AudioCodec::Flac => "FLAC",
        _ => "unknown",
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "aac".to_string();
    let mut bitrate: u64 = 128_000;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "aac".to_string()),
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().unwrap_or(128_000);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: audio_transcode --input <file> --output <file> \
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

    // Probe input for display info

    let dec = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let sample_rate = dec.sample_rate();
    let channels = dec.channels();
    let duration = dec.duration();
    let in_codec = dec.stream_info().codec_name().to_string();
    drop(dec);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    let in_codec_display = if in_codec.is_empty() {
        String::new()
    } else {
        format!("  {in_codec}")
    };
    println!(
        "Input:   {in_name}  {channels}ch  {sample_rate} Hz{in_codec_display}  {}",
        format_duration(duration)
    );
    println!(
        "Output:  {out_name}  {channels}ch  {sample_rate} Hz  {}  bitrate={bitrate}",
        codec_label(codec)
    );
    println!();
    println!("Encoding...");

    // Run pipeline

    if let Err(e) = AudioPipeline::new()
        .input(&input)
        .output(&output)
        .audio_codec(codec)
        .bitrate(bitrate)
        .run()
    {
        eprintln!("Error: {e}");
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

    println!("Done. {out_name}  {size_str}");
}
