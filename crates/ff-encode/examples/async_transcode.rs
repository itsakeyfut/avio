//! Async transcode — end-to-end `AsyncVideoDecoder` → `AsyncVideoEncoder`.
//!
//! Demonstrates two approaches to async video transcoding:
//!
//! 1. **Sequential** — decoder stream feeds directly into the encoder. Simple
//!    and idiomatic; back-pressure flows naturally from the encoder's bounded
//!    channel (capacity 8) back to the stream consumer.
//!
//! 2. **Concurrent decode + encode** — a `tokio::spawn` decode task produces
//!    frames into a `tokio::sync::mpsc` channel; the main task consumes from
//!    the channel and encodes. Decoding and encoding run in parallel, which
//!    can improve throughput on multi-core machines.
//!
//! Error paths are handled explicitly in both patterns; neither relies on
//! `unwrap()` or ignores errors from spawned tasks.
//!
//! This is the async counterpart of the `transcode` / `video_transcode`
//! examples and serves as the flagship example for the v0.6.0 async API.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example async_transcode --features tokio -- \
//!   --input  input.mp4   \
//!   --output output.mp4  \
//!   [--codec h264|h265|vp9|av1]
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
    let mut codec_str = "h264".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "h264".to_string()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: async_transcode --input <file> --output <file> \
             [--codec h264|h265|vp9|av1]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let codec = match codec_str.to_lowercase().as_str() {
        "h264" | "avc" => VideoCodec::H264,
        "h265" | "hevc" => VideoCodec::H265,
        "vp9" => VideoCodec::Vp9,
        "av1" => VideoCodec::Av1,
        other => {
            eprintln!("Unknown codec '{other}' (try h264, h265, vp9, av1)");
            process::exit(1);
        }
    };

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
    // AsyncVideoDecoder does not expose metadata; use the sync decoder briefly
    // to read width, height, and frame rate before building the encoder.

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
    println!("Output: {out_name}  {width}×{height}  codec={codec_str}");
    println!();

    // ── Pattern 1: sequential async transcode ────────────────────────────────
    //
    // The decoder's into_stream() produces frames one at a time on a
    // spawn_blocking thread. Each frame is pushed to the encoder, which queues
    // it in its bounded internal channel (capacity 8).
    //
    // When the channel is full, push().await suspends the loop — back-pressure
    // propagates naturally from the encoder back to the stream consumer without
    // any explicit coordination.
    //
    // into_stream() returns impl Stream + Send but not Unpin, so tokio::pin!
    // is required before calling .next().await.

    println!("=== Pattern 1: sequential async transcode ===");
    println!("Encoding...");

    let mut encoder = match AsyncVideoEncoder::from_builder(
        VideoEncoder::create(&output)
            .video(width, height, fps)
            .video_codec(codec),
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
                if let Err(e) = encoder.push(frame).await {
                    eprintln!("Encode error: {e}");
                    break;
                }
                frames += 1;
            }
            Err(e) => {
                eprintln!("Decode error: {e}");
                break;
            }
        }
    }

    if let Err(e) = encoder.finish().await {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

    let size_str = file_size_str(&output);
    println!("Done. {out_name}  {size_str}  {frames} frames");
    println!();

    // ── Pattern 2: concurrent decode + encode ────────────────────────────────
    //
    // Decoding and encoding are split across two concurrent tasks connected by
    // a tokio::sync::mpsc channel. The decoder runs as a background task and
    // produces frames into the channel; the main task drains the channel and
    // feeds the encoder.
    //
    // Two sources of back-pressure:
    //   1. The inter-task channel (capacity 16) throttles the producer when the
    //      consumer's push().await is blocked.
    //   2. The encoder's internal channel (capacity 8) throttles push() when
    //      the FFmpeg worker thread falls behind.
    //
    // Error handling:
    //   - Encode errors are surfaced from the main loop.
    //   - Decode errors are returned from the spawned task as
    //     Box<dyn Error + Send + Sync> and re-raised after finish().
    //   - The channel is dropped by the producer on exit, which causes
    //     rx.recv() to return None — the standard MPSC shutdown protocol.

    println!("=== Pattern 2: concurrent decode + encode ===");
    println!("Encoding...");

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    let decode_input = input.clone();
    let decode_task = tokio::spawn(async move {
        let decoder = AsyncVideoDecoder::open(decode_input)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let stream = decoder.into_stream();
        tokio::pin!(stream);

        while let Some(result) = stream.next().await {
            let frame =
                result.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            if tx.send(frame).await.is_err() {
                // Encoder dropped the receiver; stop producing.
                break;
            }
        }

        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });

    let mut encoder2 = match AsyncVideoEncoder::from_builder(
        VideoEncoder::create(&output)
            .video(width, height, fps)
            .video_codec(codec),
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    let mut frames2: u64 = 0;
    while let Some(frame) = rx.recv().await {
        if let Err(e) = encoder2.push(frame).await {
            eprintln!("Encode error: {e}");
            break;
        }
        frames2 += 1;
    }

    if let Err(e) = encoder2.finish().await {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

    // Wait for the decode task and surface any decode error.
    match decode_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("Decode task error: {e}"),
        Err(e) => eprintln!("Decode task panicked: {e}"),
    }

    let size_str2 = file_size_str(&output);
    println!("Done. {out_name}  {size_str2}  {frames2} frames");

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
