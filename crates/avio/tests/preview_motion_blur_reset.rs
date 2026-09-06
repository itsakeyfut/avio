//! A motion-blur trail must not bleed across a clip cut in preview (#1705).
//!
//! Motion blur is stateful: the exposure trail accumulates by the effect graph being
//! *reused* across a clip's frames, so the graph is where the state lives. The export
//! drain has always dropped it at each clip boundary; playback did not, so the
//! outgoing clip's trail bled into the incoming clip's first frame.
//!
//! This drives the **real** runner over a **`Timeline::to_scene`** derivation with the
//! **real** `GpuPreviewCompositor` injected, so the trail is the actual one the node
//! accumulates rather than a stand-in. The fixture is a white clip cut to a black one:
//! without the reset the first black frame reads bright, with it the frame is black.
//!
//! A hard cut, deliberately — a cross-fade blends by design, so "unblended" would mean
//! nothing there.
//!
//! Probe-gated (RK-002 / adapter): skips when the environment cannot encode, open the
//! preview, or provide a GPU adapter. Its source files are keyed to this suite, so it
//! is safe at default parallelism (RK-019).

#![cfg(all(feature = "preview", feature = "gpu"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{Clip, GpuPreviewCompositor, PlayerHandle, Timeline, TimelinePlayer};
use ff_encode::{VideoCodec, VideoEncoder};
use ff_filter::FilterStep;
use ff_format::VideoFrame;
use ff_preview::FrameSink;
use fixtures::{FileGuard, test_output_path};

const W: u32 = 64;
const H: u32 = 64;
const FPS: f64 = 30.0;
const SOURCE_FRAMES: usize = 45;
/// Each clip runs one second, so the cut is at 1 s.
const CLIP: Duration = Duration::from_secs(1);
/// A spin guard: a healthy run delivers ~60 frames (RK-019).
const MAX_FRAMES: u32 = 200;

/// Records `(pts, mean red)` for every delivered frame.
struct RecordingSink {
    frames: Arc<Mutex<Vec<(Duration, f64)>>>,
    count: u32,
    handle: PlayerHandle,
}

impl FrameSink for RecordingSink {
    fn push_frame(&mut self, rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
        let n = rgba.len() / 4;
        if n > 0 {
            let sum: f64 = rgba.chunks_exact(4).map(|px| f64::from(px[0])).sum();
            self.frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((pts, sum / n as f64));
        }
        self.count += 1;
        if self.count >= MAX_FRAMES {
            self.handle.stop();
        }
    }
}

/// Writes `SOURCE_FRAMES` frames of a flat colour, or `None` when there is no encoder.
fn make_source(path: &std::path::Path, color: [u8; 3]) -> Option<()> {
    let mut enc = VideoEncoder::create(path)
        .video(W, H, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .build()
        .ok()?;
    let mut rgba = vec![255u8; (W * H * 4) as usize];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[0] = color[0];
        px[1] = color[1];
        px[2] = color[2];
    }
    for _ in 0..SOURCE_FRAMES {
        enc.push_video(&VideoFrame::from_rgba(W, H, rgba.clone()).ok()?)
            .ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// A white clip cut to a black one, both carrying motion blur.
fn cut_timeline(white: &std::path::Path, black: &std::path::Path) -> Option<Timeline> {
    let blurred = |p: &std::path::Path, at: Duration| {
        Clip::new(p)
            .offset(at)
            .trim(Duration::ZERO, CLIP)
            .with_video_effect(FilterStep::MotionBlur {
                shutter_angle_degrees: 350.0,
                sub_frames: 8,
            })
    };
    Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![blurred(white, Duration::ZERO), blurred(black, CLIP)])
        .build()
        .ok()
}

/// Seeks into the black clip once it has seen `after` frames, then records what
/// arrives.
struct SeekingSink {
    frames: Arc<Mutex<Vec<(Duration, f64)>>>,
    count: u32,
    after: u32,
    seeked: bool,
    target: Duration,
    handle: PlayerHandle,
}

impl FrameSink for SeekingSink {
    fn push_frame(&mut self, rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
        let n = rgba.len() / 4;
        if n > 0 {
            let sum: f64 = rgba.chunks_exact(4).map(|px| f64::from(px[0])).sum();
            self.frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((pts, sum / n as f64));
        }
        self.count += 1;
        if !self.seeked && self.count >= self.after {
            // Ten frames of white is plenty for the trail to saturate.
            self.seeked = true;
            self.handle.seek(self.target);
        }
        if self.count >= MAX_FRAMES {
            self.handle.stop();
        }
    }
}

#[test]
fn a_motion_blur_trail_should_not_survive_a_seek_in_preview() {
    // A seek crosses a clip boundary too, and even within one clip it invalidates the
    // trail: what accumulated came from frames that no longer precede the new
    // position. The audio side of `seek_timeline` has always discarded its equivalent
    // (`invalidate_all`); this is the visual half.
    //
    // Plays the white clip until the trail saturates, seeks into the black clip, and
    // looks at what arrives: a surviving trail reads bright.
    let white = test_output_path("mbseek_white.mp4");
    let black = test_output_path("mbseek_black.mp4");
    let _gw = FileGuard::new(white.clone());
    let _gb = FileGuard::new(black.clone());
    if make_source(&white, [255, 255, 255]).is_none() {
        return; // no encoder
    }
    if make_source(&black, [0, 0, 0]).is_none() {
        return;
    }
    let Some(compositor) = GpuPreviewCompositor::new() else {
        return; // no adapter
    };
    let Some(timeline) = cut_timeline(&white, &black) else {
        return;
    };
    let Ok((mut runner, handle)) = TimelinePlayer::open(&timeline) else {
        return;
    };

    let target = CLIP + Duration::from_millis(400);
    let frames = Arc::new(Mutex::new(Vec::new()));
    runner.set_gpu_compositor(Box::new(compositor));
    runner.set_sink(Box::new(SeekingSink {
        frames: Arc::clone(&frames),
        count: 0,
        after: 10,
        seeked: false,
        target,
        handle,
    }));
    if runner.run().is_err() {
        return;
    }

    let frames = frames
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The first frame at or after the seek target. A little slack: a seek lands on the
    // nearest decodable frame, not exactly on the requested pts.
    let landed = frames
        .iter()
        .find(|(pts, _)| *pts + Duration::from_millis(100) >= target)
        .copied();
    let Some((pts, mean)) = landed else {
        println!(
            "skipping: the run never reached the seek target ({} frames)",
            frames.len()
        );
        return;
    };
    let before: Vec<f64> = frames.iter().take(3).map(|(_, m)| *m).collect();
    println!("preview seek: before={before:?} -> first frame at {pts:?} = {mean:.1}");

    assert!(
        before.iter().any(|m| *m > 128.0),
        "the control must play the bright clip before the seek, got {before:?}"
    );
    assert!(
        mean < 32.0,
        "the first frame after a seek must be unblended; a surviving trail reads          bright, and this one read {mean:.1}"
    );
}

#[test]
fn a_motion_blur_trail_should_not_bleed_across_a_clip_cut_in_preview() {
    let white = test_output_path("mbreset_white.mp4");
    let black = test_output_path("mbreset_black.mp4");
    let _gw = FileGuard::new(white.clone());
    let _gb = FileGuard::new(black.clone());
    if make_source(&white, [255, 255, 255]).is_none() {
        return; // no encoder
    }
    if make_source(&black, [0, 0, 0]).is_none() {
        return;
    }
    let Some(compositor) = GpuPreviewCompositor::new() else {
        return; // no adapter
    };
    let Some(timeline) = cut_timeline(&white, &black) else {
        return;
    };
    let Ok((mut runner, handle)) = TimelinePlayer::open(&timeline) else {
        return; // the environment cannot open the preview
    };

    let frames = Arc::new(Mutex::new(Vec::new()));
    runner.set_gpu_compositor(Box::new(compositor));
    runner.set_sink(Box::new(RecordingSink {
        frames: Arc::clone(&frames),
        count: 0,
        handle,
    }));
    if runner.run().is_err() {
        return;
    }

    let frames = frames
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The last frame of the white clip and the first of the black one. The cut is at
    // 1 s, and the runner delivers at the timeline rate.
    let last_white = frames
        .iter()
        .filter(|(pts, _)| *pts < CLIP)
        .next_back()
        .copied();
    let first_black = frames.iter().find(|(pts, _)| *pts >= CLIP).copied();
    let (Some((_, white_mean)), Some((cut_pts, black_mean))) = (last_white, first_black) else {
        println!(
            "skipping: the run did not cross the cut ({} frames)",
            frames.len()
        );
        return;
    };
    println!(
        "preview across the cut: white={white_mean:.1} -> first black at {cut_pts:?} = {black_mean:.1}"
    );

    assert!(
        white_mean > 128.0,
        "the control must actually be a bright clip before the cut, got {white_mean:.1}"
    );
    assert!(
        black_mean < 32.0,
        "the first frame after the cut must be unblended; a trail bleeding across it \
         reads bright, and this one read {black_mean:.1}"
    );
}
