//! Trim a media file to a time range without re-encoding.
//!
//! Uses [`StreamCopyTrimmer`] to perform a fast stream-copy trim.  Both video
//! and audio streams are preserved as-is; only the container timestamps are
//! adjusted.  The output starts at the nearest keyframe before `start`, so
//! the first few frames of the output may be from slightly before the
//! requested start time.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example stream_copy_trim --features encode -- \
//!     --input  input.mp4  \
//!     --start  10.0       \
//!     --end    30.0       \
//!     --output trimmed.mp4
//! ```

use std::process;

use ff_remux::StreamCopyTrimmer;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut start_sec = 0.0_f64;
    let mut end_sec = None::<f64>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--start" => {
                let raw = args.next().unwrap_or_default();
                start_sec = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid start: {raw}");
                    process::exit(1);
                });
            }
            "--end" => {
                let raw = args.next().unwrap_or_default();
                end_sec = Some(raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid end: {raw}");
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
        eprintln!(
            "Usage: stream_copy_trim --input <file> --start <secs> --end <secs> --output <file>"
        );
        process::exit(1);
    });
    let end_sec = end_sec.unwrap_or_else(|| {
        eprintln!("--end is required");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    if start_sec >= end_sec {
        eprintln!("Error: --start ({start_sec}) must be less than --end ({end_sec})");
        process::exit(1);
    }

    let duration = end_sec - start_sec;
    println!("Input:    {input}");
    println!("Range:    {start_sec:.3}s – {end_sec:.3}s  ({duration:.3}s)");
    println!("Output:   {output}");
    println!();
    println!("Trimming (stream-copy, no re-encode)…");

    StreamCopyTrimmer::new(&input, start_sec, end_sec, &output)
        .run()
        .unwrap_or_else(|e| {
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
