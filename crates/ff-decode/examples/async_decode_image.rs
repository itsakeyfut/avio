//! Async image decoding — single decode and parallel multi-image.
//!
//! Demonstrates two patterns for [`AsyncImageDecoder`]:
//!
//! 1. **Single decode** — `open().await?.decode().await?` produces a
//!    [`VideoFrame`] with width, height, pixel format, and plane data.
//!
//! 2. **Parallel decode** — because each `AsyncImageDecoder` is independent and
//!    its futures are `Send`, multiple images can be decoded concurrently with
//!    [`futures::future::join_all`]. This is the primary advantage over the sync
//!    decoder: wall-clock time scales with the longest image, not the sum.
//!
//! This is the async counterpart of the `decode_image` example.
//!
//! # Usage
//!
//! ```bash
//! # Single image
//! cargo run --example async_decode_image --features tokio -- --input photo.jpg
//!
//! # Multiple images decoded in parallel
//! cargo run --example async_decode_image --features tokio -- \
//!   --input a.jpg --input b.png --input c.jpg
//! ```

use std::{path::Path, process};

use ff_decode::{AsyncImageDecoder, DecodeError};
use futures::future;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut inputs: Vec<String> = Vec::new();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => inputs.push(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    if inputs.is_empty() {
        eprintln!("Usage: async_decode_image --input <file> [--input <file> ...]");
        process::exit(1);
    }

    // 1. Single async decode
    //
    // open() runs file I/O and codec initialisation on a spawn_blocking thread.
    // decode() runs the pixel conversion on a spawn_blocking thread and consumes
    // the decoder, returning a VideoFrame with the full pixel data.
    //
    // Unlike the sync ImageDecoder, both steps are non-blocking from the
    // perspective of the Tokio executor.

    println!("=== Pattern 1: single async decode ===");

    let first = inputs[0].clone();
    let file_name = Path::new(&first)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&first)
        .to_owned();

    match AsyncImageDecoder::open(first).await {
        Ok(decoder) => match decoder.decode().await {
            Ok(frame) => {
                println!("File:         {file_name}");
                println!("Dimensions:   {}×{}", frame.width(), frame.height());
                println!("Pixel format: {}", frame.format());
                println!("Planes:       {}", frame.num_planes());
                for i in 0..frame.num_planes() {
                    let stride = frame.stride(i).unwrap_or(0);
                    let plane_len = frame.plane(i).map_or(0, |p| p.len());
                    let label = match i {
                        0 => " (Y)",
                        1 => " (U)",
                        2 => " (V)",
                        _ => "",
                    };
                    println!("  plane {i}{label}  stride={stride}  size={plane_len} bytes");
                }
                println!("Total size:   {} bytes", frame.total_size());
            }
            Err(e) => eprintln!("Decode error: {e}"),
        },
        Err(DecodeError::FileNotFound { path }) => eprintln!("File not found: {}", path.display()),
        Err(e) => eprintln!("Open error: {e}"),
    }

    if inputs.len() == 1 {
        return Ok(());
    }

    // 2. Parallel decode with join_all
    //
    // Each AsyncImageDecoder::open().decode() chain is an independent future.
    // Because the futures are Send, join_all can poll them concurrently on the
    // multi-threaded Tokio runtime.
    //
    // Wall-clock time is determined by the slowest image, not the sum of all
    // decodes — the key advantage over sequential sync decoding.

    println!();
    println!(
        "=== Pattern 2: parallel decode with join_all ({} images) ===",
        inputs.len()
    );

    let decode_futures: Vec<_> = inputs
        .iter()
        .map(|path| {
            let path = path.clone();
            async move {
                let file_name = Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path)
                    .to_owned();
                let result = async { AsyncImageDecoder::open(path).await?.decode().await }.await;
                (file_name, result)
            }
        })
        .collect();

    let results = future::join_all(decode_futures).await;

    for (file_name, result) in results {
        match result {
            Ok(frame) => println!(
                "  {file_name}: {}×{}  format={}  total={} bytes",
                frame.width(),
                frame.height(),
                frame.format(),
                frame.total_size(),
            ),
            Err(e) => eprintln!("  {file_name}: error — {e}"),
        }
    }

    Ok(())
}
