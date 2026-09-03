//! Where a transition sits on the timeline, and what it costs (#1731, ADR-0009).
//!
//! A transition preserves the timeline length and is fed by the outgoing clip's handle
//! -- its frames past the out-point. Every assertion here was false before that decision
//! landed, each in its own way:
//!
//! - a 0.5 s transition turned a 60-frame hard cut into 45, so the video lost half a
//!   second the audio did not;
//! - a transition on a *middle* clip left the clips after it at their absolute
//!   `OffsetPts` over a stream that had shrunk, opening 15 frames of pure black;
//! - chained transitions each took their offset from the *previous clip's own length*
//!   rather than the accumulated stream, so the second one fired early.
//!
//! The sources are flat colours, one per clip, which makes "which clip is on screen" a
//! decidable question per frame -- and that is what placement is about. Pixel *fidelity*
//! is not: `xfade_reference_parity` and `gpu_export_tests` own that, and this suite would
//! pass just as well with a wrong blend formula, deliberately.
//!
//! Every source is longer than the clip trimmed out of it, because a handle is exactly
//! the material past the out-point. A source cut flush has none, which is its own test
//! below.
//!
//! **Every clip here runs at speed 1.0, and that is a coverage gap this suite cannot
//! close.** Placement has two speed-sensitive quantities (the stream offset and the
//! source-time handle), but a transition anywhere on a track holding a speed-changed
//! clip fails to build its filter graph at all -- `xfade` reports "First input link main
//! timebase (1/30) do not match ... (1/30000)", because the `fps` filter that follows
//! `Speed` retimes only that one input. Verified on `main` in a clean worktree, so it
//! predates this change; tracked as #1739. Until it is fixed the speed arithmetic
//! is pinned by unit tests instead: `transition::tests::composited_secs_*` /
//! `to_source_*` and `derive::tests::video_layer_should_convert_the_handle_to_source_*`.
//!
//! Probe-gated (RK-002): every leg skips when the environment cannot encode, export, or
//! decode. Each export writes to a path keyed by its test, so the suite is safe at
//! default parallelism (RK-019).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::path::Path;
use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_decode::VideoDecoder;
use ff_encode::{AudioCodec, BitrateMode, VideoCodec, VideoEncoder};
use ff_filter::XfadeTransition;
use ff_format::{AudioFrame, PixelFormat, SampleFormat, VideoFrame};
use fixtures::{FileGuard, test_output_path};

const W: u32 = 64;
const H: u32 = 64;
const FPS: f64 = 30.0;

/// Frames each clip contributes: 1 s.
const CLIP: usize = 30;
/// Transition length in output frames: 0.5 s.
const WINDOW: usize = 15;
/// Frames written into each source file. Everything past the clip's out-point is the
/// handle the blend reads; without it the transition would clamp to a hard cut. It is
/// twice the window on purpose: a container reports its duration one frame interval
/// short of what was pushed, so a source sized to exactly `CLIP + WINDOW` yields a
/// handle of 0.467 s and quietly shortens the transition by a frame.
const SOURCE_FRAMES: usize = CLIP + 2 * WINDOW;

/// An encode/decode round trip can lose or duplicate a frame at the container's edge, so
/// frame-count assertions allow one either way.
const FRAME_SLACK: usize = 1;

/// How far a frame may sit from a source colour and still count as "that clip alone".
/// Mpeg4 at the bitrate below reproduces a flat fill to within a couple of levels; a
/// half-way `Fade` between any two of these colours is over 60 away.
const SOLID_TOL: f64 = 12.0;

/// The three clip colours. Far apart in every channel, so a blend of any two is nowhere
/// near a third -- otherwise a mid-transition frame could be misread as a clean clip.
const COLORS: [[u8; 3]; 3] = [[200, 30, 30], [30, 200, 30], [30, 30, 200]];

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        // Generous: this suite decides which clip a frame came from, so the encoder must
        // not be what moves a colour.
        .bitrate_mode(BitrateMode::Cbr(4_000_000))
        .build()
}

/// Writes `SOURCE_FRAMES` frames of a flat `color`, or `None` when there is no encoder.
fn make_solid_source(path: &Path, color: [u8; 3]) -> Option<()> {
    make_solid_source_n(path, color, SOURCE_FRAMES)
}

/// Writes `frames` frames of a flat `color`, or `None` when there is no encoder.
fn make_solid_source_n(path: &Path, color: [u8; 3], frames: usize) -> Option<()> {
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
    for _ in 0..frames {
        enc.push_video(&VideoFrame::from_rgba(W, H, rgba.clone()).ok()?)
            .ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// Writes `secs` seconds of silence, or `None` when there is no audio encoder.
fn make_silent_audio(path: &Path, secs: u32) -> Option<()> {
    const RATE: u32 = 48_000;
    // `new_silent` fixes its own frame length; the loop counts in those.
    const CHUNK: u32 = 1024;
    let mut enc = VideoEncoder::create(path)
        .audio(RATE, 2)
        .audio_codec(AudioCodec::Aac)
        .build()
        .ok()?;
    let chunks = (RATE * secs).div_ceil(CHUNK);
    for i in 0..chunks {
        let pts_ms = i64::from(i) * 1000 * i64::from(CHUNK) / i64::from(RATE);
        let frame = AudioFrame::new_silent(RATE, 2, SampleFormat::F32p, pts_ms);
        enc.push_audio(&frame).ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// Every decoded frame of `path` as rgba, or an empty vec when it cannot be decoded.
fn decode_rgba(path: &Path) -> Vec<Vec<u8>> {
    let Ok(mut d) = VideoDecoder::open(path)
        .output_format(PixelFormat::Rgba)
        .build()
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(f)) = d.decode_one() {
        if let Some(plane) = f.plane(0) {
            out.push(plane.to_vec());
        }
    }
    out
}

/// The frame's mean RGB, which is all a flat fill needs.
fn mean_rgb(frame: &[u8]) -> [f64; 3] {
    let n = frame.len() / 4;
    assert!(n > 0, "measured an empty frame");
    let mut sum = [0f64; 3];
    for px in frame.chunks_exact(4) {
        for c in 0..3 {
            sum[c] += f64::from(px[c]);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let denom = n as f64;
    [sum[0] / denom, sum[1] / denom, sum[2] / denom]
}

/// Distance from a frame's mean colour to `color`.
fn distance_to(frame: &[u8], color: [u8; 3]) -> f64 {
    let m = mean_rgb(frame);
    (0..3)
        .map(|c| (m[c] - f64::from(color[c])).abs())
        .fold(0f64, f64::max)
}

/// Which of [`COLORS`] this frame *is*, or `None` when it is a blend of two of them (or
/// anything else, black included).
fn solid_clip(frame: &[u8]) -> Option<usize> {
    (0..COLORS.len()).find(|&i| distance_to(frame, COLORS[i]) <= SOLID_TOL)
}

fn render_or_skip(result: Result<(), TimelineError>) -> bool {
    match result {
        Ok(()) => true,
        Err(TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)) => {
            false
        }
        Err(e) => panic!("unexpected export error: {e}"),
    }
}

/// One clip: `idx` seconds in, one second long, optionally transitioned into.
fn clip(path: &Path, idx: u64, transition: Option<XfadeTransition>) -> Clip {
    let c = Clip::new(path)
        .offset(Duration::from_secs(idx))
        .trim(Duration::ZERO, Duration::from_secs(1));
    match transition {
        Some(kind) => c.with_transition(kind, Duration::from_millis(500)),
        None => c,
    }
}

/// Encodes `n` flat sources and returns their paths plus the guards that delete them.
fn solid_sources(tag: &str, n: usize) -> Option<(Vec<std::path::PathBuf>, Vec<FileGuard>)> {
    let mut paths = Vec::new();
    let mut guards = Vec::new();
    for i in 0..n {
        let p = test_output_path(&format!("tplace_{tag}_src{i}.mp4"));
        guards.push(FileGuard::new(p.clone()));
        make_solid_source(&p, COLORS[i])?;
        paths.push(p);
    }
    Some((paths, guards))
}

/// Renders `clips` and returns the exported frames, or `None` to skip.
fn export(tag: &str, clips: Vec<Clip>) -> Option<Vec<Vec<u8>>> {
    let timeline = Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(clips)
        .build()
        .ok()?;
    let out = test_output_path(&format!("tplace_{tag}_out.mp4"));
    let _guard = FileGuard::new(out.clone());
    let _ = std::fs::remove_file(&out);
    if !render_or_skip(timeline.render_forcing_cpu(&out, export_config())) {
        return None;
    }
    let frames = decode_rgba(&out);
    if frames.is_empty() {
        return None; // decoder unavailable -> skip
    }
    Some(frames)
}

fn assert_near(actual: usize, expected: usize, what: &str) {
    assert!(
        actual.abs_diff(expected) <= FRAME_SLACK,
        "{what}: expected ~{expected} frames, got {actual}"
    );
}

#[test]
fn transition_should_preserve_the_hard_cut_length() {
    let Some((src, _guards)) = solid_sources("len", 2) else {
        return; // encoder unavailable -> skip
    };

    let Some(cut) = export(
        "len_cut",
        vec![clip(&src[0], 0, None), clip(&src[1], 1, None)],
    ) else {
        return;
    };
    let Some(faded) = export(
        "len_fade",
        vec![
            clip(&src[0], 0, None),
            clip(&src[1], 1, Some(XfadeTransition::Fade)),
        ],
    ) else {
        return;
    };

    println!("hard cut {} frames / faded {}", cut.len(), faded.len());
    assert_near(cut.len(), CLIP * 2, "the hard cut");
    assert_near(
        faded.len(),
        cut.len(),
        "a transition must not change the timeline length; it is fed by the outgoing \
         clip's handle, not by material taken out of the timeline (ADR-0009)",
    );

    // And it really blended: the window frames belong to neither clip alone. Not all
    // WINDOW of them read that way, and should not -- a linear blend starts *at* clip A,
    // so its first two steps (170 levels apart, 1/15 of the way each) are still inside
    // SOLID_TOL of A, and its last one is inside it of B. Measured: 12 of 15.
    let mixed = faded.iter().filter(|f| solid_clip(f).is_none()).count();
    assert!(
        mixed >= WINDOW - 4,
        "expected ~{WINDOW} blended frames in a 0.5 s transition, found {mixed}; the \
         length is right but nothing was cross-faded"
    );
}

#[test]
fn transition_on_a_middle_clip_should_emit_no_black_frames() {
    // The failure this guards: the composited stream used to shrink by the transition
    // while the *third* clip kept its absolute `OffsetPts`, leaving a hole that encoded
    // as pure black.
    let Some((src, _guards)) = solid_sources("mid", 3) else {
        return;
    };
    let Some(frames) = export(
        "mid",
        vec![
            clip(&src[0], 0, None),
            clip(&src[1], 1, Some(XfadeTransition::Fade)),
            clip(&src[2], 2, None),
        ],
    ) else {
        return;
    };

    assert_near(
        frames.len(),
        CLIP * 3,
        "three 1 s clips with one transition",
    );

    let darkest = frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let m = mean_rgb(f);
            (m[0] + m[1] + m[2], i)
        })
        .fold(
            (f64::MAX, 0usize),
            |acc, x| if x.0 < acc.0 { x } else { acc },
        );
    println!("darkest frame {} sums to {:.1}", darkest.1, darkest.0);
    // Every source colour sums to 260; a blend of two of them to at least 260. A gap
    // frame is 0. The bound sits far below the content and far above black.
    assert!(
        darkest.0 > 100.0,
        "frame {} is (near) black (channel sum {:.1}): the transition opened a hole in \
         the stream instead of preserving its length",
        darkest.1,
        darkest.0
    );
}

#[test]
fn chained_transitions_should_each_blend_at_their_own_boundary() {
    // The failure this guards: the `xfade` offset came from the *previous clip's own
    // duration* rather than the accumulated stream, so the second transition in a chain
    // fired at 1 s instead of 2 s and overlapped the first.
    let Some((src, _guards)) = solid_sources("chain", 3) else {
        return;
    };
    let Some(frames) = export(
        "chain",
        vec![
            clip(&src[0], 0, None),
            clip(&src[1], 1, Some(XfadeTransition::Fade)),
            clip(&src[2], 2, Some(XfadeTransition::Fade)),
        ],
    ) else {
        return;
    };

    assert_near(
        frames.len(),
        CLIP * 3,
        "three 1 s clips with two transitions",
    );

    // Sample inside each region rather than at its edges, so one frame of round-trip
    // slack cannot decide the test.
    let probes: [(usize, Option<usize>, &str); 6] = [
        (CLIP / 2, Some(0), "clip A alone, before any transition"),
        (CLIP + WINDOW / 2, None, "the first transition window"),
        (CLIP + WINDOW + 3, Some(1), "clip B alone, after the first"),
        (2 * CLIP + WINDOW / 2, None, "the second transition window"),
        (
            2 * CLIP + WINDOW + 3,
            Some(2),
            "clip C alone, after the second",
        ),
        (3 * CLIP - 3, Some(2), "clip C at the end"),
    ];
    for (idx, expected, what) in probes {
        let Some(frame) = frames.get(idx) else {
            panic!("frame {idx} missing ({what}); the export is too short");
        };
        let got = solid_clip(frame);
        let m = mean_rgb(frame);
        println!(
            "frame {idx} ({what}): clip={got:?} mean=({:.0},{:.0},{:.0})",
            m[0], m[1], m[2]
        );
        assert_eq!(
            got, expected,
            "frame {idx} should be {what}, but reads as clip {got:?} \
             (mean {:.0},{:.0},{:.0})",
            m[0], m[1], m[2]
        );
    }
}

#[test]
fn transition_should_not_shift_the_video_against_the_audio() {
    // Adding a *video* transition used to shorten the video by its duration while the
    // audio track kept its length: measured 1.467 s of video against 2.000 s of audio.
    let Some((src, _guards)) = solid_sources("av", 2) else {
        return;
    };
    let audio = test_output_path("tplace_av_audio.m4a");
    let _ga = FileGuard::new(audio.clone());
    if make_silent_audio(&audio, 2).is_none() {
        return; // no audio encoder -> skip
    }

    let timeline = Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![
            clip(&src[0], 0, None),
            clip(&src[1], 1, Some(XfadeTransition::Fade)),
        ])
        .audio_track(vec![
            Clip::new(&audio).trim(Duration::ZERO, Duration::from_secs(2)),
        ])
        .build();
    let Ok(timeline) = timeline else {
        return;
    };

    let out = test_output_path("tplace_av_out.mp4");
    let _go = FileGuard::new(out.clone());
    let _ = std::fs::remove_file(&out);
    if !render_or_skip(timeline.render_forcing_cpu(&out, export_config())) {
        return;
    }
    let Ok(info) = ff_probe::open(&out) else {
        return; // probe unavailable -> skip
    };
    let (Some(v), Some(a)) = (info.primary_video(), info.primary_audio()) else {
        return; // the export carries only one of the two -> nothing to compare
    };
    let (Some(vd), Some(ad)) = (v.duration(), a.duration()) else {
        return; // the container reports no per-stream duration -> nothing to compare
    };
    let (vd, ad) = (vd.as_secs_f64(), ad.as_secs_f64());
    println!("video {vd:.3}s / audio {ad:.3}s");
    assert!(
        (vd - ad).abs() < 0.1,
        "the video is {vd:.3}s against {ad:.3}s of audio: a video transition moved the \
         picture out of sync with audio nothing asked to change"
    );
}

#[test]
fn a_transition_with_no_handle_should_degrade_to_a_hard_cut_of_the_same_length() {
    // The clamp: the outgoing clip is trimmed flush to the end of its source, so there
    // are no frames past its out-point to blend against. The blend shrinks to nothing --
    // and, crucially, the timeline still runs its full length, so nothing downstream
    // moves.
    let Some((src, _guards)) = solid_sources("flush", 2) else {
        return;
    };
    #[allow(clippy::cast_precision_loss)]
    let source_secs = SOURCE_FRAMES as f64 / FPS;
    let flush = Clip::new(&src[0])
        .offset(Duration::ZERO)
        .trim(Duration::ZERO, Duration::from_secs_f64(source_secs));
    let incoming = Clip::new(&src[1])
        .offset(Duration::from_secs_f64(source_secs))
        .trim(Duration::ZERO, Duration::from_secs(1))
        .with_transition(XfadeTransition::Fade, Duration::from_millis(500));

    let Some(frames) = export("flush", vec![flush, incoming]) else {
        return;
    };
    assert_near(
        frames.len(),
        SOURCE_FRAMES + CLIP,
        "a clamped transition still preserves the timeline length",
    );
    let mixed = frames.iter().filter(|f| solid_clip(f).is_none()).count();
    assert!(
        mixed <= 2,
        "with no handle there is nothing to blend, so this must be a hard cut; found \
         {mixed} blended frames"
    );
}
