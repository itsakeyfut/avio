//! A trim must start on the same source frame whichever route renders it (#1811).
//!
//! The GPU export seeks (`SeekMode::Exact`) and the CPU export does not: `derive`
//! emits `Trim`/`ATrim` and libavfilter applies them by timestamp. One in-point, two
//! mechanisms, so only a test that reads *which source frame* came out can tell that
//! they agree. Duration cannot: an export one frame late is the same length.
//!
//! The existing fixtures are structurally blind to this, because every frame they
//! write is identical. The source here carries a per-frame index painted as bilevel
//! bands, which a lossy export cannot shift: a luma *level* marker is one rounding
//! step away from its neighbour, while a band that is black or white survives.
//! `marked_source_should_round_trip_its_own_index` proves the instrument before the
//! parity test leans on it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::{Path, PathBuf};
use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_encode::{AudioCodec, VideoCodec, VideoEncoder};
use ff_filter::FilterError;
use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};
use fixtures::{FileGuard, silent_audio_frame, test_output_path};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 128;
const FPS: f64 = 30.0;
const FRAMES: usize = 90;
/// Eight horizontal bands: band 0 says a frame is present, bands 1..=7 carry the
/// index, so indices up to 127 are representable.
const BANDS: usize = 8;

/// A frame painting `index` as bilevel bands.
///
/// Band 0 is always white, so a blank canvas frame reads as "no frame" rather than
/// as index 0, which is the distinction the parity assertions are made of.
fn indexed_frame(index: usize) -> VideoFrame {
    let band_h = HEIGHT as usize / BANDS;
    let mut y = vec![0u8; (WIDTH * HEIGHT) as usize];
    for band in 0..BANDS {
        let set = band == 0 || (index >> (band - 1)) & 1 == 1;
        if !set {
            continue;
        }
        let start = band * band_h * WIDTH as usize;
        let end = start + band_h * WIDTH as usize;
        y[start..end].fill(235);
    }
    let chroma = ((WIDTH / 2) * (HEIGHT / 2)) as usize;
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y),
            PooledBuffer::standalone(vec![128u8; chroma]),
            PooledBuffer::standalone(vec![128u8; chroma]),
        ],
        vec![WIDTH as usize, (WIDTH / 2) as usize, (WIDTH / 2) as usize],
        WIDTH,
        HEIGHT,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("failed to create marked frame")
}

/// Writes a source whose every frame carries its own index. `None` means this build
/// cannot encode it, which is a skip.
fn make_marked_source(path: &PathBuf) -> Option<()> {
    let sample_rate = 48_000u32;
    let audio_frame_samples = 1024usize;
    let audio_frames =
        ((sample_rate as f64 * FRAMES as f64 / FPS) as usize).div_ceil(audio_frame_samples);

    let mut encoder = match VideoEncoder::create(path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .audio(sample_rate, 2)
        .audio_codec(AudioCodec::Aac)
        .audio_bitrate(128_000)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: cannot build source encoder: {e}");
            return None;
        }
    };
    for index in 0..FRAMES {
        if let Err(e) = encoder.push_video(&indexed_frame(index)) {
            println!("Skipping: push_video failed: {e}");
            return None;
        }
    }
    for _ in 0..audio_frames {
        if let Err(e) = encoder.push_audio(&silent_audio_frame(audio_frame_samples, sample_rate)) {
            println!("Skipping: push_audio failed: {e}");
            return None;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder finish failed: {e}");
        return None;
    }
    Some(())
}

/// Reads the index out of every frame of `path`, `None` for a frame carrying no
/// marker (a blank canvas frame, or a band pattern the export destroyed).
fn read_indices(path: &Path) -> Option<Vec<Option<usize>>> {
    let mut decoder = ff_decode::VideoDecoder::open(path)
        .output_format(PixelFormat::Yuv420p)
        .build()
        .ok()?;
    let mut out = Vec::new();
    while let Ok(Some(frame)) = decoder.decode_one() {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let stride = frame.stride(0).unwrap_or(w);
        let plane = frame.plane(0)?;
        let band_h = h / BANDS;
        let mut bits = [false; BANDS];
        for (band, bit) in bits.iter_mut().enumerate() {
            // The rows at a band edge blur into the neighbouring band once the
            // export has scaled or re-encoded, so the mean is taken over the
            // middle half of the band only.
            let lo = band * band_h + band_h / 4;
            let hi = band * band_h + band_h - band_h / 4;
            let mut sum = 0u64;
            let mut n = 0u64;
            for row in lo..hi {
                for &v in &plane[row * stride..row * stride + w] {
                    sum += u64::from(v);
                    n += 1;
                }
            }
            *bit = n > 0 && sum / n > 128;
        }
        out.push(bits[0].then(|| {
            (1..BANDS)
                .filter(|&b| bits[b])
                .map(|b| 1usize << (b - 1))
                .sum()
        }));
    }
    (!out.is_empty()).then_some(out)
}

/// Renders `timeline` and reads the frame indices back, or `None` where this build
/// cannot take part.
fn render_indices(
    timeline: Timeline,
    out: &PathBuf,
    force_cpu: bool,
) -> Option<Vec<Option<usize>>> {
    let config = EncoderConfig::builder().build();
    let rendered = if force_cpu {
        timeline.render_forcing_cpu(out, config)
    } else {
        timeline.render(out, config)
    };
    match rendered {
        Ok(()) => {}
        Err(TimelineError::Filter(FilterError::CompositionFailed { ref reason }))
            if reason.contains("filter not found") =>
        {
            println!("Skipping: this build lacks a filter the composition needs: {reason}");
            return None;
        }
        Err(TimelineError::Filter(FilterError::BuildFailed)) => {
            println!("Skipping: the graph could not be built here");
            return None;
        }
        Err(ref e @ (TimelineError::Encode(_) | TimelineError::Decode(_))) => {
            println!("Skipping: this build cannot run the pipeline: {e}");
            return None;
        }
        Err(e) => panic!("render failed: {e}"),
    }
    read_indices(out)
}

/// A timeline holding one clip trimmed from `start` for one second.
fn trimmed(src: &Path, start: f64) -> Option<Timeline> {
    let clip = Clip::new(src).trim(
        Duration::from_secs_f64(start),
        Duration::from_secs_f64(start + 1.0),
    );
    match Timeline::builder()
        .canvas(WIDTH, HEIGHT)
        .frame_rate(FPS)
        .video_track(vec![clip])
        .build()
    {
        Ok(t) => Some(t),
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            None
        }
    }
}

/// Whether the GPU export route can actually run here.
///
/// Without an adapter `render()` falls back to the CPU composer, so the comparison
/// would be the CPU route against itself: green while the seek this test exists for is
/// never reached. Naming `GpuCompositor::new` also marks this target as a GPU one for
/// `cargo xtask test`, which then runs it on its own (#1718).
#[cfg(feature = "gpu")]
fn gpu_route_available() -> bool {
    avio::GpuCompositor::new().is_some()
}

#[cfg(not(feature = "gpu"))]
fn gpu_route_available() -> bool {
    false
}

#[test]
fn marked_source_should_round_trip_its_own_index() {
    let src = test_output_path("trim_parity_control_src.mp4");
    let _g = FileGuard::new(src.clone());
    let Some(()) = make_marked_source(&src) else {
        return;
    };
    let Some(indices) = read_indices(&src) else {
        println!("Skipping: cannot decode the marked source here");
        return;
    };
    let expected: Vec<Option<usize>> = (0..indices.len()).map(Some).collect();
    assert_eq!(
        indices, expected,
        "the marker must survive its own encode before any test leans on it"
    );
}

#[test]
fn trims_should_start_on_the_same_source_frame_on_both_routes() {
    let src = test_output_path("trim_parity_src.mp4");
    let _g = FileGuard::new(src.clone());
    let Some(()) = make_marked_source(&src) else {
        return;
    };

    // A head trim and a middle trim: the middle one is where a seek happens at all.
    for (tag, start) in [("head", 0.0f64), ("mid", 1.0f64)] {
        let gpu_out = test_output_path(&format!("trim_parity_{tag}_gpu.mp4"));
        let cpu_out = test_output_path(&format!("trim_parity_{tag}_cpu.mp4"));
        let (_gg, _gc) = (
            FileGuard::new(gpu_out.clone()),
            FileGuard::new(cpu_out.clone()),
        );

        let expected = (start * FPS).round() as usize;

        let Some(t_cpu) = trimmed(&src, start) else {
            return;
        };
        let Some(cpu) = render_indices(t_cpu, &cpu_out, true) else {
            return;
        };
        assert_eq!(
            cpu.first().copied().flatten(),
            Some(expected),
            "{tag}: the CPU route must start on the frame the trim names"
        );

        if !gpu_route_available() {
            println!("Skipping the GPU leg: no adapter here, so render() would not seek");
            continue;
        }
        let Some(t_gpu) = trimmed(&src, start) else {
            return;
        };
        let Some(gpu) = render_indices(t_gpu, &gpu_out, false) else {
            return;
        };
        assert_eq!(
            gpu.first().copied().flatten(),
            Some(expected),
            "{tag}: the default route must start on the frame the trim names"
        );
        // The two routes may not end on the same frame (the trim's out-point is a
        // separate question), so the comparison is of the common prefix.
        let n = gpu.len().min(cpu.len());
        assert_eq!(
            gpu[..n],
            cpu[..n],
            "{tag}: the two routes must produce the same source frames"
        );
    }
}
