//! Encode a video at a precise target bitrate using two-pass encoding.
//!
//! Two-pass encoding produces better quality at a given file size than
//! single-pass CBR, making it the standard approach for distribution targets
//! such as streaming platform upload limits or archival at exact target sizes.
//!
//! Both passes are handled internally by `VideoEncoder` when `.two_pass()` is
//! set — the encoder buffers frames during the analysis pass then re-encodes
//! them in the output pass automatically when `finish()` is called.
//!
//! Note: two-pass encoding is video-only; audio cannot be included.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example two_pass_encode -- \
//!   --input   input.mp4  \
//!   --output  output.mp4 \
//!   --bitrate 4000000     # target bitrate in bps (required)
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::BitrateMode;
use ff_format::VideoCodec;
use ff_pipeline::{EncoderConfig, Pipeline};

fn format_bitrate(bps: u64) -> String {
    // Insert non-breaking spaces every 3 digits for readability.
    let s = (bps / 1000).to_string();
    let mut chars: Vec<char> = s.chars().collect();
    let mut i = chars.len().saturating_sub(3);
    while i > 0 {
        chars.insert(i, '\u{a0}');
        i = i.saturating_sub(3);
    }
    chars.into_iter().collect::<String>() + " kbps"
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut bitrate = None::<u64>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().ok();
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: two_pass_encode --input <file> --output <file> --bitrate <bps>");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let bitrate = bitrate.unwrap_or_else(|| {
        eprintln!("--bitrate is required (e.g. --bitrate 4000000)");
        process::exit(1);
    });

    // Probe source

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let src_w = probe.width();
    let src_h = probe.height();
    let in_codec = probe.stream_info().codec_name().to_string();
    let dur = probe.duration();
    drop(probe); // release file handle before encoding

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);
    let dur_secs = dur.as_secs();
    let dur_str = format!(
        "{:02}:{:02}:{:02}",
        dur_secs / 3600,
        (dur_secs % 3600) / 60,
        dur_secs % 60
    );

    println!("Input:   {in_name}  {src_w}×{src_h}  {in_codec}  {dur_str}");
    println!(
        "Output:  {out_name}  H.264  CBR  {}  (two-pass, video-only)",
        format_bitrate(bitrate)
    );
    println!();
    println!("Encoding (both passes handled internally)...");

    // Run pipeline

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(bitrate))
        .build();

    if let Err(e) = Pipeline::builder()
        .input(&input)
        .output(&output, config)
        .two_pass()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        })
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
