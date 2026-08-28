//! Decode an audio file and extract interleaved PCM samples.
//!
//! Demonstrates `AudioFrame::to_f32_interleaved()` and `to_i16_interleaved()`,
//! which absorb the complexity of planar/packed layout differences and
//! per-format byte interpretation.
//!
//! This replaces the common workaround of spawning `ffmpeg -f f32le ...` and
//! reading PCM from stdout.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example decode_to_pcm --features decode -- \
//!   --input  audio.mp3       \
//!   [--format f32|i16]       # output format (default: f32)
//!   [--frames 5]             # number of frames to inspect (default: 5)
//! ```

use std::process;

use ff_decode::AudioDecoder;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut format = "f32".to_string();
    let mut max_frames: usize = 5;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--format" | "-f" => format = args.next().unwrap_or_else(|| "f32".to_string()),
            "--frames" | "-n" => {
                let v = args.next().unwrap_or_default();
                max_frames = v.parse().unwrap_or(5);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: decode_to_pcm --input <file> [--format f32|i16] [--frames N]");
        process::exit(1);
    });

    let mut decoder = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening '{input}': {e}");
            process::exit(1);
        }
    };

    println!("Input:      {input}");
    println!("Channels:   {}", decoder.channels());
    println!("SampleRate: {} Hz", decoder.sample_rate());
    println!("Format:     PCM {}", format.to_uppercase());
    println!();

    let mut total_samples: usize = 0;
    let mut frame_count: usize = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        frame_count += 1;

        match format.as_str() {
            "f32" => {
                let pcm = frame.to_f32_interleaved();
                total_samples += pcm.len();

                if frame_count <= max_frames {
                    // Show the first 4 interleaved samples (up to 2 stereo pairs)
                    let preview: Vec<String> =
                        pcm.iter().take(4).map(|s| format!("{s:+.4}")).collect();
                    println!(
                        "frame {:>4}  samples={:>5}  native={:?}  pcm=[{}...]",
                        frame_count,
                        frame.sample_count(),
                        frame.format(),
                        preview.join(", "),
                    );
                }
            }
            "i16" => {
                let pcm = frame.to_i16_interleaved();
                total_samples += pcm.len();

                if frame_count <= max_frames {
                    let preview: Vec<String> =
                        pcm.iter().take(4).map(|s| format!("{s:>6}")).collect();
                    println!(
                        "frame {:>4}  samples={:>5}  native={:?}  pcm=[{}...]",
                        frame_count,
                        frame.sample_count(),
                        frame.format(),
                        preview.join(", "),
                    );
                }
            }
            other => {
                eprintln!("Unknown format '{other}' (use f32 or i16)");
                process::exit(1);
            }
        }

        if frame_count == max_frames {
            println!("  ... (showing first {max_frames} frames)");
        }
    }

    println!();
    println!("Decoded {frame_count} frames, {total_samples} interleaved PCM samples total.");
    println!(
        "  → Ready for rodio::buffer::SamplesBuffer::new({ch}, {sr}, pcm)",
        ch = decoder.channels(),
        sr = decoder.sample_rate(),
    );
}
