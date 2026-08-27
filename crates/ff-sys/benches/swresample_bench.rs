//! SwResample performance benchmarks.
//!
//! Benchmarks for audio resampling and format conversion operations.

use std::hint::black_box;
use std::os::raw::c_int;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ff_sys::swresample::{channel_layout, estimate_output_samples, sample_format};
use ff_sys::{AVChannelLayout, AVSampleFormat, SwrContext};

// The hand-written `swresample::alloc_set_opts2` / `init` / `convert` wrappers are
// sealed (`pub(crate)`), so this raw-FFI microbenchmark calls the bindgen entry
// points directly through these thin shims, keeping the benchmark bodies unchanged.

/// # Safety
/// The channel-layout pointers must be valid for the duration of the call.
unsafe fn alloc_set_opts2(
    out_ch_layout: *const AVChannelLayout,
    out_sample_fmt: AVSampleFormat,
    out_sample_rate: c_int,
    in_ch_layout: *const AVChannelLayout,
    in_sample_fmt: AVSampleFormat,
    in_sample_rate: c_int,
) -> Result<*mut SwrContext, c_int> {
    let mut ctx: *mut SwrContext = std::ptr::null_mut();
    // SAFETY: `&mut ctx` is a valid out-pointer; the layout pointers are upheld
    //         by the caller; null log_ctx and 0 log_offset are accepted.
    let ret = unsafe {
        ff_sys::swr_alloc_set_opts2(
            &mut ctx,
            out_ch_layout,
            out_sample_fmt,
            out_sample_rate,
            in_ch_layout,
            in_sample_fmt,
            in_sample_rate,
            0,
            std::ptr::null_mut(),
        )
    };
    if ret < 0 {
        Err(ret)
    } else if ctx.is_null() {
        Err(-1)
    } else {
        Ok(ctx)
    }
}

/// # Safety
/// `ctx` must be an allocated, configured `SwrContext`.
unsafe fn init(ctx: *mut SwrContext) -> Result<(), c_int> {
    // SAFETY: the caller upholds `ctx`.
    let ret = unsafe { ff_sys::swr_init(ctx) };
    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// # Safety
/// `ctx` and the sample buffers must be valid for the configured formats.
unsafe fn convert(
    ctx: *mut SwrContext,
    out: *mut *mut u8,
    out_count: c_int,
    in_: *const *const u8,
    in_count: c_int,
) -> Result<c_int, c_int> {
    // SAFETY: the caller upholds the context and sample buffers.
    let ret = unsafe { ff_sys::swr_convert(ctx, out, out_count, in_, in_count) };
    if ret < 0 { Err(ret) } else { Ok(ret) }
}

/// Benchmark context creation for different configurations.
fn bench_context_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("swresample_context_creation");

    let configs = [
        ("same_rate", 48000, 48000),
        ("44.1k_to_48k", 44100, 48000),
        ("48k_to_44.1k", 48000, 44100),
        ("96k_to_48k", 96000, 48000),
    ];

    for (name, in_rate, out_rate) in configs {
        group.bench_function(BenchmarkId::new("create_context", name), |b| {
            b.iter(|| unsafe {
                let out_layout = channel_layout::stereo();
                let in_layout = channel_layout::stereo();

                let ctx = alloc_set_opts2(
                    &out_layout,
                    sample_format::FLTP,
                    black_box(out_rate),
                    &in_layout,
                    sample_format::S16,
                    black_box(in_rate),
                )
                .expect("Context creation should succeed");

                init(ctx).expect("Init should succeed");
                ff_sys::swr_free(&mut (ctx as *mut _));
            });
        });
    }

    group.finish();
}

/// Benchmark resampling with different sample rates using synthetic data.
fn bench_resampling_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("swresample_rates");

    // Synthetic test data: S16 stereo audio
    let num_samples = 4096;
    let in_data: Vec<i16> = (0..num_samples * 2)
        .map(|i| ((i as f32 / 100.0).sin() * 10000.0) as i16)
        .collect();

    let rate_configs = [
        ("44.1k_to_48k", 44100, 48000),
        ("48k_to_44.1k", 48000, 44100),
        ("48k_to_96k", 48000, 96000),
        ("96k_to_48k", 96000, 48000),
        ("44.1k_to_96k", 44100, 96000),
    ];

    for (name, in_rate, out_rate) in rate_configs {
        group.throughput(Throughput::Elements(num_samples as u64));

        group.bench_function(BenchmarkId::new("resample", name), |b| {
            b.iter(|| unsafe {
                let out_layout = channel_layout::stereo();
                let in_layout = channel_layout::stereo();

                let mut ctx = alloc_set_opts2(
                    &out_layout,
                    sample_format::FLTP,
                    out_rate,
                    &in_layout,
                    sample_format::S16,
                    in_rate,
                )
                .expect("Context creation failed");

                init(ctx).expect("Init failed");

                let out_count = estimate_output_samples(out_rate, in_rate, num_samples as i32);
                let mut out_left: Vec<f32> = vec![0.0; out_count as usize];
                let mut out_right: Vec<f32> = vec![0.0; out_count as usize];
                let mut out_ptrs: [*mut u8; 2] =
                    [out_left.as_mut_ptr().cast(), out_right.as_mut_ptr().cast()];

                let in_ptr = in_data.as_ptr() as *const u8;
                let in_ptrs: [*const u8; 1] = [in_ptr];

                let _ = convert(
                    ctx,
                    out_ptrs.as_mut_ptr(),
                    black_box(out_count),
                    in_ptrs.as_ptr(),
                    black_box(num_samples as c_int),
                );

                ff_sys::swr_free(&mut ctx);
            });
        });
    }

    group.finish();
}

/// Benchmark format conversion (sample format changes).
fn bench_format_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("swresample_format_conversion");

    let num_samples = 4096;
    let sample_rate = 48000;

    let conversions = [
        ("s16_to_flt", sample_format::S16, sample_format::FLT),
        ("s16_to_fltp", sample_format::S16, sample_format::FLTP),
        ("flt_to_s16", sample_format::FLT, sample_format::S16),
        ("s32_to_flt", sample_format::S32, sample_format::FLT),
    ];

    group.throughput(Throughput::Elements(num_samples as u64));

    for (name, in_fmt, out_fmt) in conversions {
        // Create input data based on format
        let in_bytes = sample_format::bytes_per_sample(in_fmt) as usize;
        let in_data: Vec<u8> = vec![128; num_samples * 2 * in_bytes]; // stereo

        let out_bytes = sample_format::bytes_per_sample(out_fmt) as usize;
        let is_planar = sample_format::is_planar(out_fmt);

        group.bench_function(BenchmarkId::new("convert", name), |b| {
            b.iter(|| unsafe {
                let out_layout = channel_layout::stereo();
                let in_layout = channel_layout::stereo();

                let mut ctx = alloc_set_opts2(
                    &out_layout,
                    out_fmt,
                    sample_rate,
                    &in_layout,
                    in_fmt,
                    sample_rate,
                )
                .expect("Context creation failed");

                init(ctx).expect("Init failed");

                let out_count = num_samples as c_int + 256;

                if is_planar {
                    let mut out_left: Vec<u8> = vec![0; out_count as usize * out_bytes];
                    let mut out_right: Vec<u8> = vec![0; out_count as usize * out_bytes];
                    let mut out_ptrs: [*mut u8; 2] =
                        [out_left.as_mut_ptr(), out_right.as_mut_ptr()];

                    let in_ptr = in_data.as_ptr();
                    let in_ptrs: [*const u8; 1] = [in_ptr];

                    let _ = convert(
                        ctx,
                        out_ptrs.as_mut_ptr(),
                        black_box(out_count),
                        in_ptrs.as_ptr(),
                        black_box(num_samples as c_int),
                    );
                } else {
                    let mut out_data: Vec<u8> = vec![0; out_count as usize * 2 * out_bytes];
                    let out_ptr = out_data.as_mut_ptr();
                    let mut out_ptrs: [*mut u8; 1] = [out_ptr];

                    let in_ptr = in_data.as_ptr();
                    let in_ptrs: [*const u8; 1] = [in_ptr];

                    let _ = convert(
                        ctx,
                        out_ptrs.as_mut_ptr(),
                        black_box(out_count),
                        in_ptrs.as_ptr(),
                        black_box(num_samples as c_int),
                    );
                }

                ff_sys::swr_free(&mut ctx);
            });
        });
    }

    group.finish();
}

/// Benchmark channel layout conversion.
fn bench_channel_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("swresample_channel_conversion");

    let num_samples = 4096;
    let sample_rate = 48000;

    let conversions = [
        ("mono_to_stereo", 1, 2),
        ("stereo_to_mono", 2, 1),
        ("stereo_to_5.1", 2, 6),
        ("5.1_to_stereo", 6, 2),
    ];

    group.throughput(Throughput::Elements(num_samples as u64));

    for (name, in_channels, out_channels) in conversions {
        // Create input data (planar float)
        let in_data: Vec<f32> = vec![0.5; num_samples * in_channels];

        group.bench_function(BenchmarkId::new("convert", name), |b| {
            b.iter(|| unsafe {
                let out_layout = channel_layout::with_channels(out_channels as i32);
                let in_layout = channel_layout::with_channels(in_channels as i32);

                let mut ctx = alloc_set_opts2(
                    &out_layout,
                    sample_format::FLTP,
                    sample_rate,
                    &in_layout,
                    sample_format::FLTP,
                    sample_rate,
                )
                .expect("Context creation failed");

                init(ctx).expect("Init failed");

                let out_count = num_samples as c_int + 256;

                // Allocate output planes
                let mut out_planes: Vec<Vec<f32>> = (0..out_channels)
                    .map(|_| vec![0.0; out_count as usize])
                    .collect();
                let mut out_ptrs: Vec<*mut u8> = out_planes
                    .iter_mut()
                    .map(|v| v.as_mut_ptr().cast())
                    .collect();

                // Input planes
                let in_planes: Vec<*const u8> = (0..in_channels)
                    .map(|ch| {
                        in_data[ch * num_samples..(ch + 1) * num_samples]
                            .as_ptr()
                            .cast()
                    })
                    .collect();

                let _ = convert(
                    ctx,
                    out_ptrs.as_mut_ptr(),
                    black_box(out_count),
                    in_planes.as_ptr(),
                    black_box(num_samples as c_int),
                );

                ff_sys::swr_free(&mut ctx);
            });
        });
    }

    group.finish();
}

/// Benchmark chunk sizes for streaming conversion using synthetic data.
fn bench_chunk_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("swresample_chunk_sizes");

    let total_samples = 32768;
    let in_rate = 44100;
    let out_rate = 48000;

    // Generate synthetic S16 stereo data
    let in_data: Vec<i16> = (0..total_samples * 2)
        .map(|i| ((i as f32 / 100.0).sin() * 10000.0) as i16)
        .collect();

    let chunk_sizes = [256, 512, 1024, 2048, 4096];

    for chunk_size in chunk_sizes {
        group.throughput(Throughput::Elements(total_samples as u64));

        group.bench_function(BenchmarkId::new("chunk", chunk_size), |b| {
            b.iter(|| unsafe {
                let out_layout = channel_layout::stereo();
                let in_layout = channel_layout::stereo();

                let mut ctx = alloc_set_opts2(
                    &out_layout,
                    sample_format::FLTP,
                    out_rate,
                    &in_layout,
                    sample_format::S16,
                    in_rate,
                )
                .expect("Context creation failed");

                init(ctx).expect("Init failed");

                let mut offset = 0;
                let bytes_per_frame = 4; // S16 stereo = 2 bytes * 2 channels

                while offset < total_samples {
                    let samples_to_process = std::cmp::min(chunk_size, total_samples - offset);
                    let byte_offset = offset * bytes_per_frame / 2; // /2 because i16

                    let out_count =
                        estimate_output_samples(out_rate, in_rate, samples_to_process as i32);
                    let mut out_left: Vec<f32> = vec![0.0; out_count as usize];
                    let mut out_right: Vec<f32> = vec![0.0; out_count as usize];
                    let mut out_ptrs: [*mut u8; 2] =
                        [out_left.as_mut_ptr().cast(), out_right.as_mut_ptr().cast()];

                    let in_ptr = in_data[byte_offset..].as_ptr() as *const u8;
                    let in_ptrs: [*const u8; 1] = [in_ptr];

                    let _ = convert(
                        ctx,
                        out_ptrs.as_mut_ptr(),
                        black_box(out_count),
                        in_ptrs.as_ptr(),
                        black_box(samples_to_process as c_int),
                    );

                    offset += samples_to_process;
                }

                ff_sys::swr_free(&mut ctx);
            });
        });
    }

    group.finish();
}

/// Benchmark real-world audio processing scenarios with synthetic data.
fn bench_real_audio_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("swresample_real_audio");

    // Simulate 1 second of 44.1kHz stereo S16 audio
    let in_rate = 44100;
    let num_samples = in_rate; // 1 second
    let in_data: Vec<i16> = (0..num_samples * 2)
        .map(|i| ((i as f32 / 100.0).sin() * 10000.0) as i16)
        .collect();

    group.throughput(Throughput::Elements(num_samples as u64));

    // Scenario 1: Standard playback (44.1k to 48k FLTP)
    group.bench_function("playback_48k_fltp", |b| {
        b.iter(|| unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLTP,
                48000,
                &in_layout,
                sample_format::S16,
                in_rate,
            )
            .expect("Context creation failed");

            init(ctx).expect("Init failed");

            let out_count = estimate_output_samples(48000, in_rate, num_samples as i32);
            let mut out_left: Vec<f32> = vec![0.0; out_count as usize];
            let mut out_right: Vec<f32> = vec![0.0; out_count as usize];
            let mut out_ptrs: [*mut u8; 2] =
                [out_left.as_mut_ptr().cast(), out_right.as_mut_ptr().cast()];

            let in_ptr = in_data.as_ptr() as *const u8;
            let in_ptrs: [*const u8; 1] = [in_ptr];

            let _ = convert(
                ctx,
                out_ptrs.as_mut_ptr(),
                black_box(out_count),
                in_ptrs.as_ptr(),
                black_box(num_samples as c_int),
            );

            ff_sys::swr_free(&mut ctx);
        });
    });

    // Scenario 2: Export quality (to 96k S32)
    group.bench_function("export_96k_s32", |b| {
        b.iter(|| unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::S32,
                96000,
                &in_layout,
                sample_format::S16,
                in_rate,
            )
            .expect("Context creation failed");

            init(ctx).expect("Init failed");

            let out_count = estimate_output_samples(96000, in_rate, num_samples as i32);
            let mut out_data: Vec<i32> = vec![0; out_count as usize * 2];
            let out_ptr = out_data.as_mut_ptr() as *mut u8;
            let mut out_ptrs: [*mut u8; 1] = [out_ptr];

            let in_ptr = in_data.as_ptr() as *const u8;
            let in_ptrs: [*const u8; 1] = [in_ptr];

            let _ = convert(
                ctx,
                out_ptrs.as_mut_ptr(),
                black_box(out_count),
                in_ptrs.as_ptr(),
                black_box(num_samples as c_int),
            );

            ff_sys::swr_free(&mut ctx);
        });
    });

    // Scenario 3: Preview (fast, lower quality)
    group.bench_function("preview_22k_s16", |b| {
        b.iter(|| unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::S16,
                22050,
                &in_layout,
                sample_format::S16,
                in_rate,
            )
            .expect("Context creation failed");

            init(ctx).expect("Init failed");

            let out_count = estimate_output_samples(22050, in_rate, num_samples as i32);
            let mut out_data: Vec<i16> = vec![0; out_count as usize * 2];
            let out_ptr = out_data.as_mut_ptr() as *mut u8;
            let mut out_ptrs: [*mut u8; 1] = [out_ptr];

            let in_ptr = in_data.as_ptr() as *const u8;
            let in_ptrs: [*const u8; 1] = [in_ptr];

            let _ = convert(
                ctx,
                out_ptrs.as_mut_ptr(),
                black_box(out_count),
                in_ptrs.as_ptr(),
                black_box(num_samples as c_int),
            );

            ff_sys::swr_free(&mut ctx);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_context_creation,
    bench_format_conversion,
    bench_channel_conversion,
    bench_resampling_rates,
    bench_chunk_sizes,
    bench_real_audio_processing
);
criterion_main!(benches);
