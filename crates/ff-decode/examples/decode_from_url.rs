//! Decode video frames from an HTTP, HLS, RTMP, or SRT URL.
//!
//! Demonstrates:
//! - `VideoDecoder::open(url)` — open a network source
//! - `.network(NetworkOptions)` — configure connection and read timeouts
//! - `.is_live()` — detect live vs. VOD streams
//! - `decode_one()` — pull frames from a remote source
//!
//! The example opens the URL, prints stream metadata, and decodes up to
//! `--max-frames` video frames (default: 30), reporting wall-clock throughput.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example decode_from_url --features decode -- \
//!   --url   http://example.com/video.mp4   \
//!   [--max-frames 30]                      \
//!   [--connect-timeout 10]                 \
//!   [--read-timeout 30]
//! ```

use std::{process, time::Duration};

use ff_decode::VideoDecoder;
use ff_format::NetworkOptions;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut url = None::<String>;
    let mut max_frames: u64 = 30;
    let mut connect_timeout_secs: u64 = 10;
    let mut read_timeout_secs: u64 = 30;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--url" | "-u" => url = Some(args.next().unwrap_or_default()),
            "--max-frames" | "-n" => {
                let v = args.next().unwrap_or_default();
                max_frames = v.parse().unwrap_or(30);
            }
            "--connect-timeout" => {
                let v = args.next().unwrap_or_default();
                connect_timeout_secs = v.parse().unwrap_or(10);
            }
            "--read-timeout" => {
                let v = args.next().unwrap_or_default();
                read_timeout_secs = v.parse().unwrap_or(30);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let url = url.unwrap_or_else(|| {
        eprintln!(
            "Usage: decode_from_url --url <URL> \
             [--max-frames N] [--connect-timeout N] [--read-timeout N]"
        );
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  --url http://example.com/video.mp4");
        eprintln!("  --url rtmp://ingest.example.com/live/key");
        eprintln!("  --url srt://source.example.com:9000");
        process::exit(1);
    });

    let network = NetworkOptions {
        connect_timeout: Duration::from_secs(connect_timeout_secs),
        read_timeout: Duration::from_secs(read_timeout_secs),
        reconnect_on_error: false,
        max_reconnect_attempts: 0,
    };

    println!("URL:             {url}");
    println!("Connect timeout: {connect_timeout_secs} s");
    println!("Read timeout:    {read_timeout_secs} s");
    println!("Max frames:      {max_frames}");
    println!();
    println!("Opening...");

    let mut decoder = match VideoDecoder::open(&url).network(network).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    println!(
        "Stream:  {}×{}  {:.2} fps",
        decoder.width(),
        decoder.height(),
        decoder.frame_rate()
    );
    println!("Live:    {}", decoder.is_live());
    println!();
    println!("Decoding up to {max_frames} frames...");
    println!();

    let start = std::time::Instant::now();
    let mut decoded: u64 = 0;

    loop {
        if decoded >= max_frames {
            break;
        }

        match decoder.decode_one() {
            Ok(Some(frame)) => {
                decoded += 1;
                let pts = frame.timestamp().as_millis();
                if decoded <= 5 || decoded.is_multiple_of(10) {
                    println!("  frame {decoded:>4}  pts={pts} ms");
                }
            }
            Ok(None) => {
                println!("End of stream after {decoded} frames.");
                break;
            }
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    #[allow(clippy::cast_precision_loss)]
    let fps = decoded as f64 / elapsed.max(0.001);
    println!();
    println!("Decoded {decoded} frames in {elapsed:.2} s  ({fps:.1} fps)");
}
