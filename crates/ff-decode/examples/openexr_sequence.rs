//! Decode an `OpenEXR` image sequence to individual frames.
//!
//! Demonstrates `OpenEXR` sequence support added in v0.7.0:
//!
//! - `VideoDecoder::open()` with a printf-style `%` pattern (e.g.
//!   `frames/frame%04d.exr`) uses the `image2` demuxer automatically.
//! - `HardwareAccel::None` — hardware decoders do not support EXR; software
//!   decoding is mandatory.
//! - `PixelFormat::Gbrpf32le` — EXR files decode to three 32-bit float planes
//!   ordered G, B, R (matching the EXR channel naming convention).
//! - `VideoFrame::plane()` — access individual colour planes by index.
//! - `VideoFrame::num_planes()` — returns `3` for `gbrpf32le` frames.
//!
//! The example skips gracefully when the EXR decoder is absent from the
//! `FFmpeg` build (`--enable-decoder=exr` is an optional configure flag).
//!
//! # Usage
//!
//! ```bash
//! cargo run --example openexr_sequence --features "decode" -- \
//!   --input "frames/frame%04d.exr"  \
//!   [--fps 24]
//! ```
//!
//! # Creating a test sequence
//!
//! If you have `ffmpeg` on your PATH, you can generate a 3-frame EXR sequence
//! from any image (requires `--enable-encoder=exr` in your `FFmpeg` build):
//!
//! ```bash
//! mkdir -p /tmp/exr_seq
//! ffmpeg -loop 1 -i input.png -vf "format=gbrpf32le" \
//!        -frames:v 3 "/tmp/exr_seq/frame%04d.exr"
//! ```

use std::{path::Path, process};

use ff_decode::{HardwareAccel, VideoDecoder};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut fps: u32 = 24;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--fps" => {
                let v = args.next().unwrap_or_default();
                fps = v.parse().unwrap_or(24);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: openexr_sequence --input \"frame%04d.exr\" [--fps 24]");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input:  {in_name}  fps={fps}");
    println!();

    // ── Open the EXR sequence ─────────────────────────────────────────────────
    //
    // VideoDecoder::open() detects the `%` in the path and uses the `image2`
    // demuxer automatically.  .frame_rate() overrides the default 25 fps.
    //
    // .hardware_accel(HardwareAccel::None) is required because hardware
    // decoders do not support the EXR codec.
    let mut decoder = match VideoDecoder::open(&input)
        .hardware_accel(HardwareAccel::None)
        .frame_rate(fps)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening EXR sequence: {e}");
            eprintln!(
                "Note: EXR decoding requires FFmpeg built with \
                 --enable-decoder=exr"
            );
            process::exit(1);
        }
    };

    let width = decoder.width();
    let height = decoder.height();
    let actual_fps = decoder.frame_rate();

    println!("Sequence: {width}×{height}  actual_fps={actual_fps:.2}");

    // ── Decode loop ───────────────────────────────────────────────────────────
    //
    // EXR frames decode as gbrpf32le (32-bit float, planar, G/B/R order).
    // Each plane holds one colour channel; plane(0)=G, plane(1)=B, plane(2)=R.

    let mut frames: u64 = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error at frame {frames}: {e}");
                process::exit(1);
            }
        };

        // Report the pixel format and plane layout for the first frame.
        if frames == 0 {
            println!(
                "Frame 0: format={:?}  planes={}",
                frame.format(),
                frame.num_planes(),
            );
            for i in 0..frame.num_planes() {
                if let Some(plane) = frame.plane(i) {
                    let float_count = plane.len() / 4; // 4 bytes per f32
                    println!(
                        "  plane[{i}]: {} bytes ({float_count} f32 values)",
                        plane.len()
                    );
                }
            }
        }

        frames += 1;
    }

    println!();
    println!("Done. {frames} EXR frames decoded from '{in_name}'");
}
