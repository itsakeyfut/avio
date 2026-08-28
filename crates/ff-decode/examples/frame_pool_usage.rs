//! Decode video with an explicit frame pool to bound memory allocation.
//!
//! Demonstrates:
//! - `VecPool::new(capacity)` — create a pool that retains up to N buffers
//! - `VecPool::capacity()` — the configured pool size
//! - `VecPool::available()` — buffers currently held in the pool
//! - `FramePool` — the shared trait implemented by all pool types
//! - `VideoDecoderBuilder::frame_pool()` — attach the pool to the decoder
//!
//! Without a pool, each decoded frame allocates fresh memory.
//! With a pool, the decoder reuses buffers that have been dropped by the caller,
//! capping peak heap usage to roughly `capacity × frame_size`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example frame_pool_usage -- \
//!   --input      input.mp4  \
//!   [--pool-size 8]         # number of frame buffers to retain (default: 8)
//! ```

use std::{path::Path, process, sync::Arc};

use ff_common::VecPool;
use ff_decode::{FramePool, VideoDecoder};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut pool_size: usize = 8;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--pool-size" => {
                let v = args.next().unwrap_or_default();
                pool_size = v.parse().unwrap_or(8);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: frame_pool_usage --input <file> [--pool-size N]");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    // ── VecPool ───────────────────────────────────────────────────────────────
    //
    // VecPool::new() returns Arc<VecPool>.
    // capacity() is the maximum number of buffers retained between frames.
    // available() counts buffers currently sitting in the pool ready for reuse.

    let pool: Arc<VecPool> = VecPool::new(pool_size);
    println!(
        "Pool created: capacity={}  available={}",
        pool.capacity(),
        pool.available()
    );

    // ── FramePool (trait) ─────────────────────────────────────────────────────
    //
    // FramePool is the trait that abstracts over pool implementations.
    // The decoder accepts Arc<dyn FramePool>.

    let pool_as_trait: Arc<dyn FramePool> = pool.clone();

    // ── Attach pool to decoder ────────────────────────────────────────────────
    //
    // frame_pool() passes the pool to the decoder. On each decoded frame, the
    // decoder attempts to claim a buffer from the pool instead of allocating.
    // When the frame is dropped, the buffer returns to the pool automatically.

    let mut decoder = match VideoDecoder::open(&input).frame_pool(pool_as_trait).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    println!(
        "Input:   {in_name}  {}×{}  codec={}",
        decoder.width(),
        decoder.height(),
        decoder.stream_info().codec_name()
    );
    println!("Decoding (pool-size={pool_size})...");
    println!();

    let mut count: u64 = 0;
    let report_interval = 30;

    loop {
        match decoder.decode_one() {
            Ok(Some(frame)) => {
                count += 1;

                // Periodically print pool stats to show buffer reuse.
                if count.is_multiple_of(report_interval) {
                    println!("  frame={count:>5}  pool.available()={}", pool.available());
                }

                // Dropping `frame` here returns its buffer to the pool.
                // With pool_size=8, at most 8 buffers are retained;
                // older ones are freed if the pool is full.
                drop(frame);
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        }
    }

    println!();
    println!(
        "Done. {count} frames decoded.  pool.capacity()={}  pool.available()={}",
        pool.capacity(),
        pool.available()
    );
}
