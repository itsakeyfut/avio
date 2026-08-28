//! Async audio decoding — frame-by-frame and stream API.
//!
//! Demonstrates three patterns for consuming [`AsyncAudioDecoder`]:
//!
//! 1. **Frame-by-frame** with `decode_frame()` — the async counterpart of
//!    the sync `decode_one()` loop, useful when you need per-frame control.
//!
//! 2. **Stream API** with `into_stream()` — converts the decoder into a
//!    `futures::Stream` that composes naturally with stream combinators.
//!    Demonstrates counting total decoded samples with `map` + `fold`.
//!
//! 3. **Spawn on a task** — the stream is `Send`, so it can be moved into
//!    a `tokio::spawn` block for background processing.
//!
//! This is the async counterpart of the `decode_iterator` example's audio section.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example async_decode_audio --features tokio -- --input input.mp3
//! ```

use std::{path::Path, process};

use ff_decode::AsyncAudioDecoder;
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
        eprintln!("Usage: async_decode_audio --input <file>");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input: {in_name}");
    println!();

    // 1. Frame-by-frame: decode_frame()
    //
    // decode_frame() offloads each FFmpeg call to spawn_blocking, keeping the
    // Tokio executor free of blocking codec and I/O work.
    //
    // Returns Ok(Some(frame)) for each decoded frame, Ok(None) at end of stream.
    // AudioFrame exposes samples(), channels(), sample_rate(), and format().

    println!("=== Pattern 1: frame-by-frame with decode_frame() ===");

    let mut decoder = match AsyncAudioDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping (open failed): {e}");
            return Ok(());
        }
    };

    let mut frame_count: u64 = 0;
    let mut total_samples: u64 = 0;

    loop {
        match decoder.decode_frame().await {
            Ok(Some(frame)) => {
                if frame_count == 0 {
                    println!(
                        "First frame: samples={}  channels={}  sample_rate={}  format={}",
                        frame.samples(),
                        frame.channels(),
                        frame.sample_rate(),
                        frame.format(),
                    );
                }
                total_samples += frame.samples() as u64;
                frame_count += 1;
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                break;
            }
        }
    }

    println!("Decoded {frame_count} frames  ({total_samples} samples)");
    println!();

    // 2. Stream API: into_stream() + sample counting
    //
    // into_stream() produces impl Stream<Item = Result<AudioFrame, DecodeError>>.
    // Chaining StreamExt combinators:
    //   filter_map — discard errors, unwrap Ok frames
    //   map         — extract the sample count from each frame
    //   fold        — accumulate into a running total
    //
    // This is equivalent to the frame-by-frame loop but expressed as a
    // composable pipeline with no explicit loop or mutable accumulator.

    println!("=== Pattern 2: stream API with into_stream() — sample counting ===");

    let decoder = match AsyncAudioDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping (open failed): {e}");
            return Ok(());
        }
    };

    let stream_samples: u64 = decoder
        .into_stream()
        .filter_map(|result| async move { result.ok() })
        .map(|frame| frame.samples() as u64)
        .fold(0u64, |acc, n| async move { acc + n })
        .await;

    println!("Total samples via stream: {stream_samples}");
    println!();

    // 3. Spawn stream on a background task
    //
    // into_stream() returns impl Stream + Send, so the decoder can be moved
    // into tokio::spawn for background processing alongside other async work.
    //
    // The stream is not Unpin, so tokio::pin! is required before iterating
    // with .next().await inside the spawned task.

    println!("=== Pattern 3: stream spawned on a background task ===");

    let decoder = match AsyncAudioDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping (open failed): {e}");
            return Ok(());
        }
    };

    let handle = tokio::spawn(async move {
        let mut total: u64 = 0;
        let stream = decoder.into_stream();
        tokio::pin!(stream);
        while let Some(result) = stream.next().await {
            match result {
                Ok(frame) => total += frame.samples() as u64,
                Err(e) => {
                    eprintln!("Background decode error: {e}");
                    break;
                }
            }
        }
        total
    });

    let background_samples = handle.await?;
    println!("Background task counted {background_samples} samples");

    Ok(())
}
