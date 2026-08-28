//! Package a video as a single-rendition DASH stream using `DashOutput`.
//!
//! Pairs with `hls_output.rs` to show both adaptive streaming formats
//! side-by-side, helping you choose between HLS and DASH for your deployment.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example dash_output --features stream -- \
//!   --input    input.mp4  \
//!   --output   ./dash/    \
//!   [--segment 4]          # segment duration in seconds (default: 4)
//! ```

use std::{path::Path, process, time::Duration};

use ff_stream::DashOutput;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut segment_secs: u64 = 4;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--segment" | "-s" => {
                let v = args.next().unwrap_or_default();
                segment_secs = v.parse().unwrap_or(4);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: dash_output --input <file> --output <dir> [--segment N]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    if let Err(e) = std::fs::create_dir_all(&output) {
        eprintln!("Error: cannot create output directory: {e}");
        process::exit(1);
    }

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input:    {in_name}");
    println!("Output:   {output}");
    println!("Segment:  {segment_secs} s");
    println!();
    println!("Writing DASH segments...");

    let dash = match DashOutput::new(&output)
        .input(&input)
        .segment_duration(Duration::from_secs(segment_secs))
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = dash.write() {
        eprintln!("Error: {e}");
        process::exit(1);
    }

    println!("Done.");
    println!();

    // ── List output directory ─────────────────────────────────────────────────

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
                .is_some_and(|ext| ext.eq_ignore_ascii_case("m4s"))
        })
        .count();
    let total_bytes: u64 = files.iter().map(|(_, s)| s).sum();

    println!("Output directory:");
    for (name, size) in &files {
        #[allow(clippy::cast_precision_loss)]
        let kb = *size as f64 / 1024.0;
        if kb < 1024.0 {
            println!("  {name:<40}  ({kb:.1} KB)");
        } else {
            println!("  {name:<40}  ({:.1} MB)", kb / 1024.0);
        }
    }
    println!();
    #[allow(clippy::cast_precision_loss)]
    let total_mb = total_bytes as f64 / 1_048_576.0;
    println!("Total: {segment_count} segments  {total_mb:.1} MB");
    println!("Serve with: npx serve {output}  (open http://localhost:3000/manifest.mpd)");
}
