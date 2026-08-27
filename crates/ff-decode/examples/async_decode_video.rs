//! Async video decoding — frame-by-frame and stream API.
//!
//! Demonstrates three patterns for consuming [`AsyncVideoDecoder`]:
//!
//! 1. **Frame-by-frame** with `decode_frame()` — the async counterpart of
//!    the sync `decode_one()` loop, useful when you need precise control over
//!    each iteration.
//!
//! 2. **Stream API** with `into_stream()` — converts the decoder into a
//!    `futures::Stream` so you can apply standard stream combinators
//!    (`filter_map`, `fold`, `count`, etc.) without a manual loop.
//!
//! 3. **Spawn on a task** — the stream is `Send`, so it can be moved into a
//!    `tokio::spawn` block for background processing.
//!
//! This is the async counterpart of the `decode_iterator` example.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example async_decode_video --features tokio -- --input input.mp4
//! ```

use std::{path::Path, process};

use ff_decode::AsyncVideoDecoder;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: async_decode_video --input <file>");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input: {in_name}");
    println!();

    // ── 1. Frame-by-frame: decode_frame() ────────────────────────────────────
    //
    // decode_frame() offloads each FFmpeg call to spawn_blocking, so the
    // Tokio executor is never blocked by codec I/O or CPU-bound decoding work.
    //
    // Returns Ok(Some(frame)) for each frame, Ok(None) at end of stream.

    println!("=== Pattern 1: frame-by-frame with decode_frame() ===");

    let mut decoder = match AsyncVideoDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping (open failed): {e}");
            return Ok(());
        }
    };

    let mut frame_count: u64 = 0;

    loop {
        match decoder.decode_frame().await {
            Ok(Some(frame)) => {
                if frame_count == 0 {
                    println!(
                        "First frame: {}x{}  pts={:?}",
                        frame.width(),
                        frame.height(),
                        frame.timestamp(),
                    );
                }
                frame_count += 1;
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                break;
            }
        }
    }

    println!("Decoded {frame_count} frames");
    println!();

    // ── 2. Stream API: into_stream() + StreamExt combinators ─────────────────
    //
    // into_stream() converts the decoder into an impl Stream<Item = Result<..>>.
    // This composes naturally with the futures::StreamExt trait methods:
    //   filter_map — skip errors and unwrap Ok frames
    //   count       — consume the stream, returning the total number of items
    //
    // The decoder is consumed by into_stream() so it cannot be used afterwards.

    println!("=== Pattern 2: stream API with into_stream() ===");

    let decoder = match AsyncVideoDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping (open failed): {e}");
            return Ok(());
        }
    };

    let total = decoder
        .into_stream()
        .filter_map(|result| async move { result.ok() })
        .count()
        .await;

    println!("Total frames via stream: {total}");
    println!();

    // ── 3. Spawn stream on a background task ─────────────────────────────────
    //
    // into_stream() returns impl Stream + Send, which means it can be moved
    // into a tokio::spawn block. This is useful when you want to process frames
    // in the background while the main task continues doing other work.
    //
    // The join handle is awaited here to collect the result, but in a real
    // application the task could run concurrently with other work.

    println!("=== Pattern 3: stream spawned on a background task ===");

    let decoder = match AsyncVideoDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping (open failed): {e}");
            return Ok(());
        }
    };

    let handle = tokio::spawn(async move {
        let mut count: u64 = 0;
        let stream = decoder.into_stream();
        tokio::pin!(stream);
        while let Some(result) = stream.next().await {
            match result {
                Ok(_frame) => count += 1,
                Err(e) => {
                    eprintln!("Background decode error: {e}");
                    break;
                }
            }
        }
        count
    });

    let background_count = handle.await?;
    println!("Background task decoded {background_count} frames");

    Ok(())
}
