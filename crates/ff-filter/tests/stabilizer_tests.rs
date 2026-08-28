//! Integration tests for `Stabilizer::analyze` (pass 1) and `Stabilizer::transform` (pass 2).

mod fixtures;

use std::path::PathBuf;

use ff_filter::{AnalyzeOptions, FilterError, Interpolation, StabilizeOptions, Stabilizer};
use fixtures::{FileGuard, make_source_file, test_output_path};

/// Verifies that `Stabilizer::analyze` produces a non-empty `.trf` file when
/// run against a valid synthetic video clip.
///
/// Acceptance criterion for issue #392.
#[test]
fn analyze_should_produce_nonempty_trf_file() {
    const W: u32 = 64;
    const H: u32 = 64;
    const FPS: f64 = 30.0;
    const FRAMES: usize = 15;

    let src_path = test_output_path("vidstab_src.mp4");
    let trf_path = test_output_path("vidstab_out.trf");

    let _src_guard = FileGuard::new(src_path.clone());
    let _trf_guard = FileGuard::new(trf_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAMES, 128, 128, 128).is_none() {
        println!("Skipping: source encoder unavailable");
        return;
    }

    let result = Stabilizer::analyze(&src_path, &trf_path, &AnalyzeOptions::default());

    match result {
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabdetect not available: {message}");
            return;
        }
        Err(e) => panic!("analyze failed unexpectedly: {e}"),
        Ok(()) => {}
    }

    assert!(
        trf_path.exists(),
        ".trf file should exist after analysis: {trf_path:?}"
    );
    let size = std::fs::metadata(&trf_path)
        .expect("metadata read failed")
        .len();
    assert!(size > 0, ".trf file should be non-empty (got {size} bytes)");
}

/// Verifies that `Stabilizer::analyze` returns `Err(FilterError::Ffmpeg { .. })`
/// when the input file does not exist.
///
/// Acceptance criterion for issue #392.
#[test]
fn analyze_nonexistent_input_should_return_ffmpeg_error() {
    let trf_path = test_output_path("vidstab_nonexistent.trf");
    let _trf_guard = FileGuard::new(trf_path.clone());

    let result = Stabilizer::analyze(
        &PathBuf::from("no_such_file_99999.mp4"),
        &trf_path,
        &AnalyzeOptions::default(),
    );

    match result {
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabdetect not available: {message}");
        }
        Err(FilterError::Ffmpeg { .. }) => {
            // Expected: FFmpeg reported an error opening the non-existent file.
        }
        Err(e) => panic!("expected FilterError::Ffmpeg, got {e:?}"),
        Ok(()) => panic!("expected error for non-existent input, got Ok(())"),
    }
}

// Pass 2 — transform tests

/// Verifies that `Stabilizer::transform` produces a non-empty output file when
/// run through both passes against a valid synthetic video clip.
///
/// Acceptance criterion for issue #393.
#[test]
fn transform_should_produce_valid_output_file() {
    const W: u32 = 64;
    const H: u32 = 64;
    const FPS: f64 = 30.0;
    const FRAMES: usize = 15;

    let src_path = test_output_path("vstab_t_src.mp4");
    let trf_path = test_output_path("vstab_t_out.trf");
    let out_path = test_output_path("vstab_t_output.mp4");

    let _src_guard = FileGuard::new(src_path.clone());
    let _trf_guard = FileGuard::new(trf_path.clone());
    let _out_guard = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAMES, 128, 128, 128).is_none() {
        println!("Skipping: source encoder unavailable");
        return;
    }

    // Pass 1: analyze
    match Stabilizer::analyze(&src_path, &trf_path, &AnalyzeOptions::default()) {
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabdetect not available: {message}");
            return;
        }
        Err(e) => panic!("analyze failed unexpectedly: {e}"),
        Ok(()) => {}
    }

    // Pass 2: transform
    let result = Stabilizer::transform(
        &src_path,
        &trf_path,
        &out_path,
        &StabilizeOptions::default(),
    );

    match result {
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabtransform not available: {message}");
            return;
        }
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("no H.264 encoder available") =>
        {
            println!("Skipping: no H.264 encoder available: {message}");
            return;
        }
        Err(e) => panic!("transform failed unexpectedly: {e}"),
        Ok(()) => {}
    }

    assert!(
        out_path.exists(),
        "output file should exist after transform: {out_path:?}"
    );
    let size = std::fs::metadata(&out_path)
        .expect("metadata read failed")
        .len();
    assert!(
        size > 0,
        "output file should be non-empty (got {size} bytes)"
    );
}

/// Verifies that `Stabilizer::transform` returns `Err(FilterError::Ffmpeg { .. })`
/// when the `.trf` file does not exist.
///
/// Acceptance criterion for issue #393.
#[test]
fn transform_nonexistent_trf_should_return_ffmpeg_error() {
    let src_path = test_output_path("vstab_t_err_src.mp4");
    let out_path = test_output_path("vstab_t_err_out.mp4");

    let _src_guard = FileGuard::new(src_path.clone());
    let _out_guard = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, 64, 64, 30.0, 5, 128, 128, 128).is_none() {
        println!("Skipping: source encoder unavailable");
        return;
    }

    let result = Stabilizer::transform(
        &src_path,
        &PathBuf::from("no_such_trf_99999.trf"),
        &out_path,
        &StabilizeOptions::default(),
    );

    match result {
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabtransform not available: {message}");
        }
        Err(FilterError::Ffmpeg { .. }) => {
            // Expected: FFmpeg reported an error (trf file not found).
        }
        Err(e) => panic!("expected FilterError::Ffmpeg, got {e:?}"),
        Ok(()) => panic!("expected error for non-existent trf, got Ok(())"),
    }
}

/// Verifies that `StabilizeOptions::default()` has the documented field values.
#[test]
fn stabilize_options_default_should_have_expected_values() {
    let opts = StabilizeOptions::default();
    assert_eq!(opts.smoothing, 10);
    assert!(opts.crop_black);
    assert!((opts.zoom - 0.0_f32).abs() < f32::EPSILON);
    assert_eq!(opts.optzoom, 0);
    assert_eq!(opts.interpol, Interpolation::Bilinear);
}

// Issue #412 — motion-variance integration test

/// Verifies that two-pass stabilization produces measurably smoother output
/// than the original shaky clip, measured as mean squared frame-to-frame
/// difference in the top-left 64×64 luma region.
///
/// Acceptance criterion for issue #412.
#[test]
fn two_pass_stabilization_should_reduce_motion_variance() {
    let shaky_path = test_output_path("shaky_clip.mp4");
    let trf_path = test_output_path("shaky_stability.trf");
    let output_path = test_output_path("stabilized_clip.mp4");

    let _tg = FileGuard::new(trf_path.clone());
    let _og = FileGuard::new(output_path.clone());

    // Generate the shaky source clip (cached — deleted by FileGuard only if
    // we make a guard for it; here we keep it for re-runs within the same run).
    let _sg = FileGuard::new(shaky_path.clone());
    if make_shaky_clip(&shaky_path).is_none() {
        println!("Skipping: shaky clip encoder unavailable");
        return;
    }

    // Pass 1: analyze motion.
    match Stabilizer::analyze(&shaky_path, &trf_path, &AnalyzeOptions::default()) {
        Ok(()) => {}
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabdetect unavailable: {message}");
            return;
        }
        Err(e) => panic!("analyze failed: {e}"),
    }
    assert!(
        trf_path.exists() && trf_path.metadata().unwrap().len() > 0,
        ".trf file must be non-empty after analysis"
    );

    // Pass 2: apply transform.
    match Stabilizer::transform(
        &shaky_path,
        &trf_path,
        &output_path,
        &StabilizeOptions::default(),
    ) {
        Ok(()) => {}
        Err(FilterError::Ffmpeg { ref message, .. })
            if message.contains("not available in this FFmpeg build") =>
        {
            println!("Skipping: vidstabtransform unavailable: {message}");
            return;
        }
        Err(e) => panic!("transform failed: {e}"),
    }

    // Measure motion variance before and after stabilization.
    let input_variance = measure_frame_motion_variance(&shaky_path);
    let output_variance = measure_frame_motion_variance(&output_path);

    assert!(
        input_variance > 0.0,
        "input clip must have non-zero motion variance (got {input_variance})"
    );
    assert!(
        output_variance <= input_variance * 0.5,
        "stabilized output must have ≤50% of input motion variance; \
         input={input_variance:.4} output={output_variance:.4}"
    );
}

// Helpers for issue #412

/// Generates a 320×180, 30fps, 60-frame synthetic shaky video at `path`.
///
/// Each frame shows a 16×16 checkerboard whose horizontal position is shifted
/// by a sinusoidal amount (±15 px), producing measurable frame-to-frame jitter.
/// Returns `None` if the encoder is unavailable (caller should skip the test).
fn make_shaky_clip(path: &PathBuf) -> Option<()> {
    use ff_encode::{VideoCodec, VideoEncoder};
    use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

    const W: u32 = 320;
    const H: u32 = 180;
    const FPS: f64 = 30.0;
    const FRAMES: usize = 60;

    if path.exists() {
        return Some(());
    }

    let mut encoder = match VideoEncoder::create(path)
        .video(W, H, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: cannot build shaky clip encoder: {e}");
            return None;
        }
    };

    let stride = W as usize;
    let uv_stride = (W / 2) as usize;
    let uv_h = (H / 2) as usize;

    for i in 0..FRAMES {
        // Sinusoidal horizontal jitter: ±15 pixels.
        let shift = (15.0_f64 * (i as f64 * 0.5).sin()).round() as i32;

        let mut y_plane = vec![0u8; stride * H as usize];
        for row in 0..(H as usize) {
            for col in 0..(W as usize) {
                let eff_col =
                    usize::try_from((col as i32 + shift).rem_euclid(W as i32)).unwrap_or(0);
                let checker = ((row / 16) + (eff_col / 16)) % 2;
                y_plane[row * stride + col] = if checker == 0 { 200 } else { 50 };
            }
        }

        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(y_plane),
                PooledBuffer::standalone(vec![128u8; uv_stride * uv_h]),
                PooledBuffer::standalone(vec![128u8; uv_stride * uv_h]),
            ],
            vec![stride, uv_stride, uv_stride],
            W,
            H,
            PixelFormat::Yuv420p,
            Timestamp::default(),
            true,
        )
        .ok()?;

        if encoder.push_video(&frame).is_err() {
            return None;
        }
    }

    encoder.finish().ok()?;
    Some(())
}

/// Decodes `path` with [`ff_decode::VideoDecoder`] and returns the mean squared
/// frame-to-frame difference of the top-left 64×64 luma region.
///
/// A higher value means more inter-frame motion; `0.0` is returned when fewer
/// than two frames can be decoded.
fn measure_frame_motion_variance(path: &std::path::Path) -> f64 {
    use ff_decode::VideoDecoder;

    let mut decoder = match VideoDecoder::open(path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("measure_frame_motion_variance: failed to open {path:?}: {e}");
            return 0.0;
        }
    };

    const REGION: usize = 64;
    let mut prev_region: Option<Vec<u8>> = None;
    let mut total_msd = 0.0_f64;
    let mut pair_count = 0_u32;

    while let Ok(Some(frame)) = decoder.decode_one() {
        let reg_cols = REGION.min(frame.width() as usize);
        let reg_rows = REGION.min(frame.height() as usize);

        if let (Some(y_data), Some(y_stride)) = (frame.plane(0), frame.stride(0)) {
            let mut region = Vec::with_capacity(reg_cols * reg_rows);
            for row in 0..reg_rows {
                for col in 0..reg_cols {
                    region.push(y_data[row * y_stride + col]);
                }
            }

            if let Some(prev) = prev_region.take() {
                let pixel_count = (reg_cols * reg_rows) as f64;
                let msd = prev
                    .iter()
                    .zip(region.iter())
                    .map(|(&a, &b)| {
                        let diff = i32::from(a) - i32::from(b);
                        f64::from(diff * diff)
                    })
                    .sum::<f64>()
                    / pixel_count;
                total_msd += msd;
                pair_count += 1;
            }

            prev_region = Some(region);
        }
    }

    if pair_count == 0 {
        0.0
    } else {
        total_msd / f64::from(pair_count)
    }
}
