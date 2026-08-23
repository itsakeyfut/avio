//! SwScale performance benchmarks.
//!
//! Benchmarks for image scaling and pixel format conversion operations.

use std::hint::black_box;
use std::os::raw::c_int;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ff_sys::swscale::{get_context, scale, scale_flags};
use ff_sys::{AVPixelFormat_AV_PIX_FMT_RGB24, AVPixelFormat_AV_PIX_FMT_RGBA};

/// Load test image from assets directory.
/// Returns (width, height, RGBA pixel data).
fn load_test_image() -> (u32, u32, Vec<u8>) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let image_path = format!("{}/../../assets/img/hello-triangle.png", manifest_dir);

    let img = image::open(&image_path).expect("Failed to load test image");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    (width, height, rgba.into_raw())
}

/// Benchmark context creation for different resolutions.
fn bench_context_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("swscale_context_creation");

    let resolutions = [
        ("720p", 1280, 720),
        ("1080p", 1920, 1080),
        ("4K", 3840, 2160),
    ];

    for (name, width, height) in resolutions {
        group.bench_function(BenchmarkId::new("create_context", name), |b| {
            b.iter(|| unsafe {
                let ctx = get_context(
                    black_box(width),
                    black_box(height),
                    AVPixelFormat_AV_PIX_FMT_RGBA,
                    black_box(width / 2),
                    black_box(height / 2),
                    AVPixelFormat_AV_PIX_FMT_RGBA,
                    scale_flags::BILINEAR,
                )
                .expect("Context creation should succeed");
                ff_sys::sws_freeContext(ctx);
            });
        });
    }

    group.finish();
}

/// Benchmark scaling with different algorithms.
fn bench_scaling_algorithms(c: &mut Criterion) {
    let (src_width, src_height, src_data) = load_test_image();

    // Downscale to 720p
    let dst_width: i32 = 1280;
    let dst_height: i32 = 720;

    let algorithms = [
        ("fast_bilinear", scale_flags::FAST_BILINEAR),
        ("bilinear", scale_flags::BILINEAR),
        ("bicubic", scale_flags::BICUBIC),
        ("point", scale_flags::POINT),
        ("area", scale_flags::AREA),
        ("lanczos", scale_flags::LANCZOS),
    ];

    let mut group = c.benchmark_group("swscale_algorithms");
    group.throughput(Throughput::Elements((dst_width * dst_height) as u64));

    for (name, algo) in algorithms {
        group.bench_function(BenchmarkId::new("scale", name), |b| unsafe {
            let ctx = get_context(
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                dst_width,
                dst_height,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                algo,
            )
            .expect("Context creation should succeed");

            let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
            let src_ptrs: [*const u8; 4] = [
                src_data.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ];

            let dst_size = (dst_width * dst_height * 4) as usize;
            let mut dst_data: Vec<u8> = vec![0u8; dst_size];
            let dst_stride: [c_int; 4] = [(dst_width * 4) as c_int, 0, 0, 0];
            let dst_ptrs: [*mut u8; 4] = [
                dst_data.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];

            b.iter(|| {
                scale(
                    ctx,
                    src_ptrs.as_ptr(),
                    src_stride.as_ptr(),
                    0,
                    black_box(src_height as c_int),
                    dst_ptrs.as_ptr(),
                    dst_stride.as_ptr(),
                )
                .expect("Scaling should succeed");
            });

            ff_sys::sws_freeContext(ctx);
        });
    }

    group.finish();
}

/// Benchmark scaling to different output resolutions.
fn bench_scaling_resolutions(c: &mut Criterion) {
    let (src_width, src_height, src_data) = load_test_image();

    let output_resolutions = [
        ("256x256", 256, 256),
        ("720p", 1280, 720),
        ("1080p", 1920, 1080),
    ];

    let mut group = c.benchmark_group("swscale_resolutions");

    for (name, dst_width, dst_height) in output_resolutions {
        group.throughput(Throughput::Elements((dst_width * dst_height) as u64));

        group.bench_function(BenchmarkId::new("scale_to", name), |b| unsafe {
            let ctx = get_context(
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                dst_width,
                dst_height,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                scale_flags::BILINEAR,
            )
            .expect("Context creation should succeed");

            let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
            let src_ptrs: [*const u8; 4] = [
                src_data.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ];

            let dst_size = (dst_width * dst_height * 4) as usize;
            let mut dst_data: Vec<u8> = vec![0u8; dst_size];
            let dst_stride: [c_int; 4] = [(dst_width * 4) as c_int, 0, 0, 0];
            let dst_ptrs: [*mut u8; 4] = [
                dst_data.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];

            b.iter(|| {
                scale(
                    ctx,
                    src_ptrs.as_ptr(),
                    src_stride.as_ptr(),
                    0,
                    black_box(src_height as c_int),
                    dst_ptrs.as_ptr(),
                    dst_stride.as_ptr(),
                )
                .expect("Scaling should succeed");
            });

            ff_sys::sws_freeContext(ctx);
        });
    }

    group.finish();
}

/// Benchmark pixel format conversion.
fn bench_format_conversion(c: &mut Criterion) {
    let (src_width, src_height, src_data) = load_test_image();

    let mut group = c.benchmark_group("swscale_format_conversion");
    group.throughput(Throughput::Elements((src_width * src_height) as u64));

    group.bench_function("rgba_to_rgb24", |b| {
        unsafe {
            let ctx = get_context(
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::POINT,
            )
            .expect("Context creation should succeed");

            let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
            let src_ptrs: [*const u8; 4] = [
                src_data.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ];

            // RGB24: 3 bytes per pixel
            let dst_size = (src_width * src_height * 3) as usize;
            let mut dst_data: Vec<u8> = vec![0u8; dst_size];
            let dst_stride: [c_int; 4] = [(src_width * 3) as c_int, 0, 0, 0];
            let dst_ptrs: [*mut u8; 4] = [
                dst_data.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];

            b.iter(|| {
                scale(
                    ctx,
                    src_ptrs.as_ptr(),
                    src_stride.as_ptr(),
                    0,
                    black_box(src_height as c_int),
                    dst_ptrs.as_ptr(),
                    dst_stride.as_ptr(),
                )
                .expect("Conversion should succeed");
            });

            ff_sys::sws_freeContext(ctx);
        }
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_context_creation,
    bench_scaling_algorithms,
    bench_scaling_resolutions,
    bench_format_conversion
);
criterion_main!(benches);
