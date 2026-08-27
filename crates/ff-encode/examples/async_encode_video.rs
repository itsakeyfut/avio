//! Async video encoding with back-pressure.
//!
//! Demonstrates two patterns for [`AsyncVideoEncoder`]:
//!
//! 1. **Basic async encode** — `AsyncVideoDecoder` streams frames directly into
//!    `AsyncVideoEncoder::push()`. The encoder's bounded channel (capacity 8)
//!    suspends `push()` automatically when the worker thread falls behind,
//!    preventing unbounded memory growth.
//!
//! 2. **Producer/consumer** — a dedicated producer task decodes frames and sends
//!    them through a `tokio::sync::mpsc` channel. The main task drains the
//!    channel and feeds the encoder. Two levels of back-pressure work together:
//!    the inter-task channel and the encoder's internal channel.
//!
//! This is the async counterpart of the `encode_video_direct` example.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example async_encode_video --features tokio -- \
//!   --input  input.mp4  \
//!   --output output.mp4
//! ```

use std::{path::Path, process};

use ff_decode::{AsyncVideoDecoder, VideoDecoder};
use ff_encode::{AsyncVideoEncoder, VideoEncoder};
use ff_format::VideoCodec;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: async_encode_video --input <file> --output <file>");
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

    // ── Probe source dimensions ───────────────────────────────────────────────
    //
    // AsyncVideoDecoder does not expose metadata methods; use the sync decoder
    // briefly to read width, height, and frame rate before building the encoder.

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };
    let width = probe.width();
    let height = probe.height();
    let fps = probe.frame_rate();
    let in_codec = probe.stream_info().codec_name().to_string();
    drop(probe);

    println!("Input:  {in_name}  {width}×{height}  {fps:.2} fps  codec={in_codec}");
    println!("Output: {out_name}  {width}×{height}  codec=h264");
    println!();

    // ── Pattern 1: basic async encode ────────────────────────────────────────
    //
    // AsyncVideoEncoder::from_builder() is the entry point.
    //   - Pass any VideoEncoderBuilder configured via VideoEncoder::create().
    //   - from_builder() calls .build() internally, opens the output file, and
    //     starts a dedicated worker thread running the sync FFmpeg encode loop.
    //
    // push(frame).await queues each frame into a bounded channel (capacity 8).
    //   - When the channel is full, push() suspends the caller (back-pressure).
    //   - The worker thread drains the channel at the encoding rate.
    //   - This prevents the producer from allocating an unbounded frame buffer
    //     when the encoder is slower than the decoder.
    //
    // finish().await sends the Finish sentinel, drops the sender, and joins
    // the worker thread via spawn_blocking so the executor is never blocked.

    println!("=== Pattern 1: basic async encode ===");

    let mut encoder = match AsyncVideoEncoder::from_builder(
        VideoEncoder::create(&output)
            .video(width, height, fps)
            .video_codec(VideoCodec::H264),
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    let decoder = match AsyncVideoDecoder::open(input.clone()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    let mut frames: u64 = 0;
    let stream = decoder.into_stream();
    tokio::pin!(stream);

    while let Some(result) = stream.next().await {
        match result {
            Ok(frame) => {
                encoder.push(frame).await?;
                frames += 1;
            }
            Err(e) => {
                eprintln!("Decode error: {e}");
                break;
            }
        }
    }

    encoder.finish().await?;

    let size_str = file_size_str(&output);
    println!("Done. {out_name}  {size_str}  {frames} frames encoded");
    println!();

    // ── Pattern 2: producer / consumer with separate task ─────────────────────
    //
    // Decoding and encoding can run concurrently on separate tasks.
    // A tokio::sync::mpsc channel connects the producer (decoder) to the
    // consumer (encoder). Two levels of back-pressure work together:
    //
    //   1. The inter-task channel (capacity 16) throttles the producer when
    //      the consumer's loop is blocked by the encoder's channel.
    //
    //   2. The encoder's internal channel (capacity 8) throttles push() when
    //      the worker thread falls behind at the FFmpeg encode rate.
    //
    // When the producer exits (decoder exhausted), the sender is dropped,
    // which closes the channel and causes rx.recv() to return None — the
    // standard Tokio MPSC shutdown protocol.

    println!("=== Pattern 2: producer / consumer with separate task ===");

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    // Producer: decode frames on a background task, send to channel.
    let producer_input = input.clone();
    let producer = tokio::spawn(async move {
        match AsyncVideoDecoder::open(producer_input).await {
            Ok(decoder) => {
                let stream = decoder.into_stream();
                tokio::pin!(stream);
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(frame) => {
                            if tx.send(frame).await.is_err() {
                                // Consumer dropped the receiver; stop producing.
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("Producer decode error: {e}");
                            break;
                        }
                    }
                }
            }
            Err(e) => eprintln!("Producer open error: {e}"),
        }
    });

    // Consumer: drain channel, push frames to encoder.
    let mut encoder2 = match AsyncVideoEncoder::from_builder(
        VideoEncoder::create(&output)
            .video(width, height, fps)
            .video_codec(VideoCodec::H264),
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    let mut frames2: u64 = 0;
    while let Some(frame) = rx.recv().await {
        encoder2.push(frame).await?;
        frames2 += 1;
    }

    encoder2.finish().await?;

    // Wait for the producer task to finish cleanly.
    producer.await?;

    let size_str2 = file_size_str(&output);
    println!("Done. {out_name}  {size_str2}  {frames2} frames encoded");

    Ok(())
}

fn file_size_str(path: &str) -> String {
    match std::fs::metadata(path) {
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
    }
}
