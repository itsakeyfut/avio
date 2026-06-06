//! Memory usage tests for ff-decode.
//!
//! These tests validate that the ff-* implementation has efficient memory usage:
//! - Frames should be pooled and reused
//! - Memory should not grow unbounded during long decoding sessions
//! - Seeking should not cause memory leaks
//!
//! Note: Exact memory measurements are system-dependent, so we use relative measurements.

// Tests are allowed to use unwrap() for simplicity
#![allow(clippy::unwrap_used)]
// Platform memory queries require the Win32 process API.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use std::sync::Arc;

use ff_common::VecPool;
use ff_decode::{FramePool, HardwareAccel, SeekMode, VideoDecoder};

// ============================================================================
// Test Helpers
// ============================================================================

/// Returns the path to the test assets directory.
fn assets_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/../../assets", manifest_dir))
}

/// Returns the path to the test video file.
fn test_video_path() -> PathBuf {
    assets_dir().join("video/gameplay.mp4")
}

/// Creates a test decoder with hardware acceleration disabled for consistency.
fn create_decoder() -> VideoDecoder {
    VideoDecoder::open(&test_video_path())
        .hardware_accel(HardwareAccel::None)
        .build()
        .expect("Failed to create decoder")
}

/// Gets an estimate of current memory usage (platform-specific, best effort).
#[cfg(target_os = "windows")]
fn get_memory_usage_bytes() -> Option<usize> {
    use std::mem;
    use winapi::um::processthreadsapi::GetCurrentProcess;
    use winapi::um::psapi::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};

    unsafe {
        let process = GetCurrentProcess();
        let mut pmc: PROCESS_MEMORY_COUNTERS = mem::zeroed();
        pmc.cb = mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;

        if GetProcessMemoryInfo(process, &mut pmc, pmc.cb) != 0 {
            Some(pmc.WorkingSetSize)
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn get_memory_usage_bytes() -> Option<usize> {
    // On Unix systems, we can read from /proc/self/statm
    // This is a simplified version; production code might use a crate like `sysinfo`
    None
}

/// Helper to format bytes in a human-readable format.
fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * KB;
    const GB: usize = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

// ============================================================================
// Memory Usage Tests
// ============================================================================

#[test]
fn test_memory_stability_during_sequential_decode() {
    let mut decoder = create_decoder();

    // Get initial memory usage
    let initial_memory = get_memory_usage_bytes();

    // Decode 300 frames (10 seconds at 30fps)
    const FRAME_COUNT: usize = 300;
    let mut decoded_count = 0;

    for _ in 0..FRAME_COUNT {
        match decoder.decode_one() {
            Ok(Some(_frame)) => {
                // Frame is used here, then dropped
                decoded_count += 1;
            }
            Ok(None) => break,
            Err(e) => panic!("Decode failed: {}", e),
        }
    }

    // Get final memory usage
    let final_memory = get_memory_usage_bytes();

    println!("Decoded {} frames", decoded_count);

    if let (Some(initial), Some(final_mem)) = (initial_memory, final_memory) {
        let growth = if final_mem > initial {
            final_mem - initial
        } else {
            0
        };

        println!("Initial memory: {}", format_bytes(initial));
        println!("Final memory: {}", format_bytes(final_mem));
        println!("Memory growth: {}", format_bytes(growth));

        // Memory growth should be minimal (under 100MB for 300 frames)
        // This allows for some caching and normal allocations
        const MAX_GROWTH_MB: usize = 100;
        let max_growth_bytes = MAX_GROWTH_MB * 1024 * 1024;

        assert!(
            growth < max_growth_bytes,
            "Memory growth too high: {} (threshold: {})",
            format_bytes(growth),
            format_bytes(max_growth_bytes)
        );
    } else {
        println!("Memory measurement not available on this platform");
    }
}

#[test]
fn test_memory_stability_during_repeated_seeking() {
    let mut decoder = create_decoder();

    // Get initial memory usage
    let initial_memory = get_memory_usage_bytes();

    // Perform 100 seeks to different positions
    const SEEK_COUNT: usize = 100;
    let positions = [
        Duration::from_secs(1),
        Duration::from_secs(3),
        Duration::from_secs(5),
        Duration::from_secs(7),
        Duration::from_secs(2),
    ];

    for i in 0..SEEK_COUNT {
        let pos = positions[i % positions.len()];
        decoder.seek(pos, SeekMode::Keyframe).expect("Seek failed");
        let _ = decoder.decode_one().expect("Decode failed");
    }

    // Get final memory usage
    let final_memory = get_memory_usage_bytes();

    println!("Performed {} seeks", SEEK_COUNT);

    if let (Some(initial), Some(final_mem)) = (initial_memory, final_memory) {
        let growth = if final_mem > initial {
            final_mem - initial
        } else {
            0
        };

        println!("Initial memory: {}", format_bytes(initial));
        println!("Final memory: {}", format_bytes(final_mem));
        println!("Memory growth: {}", format_bytes(growth));

        // Memory growth should be minimal during seeking
        const MAX_GROWTH_MB: usize = 50;
        let max_growth_bytes = MAX_GROWTH_MB * 1024 * 1024;

        assert!(
            growth < max_growth_bytes,
            "Memory growth during seeking too high: {} (threshold: {})",
            format_bytes(growth),
            format_bytes(max_growth_bytes)
        );
    } else {
        println!("Memory measurement not available on this platform");
    }
}

#[test]
fn test_no_memory_leak_after_decoder_drop() {
    // Get initial memory usage
    let initial_memory = get_memory_usage_bytes();

    // Create and drop multiple decoders
    const DECODER_COUNT: usize = 10;
    for _ in 0..DECODER_COUNT {
        let mut decoder = create_decoder();

        // Decode a few frames
        for _ in 0..10 {
            if decoder.decode_one().is_err() {
                break;
            }
        }

        // Decoder is dropped here
    }

    // Force garbage collection (not guaranteed in Rust, but we can try)
    // In a real memory profiler, we would use tools like valgrind or heaptrack

    // Get final memory usage
    let final_memory = get_memory_usage_bytes();

    if let (Some(initial), Some(final_mem)) = (initial_memory, final_memory) {
        let growth = if final_mem > initial {
            final_mem - initial
        } else {
            0
        };

        println!("Created and dropped {} decoders", DECODER_COUNT);
        println!("Initial memory: {}", format_bytes(initial));
        println!("Final memory: {}", format_bytes(final_mem));
        println!("Memory growth: {}", format_bytes(growth));

        // Memory growth should be minimal after decoder cleanup
        // Allow 50MB for normal allocations and OS behavior
        const MAX_GROWTH_MB: usize = 50;
        let max_growth_bytes = MAX_GROWTH_MB * 1024 * 1024;

        assert!(
            growth < max_growth_bytes,
            "Possible memory leak detected: {} (threshold: {})",
            format_bytes(growth),
            format_bytes(max_growth_bytes)
        );
    } else {
        println!("Memory measurement not available on this platform");
    }
}

#[test]
fn test_frame_memory_is_released() {
    let mut decoder = create_decoder();

    // Get baseline memory
    let baseline_memory = get_memory_usage_bytes();

    // Decode frames in a scope, so they're dropped
    {
        let mut frames = Vec::new();
        for _ in 0..10 {
            if let Ok(Some(frame)) = decoder.decode_one() {
                frames.push(frame);
            }
        }

        // Get memory with frames in scope
        let with_frames_memory = get_memory_usage_bytes();

        if let (Some(baseline), Some(with_frames)) = (baseline_memory, with_frames_memory) {
            println!(
                "Memory with {} frames in scope: {}",
                frames.len(),
                format_bytes(with_frames)
            );
            println!(
                "Frame memory usage: {}",
                format_bytes(if with_frames > baseline {
                    with_frames - baseline
                } else {
                    0
                })
            );
        }

        // Frames are dropped here
    }

    // Decode a few more frames to ensure decoder still works
    for _ in 0..5 {
        let _ = decoder.decode_one();
    }

    // Get memory after frames dropped
    let after_drop_memory = get_memory_usage_bytes();

    if let (Some(baseline), Some(after_drop)) = (baseline_memory, after_drop_memory) {
        let growth = if after_drop > baseline {
            after_drop - baseline
        } else {
            0
        };

        println!("Baseline memory: {}", format_bytes(baseline));
        println!("Memory after frame drop: {}", format_bytes(after_drop));
        println!("Net growth: {}", format_bytes(growth));

        // Memory should return close to baseline (allow 60MB variance for FFmpeg internal caching)
        const MAX_GROWTH_MB: usize = 60;
        let max_growth_bytes = MAX_GROWTH_MB * 1024 * 1024;

        assert!(
            growth < max_growth_bytes,
            "Frame memory not properly released: {} (threshold: {})",
            format_bytes(growth),
            format_bytes(max_growth_bytes)
        );
    } else {
        println!("Memory measurement not available on this platform");
    }
}

#[test]
fn test_thumbnail_memory_efficiency() {
    let mut decoder = create_decoder();

    // Get baseline memory
    let baseline_memory = get_memory_usage_bytes();

    // Generate 20 thumbnails
    const THUMBNAIL_COUNT: usize = 20;
    let thumbnails = decoder
        .thumbnails(THUMBNAIL_COUNT, 160, 90)
        .expect("Failed to generate thumbnails");

    assert_eq!(thumbnails.len(), THUMBNAIL_COUNT);

    // Get memory with thumbnails
    let with_thumbnails_memory = get_memory_usage_bytes();

    if let (Some(baseline), Some(with_thumbs)) = (baseline_memory, with_thumbnails_memory) {
        let thumbnail_memory = if with_thumbs > baseline {
            with_thumbs - baseline
        } else {
            0
        };

        // Each 160x90 YUV420p thumbnail is approximately:
        // Y: 160*90 = 14,400 bytes
        // U: 80*45 = 3,600 bytes
        // V: 80*45 = 3,600 bytes
        // Total per frame: ~22KB
        // 20 frames: ~440KB

        println!("Thumbnail memory usage: {}", format_bytes(thumbnail_memory));

        // Allow up to 30MB for 20 thumbnails (includes FFmpeg codec/format context overhead)
        const MAX_THUMBNAIL_MEMORY_MB: usize = 30;
        let max_memory_bytes = MAX_THUMBNAIL_MEMORY_MB * 1024 * 1024;

        assert!(
            thumbnail_memory < max_memory_bytes,
            "Thumbnail memory usage too high: {} (threshold: {})",
            format_bytes(thumbnail_memory),
            format_bytes(max_memory_bytes)
        );
    } else {
        println!("Memory measurement not available on this platform");
    }

    // Drop thumbnails
    drop(thumbnails);
}

// ============================================================================
// Memory Efficiency Comparison Tests
// ============================================================================

#[test]
#[ignore = "performance thresholds are environment-dependent; run explicitly with -- --include-ignored"]
fn test_decoder_memory_overhead() {
    // Measure the base memory overhead of creating a decoder
    let baseline_memory = get_memory_usage_bytes();

    let decoder = create_decoder();

    let with_decoder_memory = get_memory_usage_bytes();

    if let (Some(baseline), Some(with_decoder)) = (baseline_memory, with_decoder_memory) {
        let overhead = if with_decoder > baseline {
            with_decoder - baseline
        } else {
            0
        };

        println!("Decoder memory overhead: {}", format_bytes(overhead));

        // Decoder should have minimal overhead (under 50MB)
        const MAX_OVERHEAD_MB: usize = 50;
        let max_overhead_bytes = MAX_OVERHEAD_MB * 1024 * 1024;

        assert!(
            overhead < max_overhead_bytes,
            "Decoder overhead too high: {} (threshold: {})",
            format_bytes(overhead),
            format_bytes(max_overhead_bytes)
        );
    } else {
        println!("Memory measurement not available on this platform");
    }

    drop(decoder);
}

// ============================================================================
// Frame Pool Tests
// ============================================================================

#[test]
fn frame_pool_should_accumulate_buffers_after_decode() {
    let pool = VecPool::new(8);
    let pool_dyn: Arc<dyn FramePool> = Arc::clone(&pool) as Arc<dyn FramePool>;

    let mut decoder = VideoDecoder::open(&test_video_path())
        .hardware_accel(HardwareAccel::None)
        .frame_pool(pool_dyn)
        .build()
        .expect("Failed to create decoder");

    // Decode and immediately drop 5 frames. Each drop should return the
    // buffer to the pool, growing pool.available().
    for _ in 0..5 {
        match decoder.decode_one() {
            Ok(Some(frame)) => drop(frame),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    assert!(
        pool.available() > 0,
        "pool.available() should be > 0 after dropping decoded frames, got {}",
        pool.available()
    );
}

#[test]
fn frame_pool_available_should_be_zero_while_frames_are_held() {
    // Case C: decode multiple frames simultaneously, verify pool is empty
    // while they're held, then verify it fills after they're dropped.
    let pool = VecPool::new(8);
    let pool_dyn: Arc<dyn FramePool> = Arc::clone(&pool) as Arc<dyn FramePool>;

    let mut decoder = VideoDecoder::open(&test_video_path())
        .hardware_accel(HardwareAccel::None)
        .frame_pool(pool_dyn)
        .build()
        .expect("Failed to create decoder");

    // Decode 4 frames and hold them all.
    let mut held_frames = Vec::new();
    for _ in 0..4 {
        match decoder.decode_one() {
            Ok(Some(frame)) => held_frames.push(frame),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    assert_eq!(held_frames.len(), 4, "Should have decoded 4 frames");

    // All buffers are in use — pool must be empty.
    assert_eq!(
        pool.available(),
        0,
        "pool.available() should be 0 while frames are held, got {}",
        pool.available()
    );

    // Drop all frames — buffers should return to the pool.
    drop(held_frames);

    assert!(
        pool.available() > 0,
        "pool.available() should be > 0 after dropping all held frames, got {}",
        pool.available()
    );
}
