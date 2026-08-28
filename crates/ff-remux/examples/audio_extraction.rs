//! Extract the audio track from a video file to a standalone audio file.
//!
//! Uses [`AudioExtractor`] to copy the audio stream out of a container without
//! re-encoding.  The output format is determined by the output file extension
//! (e.g., `.m4a`, `.aac`, `.mp3`, `.opus`).
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_extraction --features encode -- \
//!     --input  video.mp4     \
//!     --output audio.m4a
//!
//! # Extract a specific audio stream (0-based index):
//! cargo run --example audio_extraction --features encode -- \
//!     --input  video.mp4     \
//!     --output audio.m4a     \
//!     --stream 1
//! ```

use std::process;

use ff_remux::AudioExtractor;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut stream_index = None::<usize>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--stream" | "-s" => {
                let raw = args.next().unwrap_or_default();
                stream_index = Some(raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid stream index: {raw}");
                    process::exit(1);
                }));
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: audio_extraction --input <file> --output <audio> [--stream <index>]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    println!("Input:  {input}");
    println!(
        "Stream: {}",
        stream_index.map_or_else(
            || "first audio stream (default)".to_string(),
            |i| i.to_string()
        )
    );
    println!("Output: {output}");
    println!();
    println!("Extracting audio track (stream-copy, no re-encode)…");

    let mut extractor = AudioExtractor::new(&input, &output);
    if let Some(idx) = stream_index {
        extractor = extractor.stream_index(idx);
    }

    extractor.run().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

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
