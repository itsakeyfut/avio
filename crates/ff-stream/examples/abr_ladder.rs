//! Generate an adaptive bitrate HLS or DASH stream using `AbrLadder`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example abr_ladder --features stream -- \
//!   --input   input.mp4 \
//!   --output  ./abr/    \
//!   [--format hls]      \
//!   [--ladder 1080,6000000:720,3000000:480,1500000]
//! ```

use std::{path::Path, process};

use ff_probe::open;
use ff_stream::{AbrLadder, Rendition, StreamError};

// Default rendition ladder (height_px, bitrate_bps)
const DEFAULT_LADDER: &[(u32, u64)] = &[(1080, 6_000_000), (720, 3_000_000), (480, 1_500_000)];

fn parse_ladder(s: &str) -> Result<Vec<(u32, u64)>, String> {
    s.split(':')
        .map(|pair| {
            let mut parts = pair.splitn(2, ',');
            let height: u32 = parts
                .next()
                .ok_or_else(|| format!("missing height in '{pair}'"))?
                .parse()
                .map_err(|_| format!("invalid height in '{pair}'"))?;
            let bitrate: u64 = parts
                .next()
                .ok_or_else(|| format!("missing bitrate in '{pair}'"))?
                .parse()
                .map_err(|_| format!("invalid bitrate in '{pair}'"))?;
            Ok((height, bitrate))
        })
        .collect()
}

fn warn_if_not_descending(ladder: &[(u32, u64)]) {
    for i in 1..ladder.len() {
        if ladder[i].1 > ladder[i - 1].1 {
            println!(
                "  Warning: rendition #{i} bitrate {} > rendition #{} bitrate {} — expected descending order",
                ladder[i].1,
                i - 1,
                ladder[i - 1].1
            );
        }
    }
}

fn dir_summary(output: &str) -> (Vec<(String, u64)>, u64) {
    let Ok(entries) = std::fs::read_dir(output) else {
        return (Vec::new(), 0);
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
    let total: u64 = files.iter().map(|(_, s)| s).sum();
    (files, total)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut format = "hls".to_string();
    let mut ladder_str = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--format" | "-f" => format = args.next().unwrap_or_else(|| "hls".to_string()),
            "--ladder" => ladder_str = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: abr_ladder --input <file> --output <dir> [--format hls|dash] [--ladder H,BPS:H,BPS]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let format_lower = format.to_lowercase();
    if format_lower != "hls" && format_lower != "dash" {
        eprintln!("Error: --format must be 'hls' or 'dash'");
        process::exit(1);
    }

    // Parse ladder
    let ladder_pairs = match ladder_str {
        Some(ref s) => parse_ladder(s).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        }),
        None => DEFAULT_LADDER.to_vec(),
    };

    // Probe source to get aspect ratio for width computation
    let src_info = match open(&input) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let (src_w, src_h) = src_info
        .video_streams()
        .first()
        .map_or((1920, 1080), |v| (v.width(), v.height()));

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let dur = src_info.duration();
    let dur_secs = dur.as_secs();
    let dur_str = format!(
        "{:02}:{:02}:{:02}",
        dur_secs / 3600,
        (dur_secs % 3600) / 60,
        dur_secs % 60
    );

    println!("Input:   {in_name}  ({src_w}×{src_h}  {dur_str})");
    println!("Format:  {}", format_lower.to_uppercase());
    println!("Output:  {output}");
    println!();

    // Build renditions, computing width from aspect ratio
    let aspect = if src_h > 0 {
        f64::from(src_w) / f64::from(src_h)
    } else {
        16.0 / 9.0
    };

    println!("Renditions:");
    warn_if_not_descending(&ladder_pairs);
    let mut abr = AbrLadder::new(&input);
    for (i, &(height, bitrate)) in ladder_pairs.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = ((f64::from(height) * aspect).round() as u32 / 2) * 2; // ensure even
        println!("  #{i}  {width}×{height}  {bitrate} bps");
        abr = abr.add_rendition(Rendition {
            width,
            height,
            bitrate,
        });
    }
    println!();

    // Create output directory
    if let Err(e) = std::fs::create_dir_all(&output) {
        eprintln!("Error: cannot create output directory: {e}");
        process::exit(1);
    }

    print!("Encoding...");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let result = if format_lower == "hls" {
        abr.hls(&output)
    } else {
        abr.dash(&output)
    };

    match result {
        Ok(()) => println!(" done."),
        Err(StreamError::Ffmpeg { code, message }) => {
            println!();
            eprintln!("Error: FFmpeg failed: {message} (code={code})");
            process::exit(1);
        }
        Err(e) => {
            println!();
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }

    println!();
    println!("Output structure:");

    if format_lower == "hls" {
        // master.m3u8 + subdirs
        let (top_files, _) = dir_summary(&output);
        for (name, size) in &top_files {
            #[allow(clippy::cast_precision_loss)]
            let kb = *size as f64 / 1024.0;
            if std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u8"))
            {
                println!("  {name}  ({kb:.1} KB)");
            }
        }
        for i in 0..ladder_pairs.len() {
            let subdir = Path::new(&output).join(i.to_string());
            let subdir_str = subdir.to_string_lossy();
            let (sub_files, sub_total) = dir_summary(&subdir_str);
            let seg_count = sub_files
                .iter()
                .filter(|(n, _)| {
                    std::path::Path::new(n)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
                })
                .count();
            #[allow(clippy::cast_precision_loss)]
            let sub_mb = sub_total as f64 / 1_048_576.0;
            println!("  {i}/  playlist.m3u8 + {seg_count} segments  ({sub_mb:.1} MB)");
        }
    } else {
        // Single manifest.mpd
        let manifest = Path::new(&output).join("manifest.mpd");
        if manifest.exists() {
            let size = manifest.metadata().map_or(0, |m| m.len());
            #[allow(clippy::cast_precision_loss)]
            let kb = size as f64 / 1024.0;
            println!("  manifest.mpd  ({kb:.1} KB)");
        }
        println!("  (segment files alongside manifest.mpd)");
    }

    println!();
    if format_lower == "hls" {
        println!("Serve with: npx serve {output}  (open http://localhost:3000/master.m3u8)");
    } else {
        println!("Serve with: npx serve {output}  (open http://localhost:3000/manifest.mpd)");
    }
}
