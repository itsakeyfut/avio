//! Dump raw PCM bytes from decoded audio frames to a `.raw` file.
//!
//! Demonstrates `AudioFrame::data()` and `AudioFrame::channel()`:
//!
//! - **Packed formats** (F32, I16, U8, …): `data()` returns the interleaved
//!   byte slice directly — one contiguous buffer, ready to write.
//! - **Planar formats** (F32p, I16p, …): `data()` returns an empty slice.
//!   Each channel must be read with `channel(n)` and the bytes interleaved
//!   by hand before writing.
//!
//! The output file contains raw little-endian PCM and can be played back with:
//!
//! ```bash
//! ffplay -f f32le -ar 48000 -ac 2 output.raw
//! ```
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_raw_dump --features decode -- \
//!   --input  audio.mp3        \
//!   --output output.raw       \
//!   [--frames 100]            # max frames to dump (default: all)
//! ```

use std::io::Write as _;
use std::{fs, process};

use ff_decode::AudioDecoder;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut max_frames = usize::MAX;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--frames" | "-n" => {
                let v = args.next().unwrap_or_default();
                max_frames = v.parse().unwrap_or(usize::MAX);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: audio_raw_dump --input <file> --output <file> [--frames N]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let mut decoder = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening '{input}': {e}");
            process::exit(1);
        }
    };

    let channels = decoder.channels() as usize;
    let sample_rate = decoder.sample_rate();

    let mut out_file = match fs::File::create(&output) {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!("Cannot create '{output}': {e}");
            process::exit(1);
        }
    };

    println!("Input:       {input}");
    println!("Output:      {output}  (raw PCM, little-endian)");
    println!("Channels:    {channels}");
    println!("Sample rate: {sample_rate} Hz");
    println!();

    let mut frame_count = 0usize;
    let mut bytes_written: u64 = 0;
    let mut first_frame_logged = false;

    loop {
        if frame_count >= max_frames {
            break;
        }

        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        let fmt = frame.format();
        let bps = frame.bytes_per_sample();

        // Log the layout of the first frame so the caller knows what they're
        // getting and how to play it back with ffplay.
        if !first_frame_logged {
            first_frame_logged = true;
            let layout = if fmt.is_packed() { "packed" } else { "planar" };
            println!("Native format: {fmt:?}  ({layout}, {bps} bytes/sample)");
            println!(
                "Tip: ffplay -f {fmt_name} -ar {sample_rate} -ac {channels} {output}",
                fmt_name = ffplay_format_name(fmt),
            );
            println!();
        }

        if fmt.is_packed() {
            // ── Packed: data() gives the whole interleaved plane ──────────────
            //
            // Layout: [L0 R0 L1 R1 ...] where each sample is `bps` bytes.
            // data() is non-empty because there is exactly one plane.
            let bytes = frame.data();
            debug_assert!(!bytes.is_empty(), "packed frame must have non-empty data()");
            if let Err(e) = out_file.write_all(bytes) {
                eprintln!("Write error: {e}");
                process::exit(1);
            }
            bytes_written += bytes.len() as u64;
        } else {
            // ── Planar: data() returns &[] — use channel() instead ────────────
            //
            // Layout: plane[0] = [L0 L1 L2 ...], plane[1] = [R0 R1 R2 ...]
            // We interleave by cycling through samples across all channels.
            debug_assert!(
                frame.data().is_empty(),
                "planar frame must have empty data()"
            );
            let samples_per_channel = frame.samples();
            for sample_idx in 0..samples_per_channel {
                for ch in 0..channels {
                    if let Some(plane) = frame.channel(ch) {
                        let start = sample_idx * bps;
                        let end = start + bps;
                        if end <= plane.len() {
                            if let Err(e) = out_file.write_all(&plane[start..end]) {
                                eprintln!("Write error: {e}");
                                process::exit(1);
                            }
                            bytes_written += bps as u64;
                        }
                    }
                }
            }
        }

        frame_count += 1;
    }

    if let Err(e) = out_file.flush() {
        eprintln!("Flush error: {e}");
        process::exit(1);
    }

    println!("Dumped {frame_count} frames, {bytes_written} bytes → {output}");
}

/// Returns the ffplay `-f` format name for a given sample format.
fn ffplay_format_name(fmt: ff_format::SampleFormat) -> &'static str {
    use ff_format::SampleFormat;
    match fmt {
        SampleFormat::U8 | SampleFormat::U8p => "u8",
        SampleFormat::I16 | SampleFormat::I16p => "s16le",
        SampleFormat::I32 | SampleFormat::I32p => "s32le",
        SampleFormat::F32 | SampleFormat::F32p => "f32le",
        SampleFormat::F64 | SampleFormat::F64p => "f64le",
        SampleFormat::Other(_) | _ => "f32le",
    }
}
