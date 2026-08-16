//! Verify multitrack composition: place a base clip on V1 and a half-opacity
//! overlay of the same clip on V2, render the composite, then re-probe the
//! output and check its dimensions.
//!
//! This exercises the multi-layer composition derivation (`Timeline::render` ->
//! `MultiTrackComposer`) against avio alone through the public facade.
//!
//! ```bash
//! cargo run -p avio-examples --bin multitrack_export
//! cargo run -p avio-examples --bin multitrack_export -- --input clip.mp4 --keep
//! ```

use avio::{Clip, EncoderConfig, Timeline};
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

    // ── Build a two-track timeline: V1 base + V2 half-opacity overlay ──────────
    let output = tmp.path().join("multitrack.mp4");
    let timeline = Timeline::builder()
        .canvas(canvas_w, canvas_h)
        .frame_rate(fps)
        .video_track(vec![Clip::new(&input)])
        .video_track(vec![Clip::new(&input).with_opacity(0.5)])
        .build()?;
    println!(
        "rendering 2-track composite {canvas_w}x{canvas_h} {fps:.2}fps -> {}",
        output.display()
    );
    timeline.render(&output, EncoderConfig::builder().build())?;

    // ── Verify the rendered output ────────────────────────────────────────────
    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let mut report = Report::new("multitrack_export");
    report.check("output file is non-empty", size > 0);

    match avio::open(&output) {
        Ok(out_info) => {
            let out_video = out_info.video_streams();
            let out_primary = out_video.first();
            let (out_w, out_h) = out_primary.map_or((0, 0), |v| (v.width(), v.height()));
            println!(
                "output: {out_w}x{out_h} dur={:.3}s size={size}B",
                out_info.duration().as_secs_f64()
            );
            report.check("output has a video stream", out_primary.is_some());
            report.check(
                "composite dims match canvas",
                out_w == canvas_w && out_h == canvas_h,
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
