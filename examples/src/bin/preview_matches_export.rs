//! Verify that the real-time preview and the export agree: drive one timeline through
//! `TimelinePlayer` and through `Timeline::render`, and compare what each produced.
//!
//! `crates/avio/tests/preview_export_parity.rs` makes this claim inside the crate. This
//! script makes it from outside, through the public facade a consumer has: the preview's
//! only output channel is the sink `SceneRunner::set_sink` takes, so a consumer has to
//! be able to name and implement `FrameSink` for the preview to be observable at all.
//!
//! The clip carries a non-neutral `ColorCorrect`, so an effect reaches both pipelines
//! rather than both merely copying frames through.
//!
//! What is compared: the frame size, the number of frames, the span the preview covered,
//! and the colour of each frame against the export's frame at the same index. Frame by
//! frame rather than over the sequence: the fixture sweeps a full hue circle, so its
//! sequence average is achromatic and a per-channel divergence would cancel out in it
//! (the average measures spread 1.3, below this script's own blank-frame threshold).
//!
//! Not per-pixel convergence, which is out of scope for v0.18: the preview composites in
//! rgba while the export normalises to `yuv420p`, and the export is measured after a
//! lossy encode, so the two land on different numbers for reasons that have nothing to
//! do with a regression.
//!
//! ```bash
//! cargo run -p avio-examples --bin preview_matches_export
//! cargo run -p avio-examples --bin preview_matches_export -- --input clip.mp4 --keep
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{Clip, FrameSink, Pacing, PlayerHandle, Timeline, TimelinePlayer};
use avio_examples::{
    BoxResult, Report, channel_distance, decoded_frame_means, mean_rgb, parse_args, render_or_skip,
    resolve_input,
};

/// Contrast for the clip's colour correction. Non-neutral, so an effect actually reaches
/// both pipelines (a neutral `ColorCorrect` compiles to no filter at all).
const CONTRAST: f32 = 1.3;
/// Per-channel difference allowed between a preview frame and the export frame at the
/// same index, taken at its worst over the clip. Coarse by design, for the
/// rgba-vs-`yuv420p` and lossy-encode reasons above. Measured 6.85 at its worst on the
/// synthetic fixture; the margin is for content and builds that are less forgiving, not
/// for slack. Swapping the preview's red and blue channels measures 254.65 here, so the
/// tolerance is nowhere near wide enough to hide a per-channel divergence.
const TOL_MEAN_CHANNEL: f64 = 24.0;
/// Frames the two legs may differ by. The encoder's flush can add one, and the runner
/// may not deliver the very last frame before EOF.
const TOL_FRAMES: usize = 2;
/// Minimum spread between the brightest and darkest channel of the first frame. The
/// fixture starts on a saturated hue, so a blank, white or grey frame, which is the
/// shape a broken leg produces, falls below this.
const MIN_CHROMA_SPREAD: f64 = 30.0;
/// Multiple of the timeline's own frame count at which the preview leg gives up. A spin
/// guard for a runner that fails to terminate, so it has to scale with the input: a
/// fixed cap would cut a long `--input` clip short and report that as a mismatch.
const PREVIEW_FRAME_CAP_FACTOR: usize = 4;

/// What the preview leg delivered.
///
/// Every frame's mean colour is kept rather than summed: the comparison against the
/// export is per frame, because the sequence average is achromatic on a hue-sweeping
/// fixture and would hide a per-channel divergence.
#[derive(Default, Clone)]
struct PreviewStats {
    size: Option<(u32, u32)>,
    frame_means: Vec<[f64; 3]>,
    last_pts: Duration,
}

/// Collects [`PreviewStats`] from the runner's presentation thread.
struct StatsSink {
    stats: Arc<Mutex<PreviewStats>>,
    handle: PlayerHandle,
    cap: usize,
}

impl FrameSink for StatsSink {
    fn push_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts: Duration) {
        let mean = mean_rgb(rgba);
        let mut stats = match self.stats.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        stats.size.get_or_insert((width, height));
        stats.frame_means.push(mean);
        stats.last_pts = pts;
        if stats.frame_means.len() >= self.cap {
            self.handle.stop();
        }
    }
}

/// Plays `timeline` unpaced and returns what the preview delivered, or `Err(reason)`
/// when the environment cannot open or run the player (skip).
///
/// The stats are read back through a retained handle rather than reclaimed from the
/// sink, so there is no "could not take the stats back" path that would have to be
/// reported as a skip when it is not an environment problem at all.
fn preview_stats(timeline: &Timeline, cap: usize) -> Result<PreviewStats, String> {
    let (mut runner, handle) =
        TimelinePlayer::open(timeline).map_err(|e| format!("preview unavailable: {e}"))?;
    let stats = Arc::new(Mutex::new(PreviewStats::default()));
    // Unpaced: the real-time clock drops late frames, so a paced run would compare the
    // export against whatever this machine kept up with (ADR-0015).
    runner.set_pacing(Pacing::Unpaced);
    runner.set_sink(Box::new(StatsSink {
        stats: Arc::clone(&stats),
        handle,
        cap,
    }));
    runner.run().map_err(|e| format!("preview failed: {e}"))?;
    let collected = stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Ok(collected)
}

fn main() -> BoxResult<()> {
    let args = parse_args();
    let tmp = tempfile::tempdir()?;
    let mut report = Report::new("preview_matches_export");
    // A machine with no encoder cannot even make the fixture; that is the environment,
    // not a regression, so it skips like the two legs below.
    let input = match resolve_input(&args, tmp.path()) {
        Ok(path) => path,
        Err(e) => {
            report.skip("generate the input clip", &e.to_string());
            return report.finish();
        }
    };

    let in_info = avio::open(&input)?;
    let in_video = in_info.video_streams();
    let Some(v) = in_video.first() else {
        return Err("input has no video stream".into());
    };
    let (canvas_w, canvas_h, rate) = (v.width(), v.height(), v.frame_rate());
    // The ratio drives the timeline; the decimal is only for the messages below.
    let fps = rate.as_f64();
    let duration = in_info.duration();

    // The clip builder's colour correction is a `ColorCorrect` effect on the same list
    // `effect_color_correct` reaches through the editing model.
    let clip = Clip::new(&input).with_color_correction(0.0, CONTRAST, 1.0);
    let timeline = Timeline::builder()
        .canvas(canvas_w, canvas_h)
        .frame_rate(rate)
        .video_track(vec![clip])
        .build()?;

    // The guard scales with the input so a long `--input` clip is not cut short and
    // then reported as a frame-count mismatch.
    let expected_frames = (duration.as_secs_f64() * fps).ceil().max(1.0) as usize;
    let cap = expected_frames
        .saturating_mul(PREVIEW_FRAME_CAP_FACTOR)
        .max(60);

    println!("previewing {canvas_w}x{canvas_h} {fps:.2}fps unpaced");
    let preview = match preview_stats(&timeline, cap) {
        Ok(s) => s,
        Err(reason) => {
            report.skip("preview leg", &reason);
            return report.finish();
        }
    };

    let output = tmp.path().join("export.mp4");
    println!("exporting the same timeline -> {}", output.display());
    if let Some(reason) = render_or_skip(timeline.clone(), &output)? {
        report.skip("export leg", &reason);
        return report.finish();
    }
    let export = match decoded_frame_means(&output) {
        Ok(means) => means,
        Err(e) => {
            report.skip("decode the export", &e.to_string());
            return report.finish();
        }
    };

    let preview_frames = preview.frame_means.len();
    let frame_gap = preview_frames.abs_diff(export.len());
    let round1 = |m: [f64; 3]| m.map(|c| (c * 10.0).round() / 10.0);
    println!(
        "preview: {preview_frames} frames, span {:.3}s, first {:?}",
        preview.last_pts.as_secs_f64(),
        preview.frame_means.first().copied().map(round1)
    );
    println!(
        "export:  {} frames, first {:?} (timeline {:.3}s)",
        export.len(),
        export.first().copied().map(round1),
        duration.as_secs_f64()
    );
    if preview_frames >= cap {
        report.skip(
            "the preview leg ran to completion",
            &format!("stopped at the {cap}-frame guard"),
        );
        return report.finish();
    }

    report.check("the preview delivered frames", preview_frames > 0);
    report.check("the export decoded to frames", !export.is_empty());
    report.check(
        "the preview rendered at the canvas size",
        preview.size == Some((canvas_w, canvas_h)),
    );
    report.check(
        "both legs produced the same number of frames",
        frame_gap <= TOL_FRAMES,
    );
    // Two frame intervals of slack: the runner may stop before presenting the last one.
    let span_floor = duration.as_secs_f64() - 2.0 / fps.max(1.0);
    report.check(
        "the preview covered the timeline's span",
        preview.last_pts.as_secs_f64() >= span_floor,
    );
    let first_spread = |m: [f64; 3]| {
        m.iter().copied().fold(f64::MIN, f64::max) - m.iter().copied().fold(f64::MAX, f64::min)
    };
    report.check(
        "both legs rendered colour, not a blank frame",
        preview
            .frame_means
            .first()
            .is_some_and(|m| first_spread(*m) >= MIN_CHROMA_SPREAD)
            && export
                .first()
                .is_some_and(|m| first_spread(*m) >= MIN_CHROMA_SPREAD),
    );
    // Frame by frame, not over the sequence: the fixture's hue sweep makes the sequence
    // average achromatic, so a per-channel divergence would cancel out in it.
    let worst = preview
        .frame_means
        .iter()
        .zip(export.iter())
        .map(|(p, e)| channel_distance(*p, *e))
        .fold(0.0f64, f64::max);
    println!("preview-vs-export worst per-frame channel distance: {worst:.2}");
    report.check(
        "every frame agrees on colour within tolerance",
        worst <= TOL_MEAN_CHANNEL,
    );

    if args.keep {
        println!("kept temp dir: {}", tmp.keep().display());
    }
    report.finish()
}
