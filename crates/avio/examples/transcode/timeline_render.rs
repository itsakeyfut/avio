//! Arrange two clips on a video track and render them with `Timeline::render()`.
//!
//! Demonstrates the [`Timeline`] and [`Clip`] APIs. Two clips are placed on a
//! single video track: the first starts at the beginning of the timeline, the
//! second is offset by five seconds. `Timeline::render()` builds the filter
//! graph and encodes the composition to the output file in one call.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example timeline_render --features filter,pipeline -- \
//!   --clip-a  intro.mp4   \
//!   --clip-b  main.mp4    \
//!   --output  rendered.mp4
//! ```

use std::{path::PathBuf, process, time::Duration};

use avio::{Clip, EncoderConfig, Timeline};

// Argument parsing

struct Args {
    clip_a: PathBuf,
    clip_b: PathBuf,
    output: PathBuf,
}

fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let get = |flag: &str| -> Option<String> {
        args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    };

    let clip_a = if let Some(p) = get("--clip-a") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --clip-a <path> is required");
        process::exit(1);
    };
    let clip_b = if let Some(p) = get("--clip-b") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --clip-b <path> is required");
        process::exit(1);
    };
    let output = if let Some(p) = get("--output") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --output <path> is required");
        process::exit(1);
    };

    Args {
        clip_a,
        clip_b,
        output,
    }
}

// Main

fn main() {
    let args = parse_args();

    // 1. Build clips

    // clip_a starts at the beginning of the timeline.
    let clip_a = Clip::new(&args.clip_a);

    // clip_b is offset by 5 seconds on the timeline.
    let clip_b = Clip::new(&args.clip_b).offset(Duration::from_secs(5));

    // 2. Build timeline

    let timeline = match Timeline::builder()
        .canvas(1280, 720)
        .frame_rate(30.0)
        .video_track(vec![clip_a, clip_b])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: failed to build timeline: {e}");
            process::exit(1);
        }
    };

    println!(
        "rendering {} video track(s) → {}",
        timeline.video_tracks().len(),
        args.output.display(),
    );

    // 3. Render

    let config = EncoderConfig::builder().build();

    if let Err(e) = timeline.render(&args.output, config) {
        eprintln!("error: timeline render failed: {e}");
        process::exit(1);
    }

    println!("done: {}", args.output.display());
}
