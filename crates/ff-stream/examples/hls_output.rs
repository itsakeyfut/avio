//! Package a video as an HLS stream using `HlsOutput`.
//!
//! Demonstrates:
//! - `HlsOutput::new()` — create an HLS output builder
//! - `HlsOutput::input()` — set the source file
//! - `HlsOutput::segment_duration()` — target segment length
//! - `HlsOutput::keyframe_interval()` — force an IDR every N frames
//! - `HlsOutput::bitrate()` — target video bitrate
//! - `HlsOutput::build()` / `write()` — finalise and write all segments
//!
//! `keyframe_interval(N)` inserts a forced keyframe every N frames so that
//! HLS segment boundaries fall exactly on IDR frames. The default is 48.
//! Set it to `fps * segment_duration` for clean segment alignment.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example hls_output --features stream -- \
//!   --input              input.mp4  \
//!   --output             ./hls/     \
//!   [--segment           6]         \
//!   [--keyframe-interval 48]        \
//!   [--bitrate           2000000]
//! ```

use std::{path::Path, process, time::Duration};

use ff_stream::HlsOutput;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut segment_secs: u64 = 6;
    let mut bitrate = None::<u64>;
    let mut keyframe_interval = None::<u32>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--segment" | "-s" => {
                let v = args.next().unwrap_or_default();
                segment_secs = v.parse().unwrap_or(6);
            }
            "--keyframe-interval" | "-k" => {
                let v = args.next().unwrap_or_default();
                keyframe_interval = v.parse().ok();
            }
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
        eprintln!(
            "Usage: hls_output --input <file> --output <dir> \
             [--segment N] [--keyframe-interval N] [--bitrate N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // Create output directory
    if let Err(e) = std::fs::create_dir_all(&output) {
        eprintln!("Error: cannot create output directory: {e}");
        process::exit(1);
    }

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    // Print header
    println!("Input:    {in_name}");
    println!("Output:   {output}");
    println!("Segment:  {segment_secs} s");
    if let Some(kfi) = keyframe_interval {
        println!("Keyframe: every {kfi} frames");
    }
    if let Some(br) = bitrate {
        println!("Bitrate:  {br} bps");
    }
    println!();
    println!("Writing HLS segments...");

    // ── Build HLS output ──────────────────────────────────────────────────────
    //
    // keyframe_interval(N) forces an IDR frame every N frames.
    // The default is 48; for clean segment alignment use fps × segment_duration.

    let mut builder = HlsOutput::new(&output)
        .input(&input)
        .segment_duration(Duration::from_secs(segment_secs));

    if let Some(kfi) = keyframe_interval {
        builder = builder.keyframe_interval(kfi);
    }

    if let Some(br) = bitrate {
        builder = builder.bitrate(br);
    }

    let hls = match builder.build() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = hls.write() {
        eprintln!("Error: {e}");
        process::exit(1);
    }

    println!("Done.");
    println!();

    // List output directory
    let entries = match std::fs::read_dir(&output) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Warning: cannot list output: {e}");
            return;
        }
    };

    let mut files: Vec<(String, u64)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let size = e.metadata().ok()?.len();
            Some((name, size))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let segment_count = files
        .iter()
        .filter(|(n, _)| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
        })
        .count();
    let total_bytes: u64 = files.iter().map(|(_, s)| s).sum();

    println!("Output directory:");
    for (name, size) in &files {
        #[allow(clippy::cast_precision_loss)]
        let kb = *size as f64 / 1024.0;
        if kb < 1024.0 {
            println!("  {name:<30}  ({kb:.1} KB)");
        } else {
            println!("  {name:<30}  ({:.1} MB)", kb / 1024.0);
        }
    }
    println!();
    #[allow(clippy::cast_precision_loss)]
    let total_mb = total_bytes as f64 / 1_048_576.0;
    println!("Total: {segment_count} segments  {total_mb:.1} MB");
    println!("Serve with: npx serve {output}  (open http://localhost:3000/playlist.m3u8)");
}
