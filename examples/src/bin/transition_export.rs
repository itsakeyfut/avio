//! Verify an inter-clip transition: place two clips on V1 with a crossfade from
//! the first into the second, render, then re-probe the output.
//!
//! This exercises the transition derivation (`Clip::with_transition` ->
//! `Timeline::render` -> `FilterStep::XFade`) against avio alone.
//!
//! ```bash
//! cargo run -p avio-examples --bin transition_export
//! cargo run -p avio-examples --bin transition_export -- --input clip.mp4 --keep
//! ```

use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, XfadeTransition};
use avio_examples::{BoxResult, Report, parse_args, resolve_input};

fn main() -> BoxResult<()> {
    let args = parse_args();
    let tmp = tempfile::tempdir()?;
    let input = resolve_input(&args, tmp.path())?;

    let in_info = avio::open(&input)?;
    let in_video = in_info.video_streams();
    let Some(v) = in_video.first() else {
        return Err("input has no video stream".into());
    };
    let (canvas_w, canvas_h, fps) = (v.width(), v.height(), v.fps());

    // ── Two 1s clips back to back, with a 0.5s crossfade into the second ──────
    //
    // The clips tile the timeline: a transition does not overlap them and does not
    // shorten the result (ADR-0009). It is fed by the first clip's *handle* -- the
    // frames past its out-point -- which this input has, being longer than the 1s
    // trimmed out of it.
    let clip_len = Duration::from_secs(1);
    let xfade = Duration::from_millis(500);
    let output = tmp.path().join("transition.mp4");
    let timeline = Timeline::builder()
        .canvas(canvas_w, canvas_h)
        .frame_rate(fps)
        .video_track(vec![
            Clip::new(&input).trim(Duration::ZERO, clip_len),
            Clip::new(&input)
                .trim(Duration::ZERO, clip_len)
                .offset(clip_len)
                .with_transition(XfadeTransition::Fade, xfade),
        ])
        .build()?;
    println!(
        "rendering 2-clip xfade {canvas_w}x{canvas_h} {fps:.2}fps -> {}",
        output.display()
    );
    timeline.render(&output, EncoderConfig::builder().build())?;

    // ── Verify the rendered output ────────────────────────────────────────────
    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let mut report = Report::new("transition_export");
    report.check("output file is non-empty", size > 0);

    match avio::open(&output) {
        Ok(out_info) => {
            let out_video = out_info.video_streams();
            let out_primary = out_video.first();
            let (out_w, out_h) = out_primary.map_or((0, 0), |v| (v.width(), v.height()));
            let out_dur = out_info.duration().as_secs_f64();
            println!("output: {out_w}x{out_h} dur={out_dur:.3}s size={size}B");
            report.check("output has a video stream", out_primary.is_some());
            report.check(
                "output dims match canvas",
                out_w == canvas_w && out_h == canvas_h,
            );
            // Two 1s clips and a transition that costs the timeline nothing: ~2s,
            // the same length a hard cut would produce.
            let expected = clip_len.as_secs_f64() * 2.0;
            report.check(
                "transition preserves the hard-cut duration",
                (out_dur - expected).abs() < 0.2,
            );
        }
        Err(e) => {
            println!("  (could not re-probe output: {e})");
            report.check("output is probeable", false);
        }
    }

    if args.keep {
        println!("kept temp dir: {}", tmp.keep().display());
    }
    report.finish()
}
