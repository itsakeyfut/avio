//! Print container and stream metadata from a media file without a separate probe call.
//!
//! Demonstrates [`VideoDecoder::container_info`] and [`AudioDecoder::container_info`]
//! alongside the existing [`VideoDecoder::stream_info`] / [`AudioDecoder::stream_info`]
//! APIs. Both are available immediately after `.build()` at no extra file-open cost.
//!
//! # Usage
//!
//! ```bash
//! # Video file
//! cargo run --example decode_metadata -- --input video.mp4
//!
//! # Audio-only file
//! cargo run --example decode_metadata -- --input audio.mp3
//! ```

use std::process;
use std::time::Duration;

use ff_decode::{AudioDecoder, VideoDecoder};

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    let ms = d.subsec_millis();
    format!("{h:02}:{m:02}:{sec:02}.{ms:03}")
}

fn fmt_bitrate(bps: u64) -> String {
    if bps >= 1_000_000 {
        // Integer arithmetic: round to one decimal place without f64 cast precision issues
        let whole = bps / 1_000_000;
        let frac = (bps % 1_000_000) / 100_000;
        format!("{whole}.{frac} Mbps")
    } else {
        format!("{} kbps", bps / 1_000)
    }
}

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
        eprintln!("Usage: decode_metadata --input <file>");
        process::exit(1);
    });

    // Try video decoder first
    let video_result = VideoDecoder::open(&input).build();

    if let Ok(dec) = video_result {
        let ci = dec.container_info();
        let si = dec.stream_info();

        println!("Container");
        println!("  format:     {}", ci.format_name());
        match ci.bit_rate() {
            Some(br) => println!("  bit_rate:   {}", fmt_bitrate(br)),
            None => println!("  bit_rate:   (unknown)"),
        }
        println!("  nb_streams: {}", ci.nb_streams());
        println!();

        println!("Video stream");
        println!(
            "  codec:      {}  {}×{}  {:?}  {:.3} fps",
            si.codec_name(),
            si.width(),
            si.height(),
            si.pixel_format(),
            si.fps(),
        );
        match dec.duration_opt() {
            Some(d) => println!("  duration:   {}", fmt_duration(d)),
            None => println!("  duration:   (unknown — live stream or raw format)"),
        }
        return;
    }

    // Fall back to audio decoder
    let audio_result = AudioDecoder::open(&input).build();

    match audio_result {
        Ok(dec) => {
            let ci = dec.container_info();
            let si = dec.stream_info();

            println!("Container");
            println!("  format:     {}", ci.format_name());
            match ci.bit_rate() {
                Some(br) => println!("  bit_rate:   {}", fmt_bitrate(br)),
                None => println!("  bit_rate:   (unknown)"),
            }
            println!("  nb_streams: {}", ci.nb_streams());
            println!();

            println!("Audio stream");
            println!(
                "  codec:      {}  {} Hz  {}ch  {:?}",
                si.codec_name(),
                si.sample_rate(),
                si.channels(),
                si.sample_format(),
            );
            match dec.duration_opt() {
                Some(d) => println!("  duration:   {}", fmt_duration(d)),
                None => println!("  duration:   (unknown — live stream or raw format)"),
            }
        }
        Err(e) => {
            eprintln!("Error: could not open '{input}' as video or audio: {e}");
            process::exit(1);
        }
    }
}
