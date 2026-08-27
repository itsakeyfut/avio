//! Generate an animated GIF preview from a time range in a video file.
//!
//! Uses [`GifPreview`] to extract a short clip, scale it to the target width
//! (height scales proportionally), and encode it as an animated GIF.  GIF
//! previews are useful for social media thumbnails, README animations, and
//! hover-preview widgets.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example gif_preview --features encode -- \
//!     --input  video.mp4   \
//!     --output preview.gif
//!
//! # Custom range, frame rate, and width:
//! cargo run --example gif_preview --features encode -- \
//!     --input    video.mp4   \
//!     --output   preview.gif \
//!     --start    5           \
//!     --duration 4           \
//!     --fps      12          \
//!     --width    480
//! ```

use std::process;
use std::time::Duration;

use ff_encode::GifPreview;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut start_secs = 0u64;
    let mut duration_secs = 3u64;
    let mut fps = 10.0_f64;
    let mut width = 320u32;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--start" => {
                let raw = args.next().unwrap_or_default();
                start_secs = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid start: {raw}");
                    process::exit(1);
                });
            }
            "--duration" => {
                let raw = args.next().unwrap_or_default();
                duration_secs = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid duration: {raw}");
                    process::exit(1);
                });
            }
            "--fps" => {
                let raw = args.next().unwrap_or_default();
                fps = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid fps: {raw}");
                    process::exit(1);
                });
            }
            "--width" => {
                let raw = args.next().unwrap_or_default();
                width = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid width: {raw}");
                    process::exit(1);
                });
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: gif_preview --input <video> --output <gif> \
             [--start <secs>] [--duration <secs>] [--fps <f>] [--width <px>]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    println!("Input:    {input}");
    println!(
        "Range:    {start_secs}s + {duration_secs}s  →  {}s – {}s",
        start_secs,
        start_secs + duration_secs
    );
    println!("FPS:      {fps}");
    println!("Width:    {width} px  (height scales proportionally)");
    println!("Output:   {output}");
    println!();
    println!("Generating animated GIF…");

    GifPreview::new(&input)
        .start(Duration::from_secs(start_secs))
        .duration(Duration::from_secs(duration_secs))
        .fps(fps)
        .width(width)
        .output(&output)
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
