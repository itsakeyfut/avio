//! Criterion benchmarks for ff-preview.
//!
//! Measures the percentage of video frames delivered on time during a
//! 1080p/30 fps playback loop. "On time" means the wall-clock delta between
//! consecutive [`FrameSink::push_frame`] calls is within ±1 frame period
//! (33 ms) of the PTS delta.
//!
//! # Target
//!
//! ≥99% of frame pairs on time at 1080p/30 fps. This target is documented
//! here as a comment — **not** an assertion — because timing is
//! environment-sensitive and must not fail CI.
//!
//! # Reference asset
//!
//! `assets/test/preview_bench_1080p.mp4` — a 60-second, 1920×1080, 30 fps
//! H.264/AAC file. The benchmark skips gracefully if the file is absent.
//!
//! Run with:
//! ```bash
//! cargo bench -p ff-preview
//! cargo bench -p ff-preview --features tokio
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use ff_preview::{FrameSink, PreviewPlayer};

// Asset path

fn bench_video_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/preview_bench_1080p.mp4")
}

// TimingSink

/// [`FrameSink`] that records `(wall_time, pts)` for each delivered frame.
///
/// Share `buf` with the outer benchmark scope to read timings after playback.
struct TimingSink {
    buf: Arc<Mutex<Vec<(Instant, Duration)>>>,
}

impl FrameSink for TimingSink {
    fn push_frame(&mut self, _rgba: &[u8], _width: u32, _height: u32, pts: Duration) {
        self.buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((Instant::now(), pts));
    }
}

// On-time metric

/// Returns `(on_time_count, total_pairs)`.
///
/// A consecutive frame pair is "on time" when
/// `|wall_delta − pts_delta| ≤ frame_period`.
///
/// Target: ≥99% of pairs on time at 1080p/30 fps.
fn count_on_time(timings: &[(Instant, Duration)], frame_period: Duration) -> (usize, usize) {
    let mut on_time = 0_usize;
    let mut total = 0_usize;
    for pair in timings.windows(2) {
        let wall = pair[1].0.duration_since(pair[0].0);
        let pts = pair[1].1.saturating_sub(pair[0].1);
        total += 1;
        let diff = if wall > pts { wall - pts } else { pts - wall };
        if diff <= frame_period {
            on_time += 1;
        }
    }
    (on_time, total)
}

// Sync benchmark

fn bench_1080p_sync_playback(c: &mut Criterion) {
    let path = bench_video_path();
    if !path.exists() {
        println!("skipping benchmark: reference file not found at {path:?}");
        return;
    }

    let mut group = c.benchmark_group("preview");

    group.bench_function("1080p_sync_playback_on_time_pct", |b| {
        b.iter_batched(
            || {
                let buf = Arc::new(Mutex::new(Vec::<(Instant, Duration)>::new()));
                let (runner, handle) = PreviewPlayer::open(&path)
                    .expect("failed to open player")
                    .split();
                (runner, handle, buf)
            },
            |(mut runner, handle, buf)| {
                runner.set_sink(Box::new(TimingSink {
                    buf: Arc::clone(&buf),
                }));
                handle.play();
                let _ = runner.run();

                // 30 fps → ~33 ms per frame
                let frame_period = Duration::from_secs_f64(1.0 / 30.0);
                let timings = buf
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (on_time, total) = count_on_time(&timings, frame_period);
                if total > 0 {
                    let pct = on_time as f64 / total as f64 * 100.0;
                    eprintln!("on-time frames: {on_time}/{total} ({pct:.1}%) [target ≥99%]");
                }
            },
            criterion::BatchSize::LargeInput,
        );
    });

    group.finish();
}

// Async benchmark (tokio feature)

#[cfg(feature = "tokio")]
fn bench_1080p_async_playback(c: &mut Criterion) {
    let path = bench_video_path();
    if !path.exists() {
        println!("skipping benchmark: reference file not found at {path:?}");
        return;
    }

    let rt = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(rt) => rt,
        Err(e) => {
            println!("skipping: could not build tokio runtime: {e}");
            return;
        }
    };

    let mut group = c.benchmark_group("preview");

    // Run the blocking playback loop on a spawn_blocking thread — the
    // idiomatic async pattern for long-running blocking work.
    group.bench_function("1080p_async_playback_on_time_pct", |b| {
        b.iter_batched(
            || {
                let buf = Arc::new(Mutex::new(Vec::<(Instant, Duration)>::new()));
                (Arc::clone(&buf), buf)
            },
            |(buf_inner, buf_outer)| {
                let p = path.clone();
                rt.block_on(async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        let (mut runner, handle) = match PreviewPlayer::open(&p) {
                            Ok(player) => player.split(),
                            Err(e) => {
                                println!("skip: {e}");
                                return;
                            }
                        };
                        runner.set_sink(Box::new(TimingSink { buf: buf_inner }));
                        handle.play();
                        let _ = runner.run();
                    })
                    .await;
                });

                let frame_period = Duration::from_secs_f64(1.0 / 30.0);
                let timings = buf_outer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (on_time, total) = count_on_time(&timings, frame_period);
                if total > 0 {
                    let pct = on_time as f64 / total as f64 * 100.0;
                    eprintln!("async on-time frames: {on_time}/{total} ({pct:.1}%) [target ≥99%]");
                }
            },
            criterion::BatchSize::LargeInput,
        );
    });

    group.finish();
}

// criterion_group / criterion_main

#[cfg(not(feature = "tokio"))]
criterion_group!(preview_benches, bench_1080p_sync_playback);

#[cfg(feature = "tokio")]
criterion_group!(
    preview_benches,
    bench_1080p_sync_playback,
    bench_1080p_async_playback
);

criterion_main!(preview_benches);
