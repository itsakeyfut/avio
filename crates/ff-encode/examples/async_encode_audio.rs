//! Async audio encoding with back-pressure.
//!
//! Demonstrates two patterns for [`AsyncAudioEncoder`]:
//!
//! 1. **Basic async encode** — `AsyncAudioDecoder` streams frames directly into
//!    `AsyncAudioEncoder::push()`. The encoder's bounded channel (capacity 8)
//!    suspends `push()` when the worker thread falls behind.
//!
//! 2. **Streaming from an async source** — simulates an audio capture or
//!    network stream scenario: a producer task sends frames through a
//!    `tokio::sync::mpsc` channel; the main task encodes them as they arrive.
//!    This is the typical pattern for real-time audio pipelines where frames
//!    arrive asynchronously (microphone, network, live stream).
//!
//! **Why async for audio?** The sync `AudioEncoder` blocks the calling thread
//! until `FFmpeg` processes each frame. The async wrapper offloads that work to a
//! dedicated worker thread, allowing the async executor to stay responsive for
//! audio capture, UI updates, or other concurrent tasks.
//!
//! This is the async counterpart of the `encode_audio_direct` example.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example async_encode_audio --features tokio -- \
//!   --input  input.mp3  \
//!   --output output.aac \
//!   [--codec aac|mp3|opus|flac]  \
//!   [--bitrate 192000]
//! ```

use std::{path::Path, process};

use ff_decode::{AsyncAudioDecoder, AudioDecoder};
use ff_encode::{AsyncAudioEncoder, AudioEncoder};
use ff_format::AudioCodec;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "aac".to_string();
    let mut bitrate: u64 = 192_000;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "aac".to_string()),
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().unwrap_or(192_000);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: async_encode_audio --input <file> --output <file> \
             [--codec aac|mp3|opus|flac] [--bitrate N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let codec = match codec_str.to_lowercase().as_str() {
        "aac" => AudioCodec::Aac,
        "mp3" => AudioCodec::Mp3,
        "opus" => AudioCodec::Opus,
        "flac" => AudioCodec::Flac,
        other => {
            eprintln!("Unknown codec '{other}' (try aac, mp3, opus, flac)");
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

    // ── Probe source parameters ───────────────────────────────────────────────
    //
    // AsyncAudioDecoder does not expose metadata; use the sync decoder briefly
    // to read sample_rate and channels before building the async encoder.

    let probe = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };
    let sample_rate = probe.sample_rate();
    let channels = probe.channels();
    let in_codec = probe.stream_info().codec_name().to_string();
    drop(probe);

    println!("Input:  {in_name}  codec={in_codec}  sample_rate={sample_rate}  channels={channels}");
    println!("Output: {out_name}  codec={codec_str}  bitrate={bitrate}");
    println!();

    // ── Pattern 1: basic async encode ────────────────────────────────────────
    //
    // AsyncAudioEncoder::from_builder() consumes an AudioEncoderBuilder,
    // calls .build() internally to open the output file, and starts a worker
    // thread running the synchronous FFmpeg encode loop.
    //
    // push(frame).await queues the frame into a bounded channel (capacity 8).
    //   - Suspends the caller when the channel is full (back-pressure).
    //   - Prevents memory growth if the encoder is slower than the source.
    //
    // finish().await sends the Finish sentinel, drops the sender, and joins
    // the worker via spawn_blocking — the async executor is never blocked.

    println!("=== Pattern 1: basic async encode ===");

    let mut encoder = match AsyncAudioEncoder::from_builder(
        AudioEncoder::create(&output)
            .audio(sample_rate, channels)
            .audio_codec(codec)
            .audio_bitrate(bitrate),
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    let decoder = match AsyncAudioDecoder::open(input.clone()).await {
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

    // ── Pattern 2: streaming from an async source ─────────────────────────────
    //
    // In real-time audio applications (microphone, network, live stream), frames
    // arrive asynchronously from an external source. This pattern models that
    // scenario with a producer task that sends frames through a channel.
    //
    // The mpsc channel (capacity 32) between producer and consumer decouples the
    // two. The encoder's internal channel (capacity 8) provides a second level
    // of back-pressure, throttling the consumer when the FFmpeg worker is busy.
    //
    // Shutdown protocol: the producer drops its sender when exhausted, which
    // closes the channel and causes rx.recv() to return None.

    println!("=== Pattern 2: streaming from an async source ===");

    let (tx, mut rx) = tokio::sync::mpsc::channel(32);

    // Producer: simulate an async audio source (here: decode from a file).
    let producer_input = input.clone();
    let producer = tokio::spawn(async move {
        match AsyncAudioDecoder::open(producer_input).await {
            Ok(decoder) => {
                let stream = decoder.into_stream();
                tokio::pin!(stream);
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(frame) => {
                            if tx.send(frame).await.is_err() {
                                break; // Consumer dropped; stop producing.
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

    // Consumer: drain channel, push frames to async encoder.
    let mut encoder2 = match AsyncAudioEncoder::from_builder(
        AudioEncoder::create(&output)
            .audio(sample_rate, channels)
            .audio_codec(codec)
            .audio_bitrate(bitrate),
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
