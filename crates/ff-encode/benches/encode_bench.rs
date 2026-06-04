//! Benchmark tests for ff-encode.
//!
//! These benchmarks measure the performance of video/audio encoding operations.
//!
//! Run with: `cargo bench -p ff-encode`

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use ff_encode::{Preset, VideoCodec, VideoEncoder};
use ff_format::{PixelFormat, VideoFrame};

// ============================================================================
// Test Helpers
// ============================================================================

/// Returns the path to the test output directory.
fn bench_output_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/target/bench_output", manifest_dir))
}

/// Creates a test output path.
fn bench_output_path(filename: &str) -> PathBuf {
    let dir = bench_output_dir();
    std::fs::create_dir_all(&dir).ok();
    dir.join(filename)
}

/// Creates a black test frame with the specified dimensions.
fn create_black_frame(width: u32, height: u32) -> VideoFrame {
    VideoFrame::empty(width, height, PixelFormat::Yuv420p).expect("Failed to create frame")
}

// ============================================================================
// Encode Benchmarks
// ============================================================================

/// Benchmark encoding a single frame.
fn bench_encode_single_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_single_frame");

    group.bench_function("640x480_mpeg4", |b| {
        b.iter_batched(
            || {
                let output_path = bench_output_path("bench_single_frame.mp4");
                let encoder = VideoEncoder::create(&output_path)
                    .video(640, 480, 30.0)
                    .video_codec(VideoCodec::Mpeg4)
                    .preset(Preset::Ultrafast)
                    .build()
                    .expect("Failed to build encoder");
                (encoder, create_black_frame(640, 480))
            },
            |(mut encoder, frame)| {
                encoder
                    .push_video(black_box(&frame))
                    .expect("Failed to push frame");
                encoder.finish().expect("Failed to finish encoding");
            },
            criterion::BatchSize::LargeInput,
        );
    });

    group.finish();
}

/// Benchmark encoding multiple frames.
fn bench_encode_multiple_frames(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_multiple_frames");

    for frame_count in [30, 60, 150].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(frame_count),
            frame_count,
            |b, &frame_count| {
                b.iter_batched(
                    || {
                        let output_path =
                            bench_output_path(&format!("bench_{}_frames.mp4", frame_count));
                        let encoder = VideoEncoder::create(&output_path)
                            .video(640, 480, 30.0)
                            .video_codec(VideoCodec::Mpeg4)
                            .preset(Preset::Ultrafast)
                            .build()
                            .expect("Failed to build encoder");
                        (encoder, create_black_frame(640, 480))
                    },
                    |(mut encoder, frame)| {
                        for _ in 0..frame_count {
                            encoder
                                .push_video(black_box(&frame))
                                .expect("Failed to push frame");
                        }
                        encoder.finish().expect("Failed to finish encoding");
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark encoding at different resolutions.
fn bench_encode_resolutions(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_resolutions");

    let resolutions = [
        (320, 240, "320x240"),
        (640, 480, "640x480"),
        (1280, 720, "720p"),
        (1920, 1080, "1080p"),
    ];

    for (width, height, name) in resolutions.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &(*width, *height),
            |b, dims: &(u32, u32)| {
                let (width, height) = *dims;
                b.iter_batched(
                    || {
                        let output_path = bench_output_path(&format!("bench_{}.mp4", name));
                        let encoder = VideoEncoder::create(&output_path)
                            .video(width, height, 30.0)
                            .video_codec(VideoCodec::Mpeg4)
                            .preset(Preset::Ultrafast)
                            .build()
                            .expect("Failed to build encoder");
                        (encoder, create_black_frame(width, height))
                    },
                    |(mut encoder, frame)| {
                        // Encode 30 frames (1 second at 30fps)
                        for _ in 0..30 {
                            encoder
                                .push_video(black_box(&frame))
                                .expect("Failed to push frame");
                        }
                        encoder.finish().expect("Failed to finish encoding");
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark encoding with different presets.
fn bench_encode_presets(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_presets");

    let presets = [
        (Preset::Ultrafast, "ultrafast"),
        (Preset::Faster, "faster"),
        (Preset::Fast, "fast"),
        (Preset::Medium, "medium"),
    ];

    for (preset, name) in presets.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(name), preset, |b, &preset| {
            b.iter_batched(
                || {
                    let output_path = bench_output_path(&format!("bench_{}.mp4", name));
                    let encoder = VideoEncoder::create(&output_path)
                        .video(640, 480, 30.0)
                        .video_codec(VideoCodec::Mpeg4)
                        .preset(preset)
                        .build()
                        .expect("Failed to build encoder");
                    (encoder, create_black_frame(640, 480))
                },
                |(mut encoder, frame)| {
                    // Encode 30 frames
                    for _ in 0..30 {
                        encoder
                            .push_video(black_box(&frame))
                            .expect("Failed to push frame");
                    }
                    encoder.finish().expect("Failed to finish encoding");
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

/// Benchmark encoder creation overhead.
fn bench_encoder_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("encoder_creation");

    group.bench_function("create_640x480", |b| {
        b.iter(|| {
            let output_path = bench_output_path("bench_creation.mp4");
            let encoder = VideoEncoder::create(black_box(&output_path))
                .video(640, 480, 30.0)
                .video_codec(VideoCodec::Mpeg4)
                .preset(Preset::Ultrafast)
                .build()
                .expect("Failed to build encoder");
            black_box(encoder)
        });
    });

    group.finish();
}

/// Benchmark complete encode workflow.
fn bench_complete_workflow(c: &mut Criterion) {
    let mut group = c.benchmark_group("workflow");

    group.bench_function("create_encode_30_frames_finish", |b| {
        b.iter(|| {
            let output_path = bench_output_path("bench_workflow.mp4");
            let mut encoder = VideoEncoder::create(black_box(&output_path))
                .video(640, 480, 30.0)
                .video_codec(VideoCodec::Mpeg4)
                .preset(Preset::Ultrafast)
                .build()
                .expect("Failed to build encoder");

            let frame = create_black_frame(640, 480);

            // Encode 30 frames
            for _ in 0..30 {
                encoder
                    .push_video(black_box(&frame))
                    .expect("Failed to push frame");
            }

            encoder.finish().expect("Failed to finish encoding");
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_encode_single_frame,
    bench_encode_multiple_frames,
    bench_encode_resolutions,
    bench_encode_presets,
    bench_encoder_creation,
    bench_complete_workflow,
);

criterion_main!(benches);
