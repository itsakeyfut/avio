//! Trim a clip without re-encoding using `StreamCopyTrimmer`.
//!
//! Stream-copy trimming is fast — it copies codec data directly without decoding
//! or encoding.  The output will start on the nearest keyframe at or before
//! `--start`, so the actual trim point may differ slightly from the requested one.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example clip_trim -- \
//!   --input   input.mp4  \
//!   --output  trimmed.mp4 \
//!   --start   10.0        \   # start time in seconds (default: 0.0)
//!   --end     30.0            # end time in seconds (default: 10.0)
//! ```

use std::{path::Path, process};

use ff_remux::StreamCopyTrimmer;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut start: f64 = 0.0;
    let mut end: f64 = 10.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--start" => start = args.next().unwrap_or_default().parse().unwrap_or(0.0),
            "--end" => end = args.next().unwrap_or_default().parse().unwrap_or(10.0),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: clip_trim --input <file> --output <file> \
             --start <seconds> --end <seconds>"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:   {in_name}");
    println!("Trim:    {start:.3}s → {end:.3}s  (stream copy — no re-encode)");
    println!("Output:  {out_name}");
    println!();

    if let Err(e) = StreamCopyTrimmer::new(&input, start, end, &output).run() {
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
