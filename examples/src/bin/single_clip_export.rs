//! Verify exporting a single clip: place it on a one-clip [`Timeline`], render it,
//! then re-probe the output and check its dimensions and duration.
//!
//! This exercises the export path (decode -> compose -> encode) against avio alone
//! through the public facade.
//!
//! ```bash
//! cargo run -p avio-examples --bin single_clip_export            # synthetic clip
//! cargo run -p avio-examples --bin single_clip_export -- --input clip.mp4 --keep
//! ```

use avio::{Clip, EncoderConfig, Timeline};
use avio_examples::{BoxResult, Report, parse_args, resolve_input};

fn main() -> BoxResult<()> {
    let args = parse_args();
    let tmp = tempfile::tempdir()?;
    let input = resolve_input(&args, tmp.path())?;

    // ── Probe input for canvas dimensions + duration ──────────────────────────
    let in_info = avio::open(&input)?;
    let in_video = in_info.video_streams();
    let Some(v) = in_video.first() else {
        return Err("input has no video stream".into());
    };
    let (canvas_w, canvas_h, fps) = (v.width(), v.height(), v.fps());
    let in_duration = in_info.duration();

    // ── Build a single-clip timeline and render ───────────────────────────────
    let output = tmp.path().join("export.mp4");
    let timeline = Timeline::builder()
        .canvas(canvas_w, canvas_h)
        .frame_rate(fps)
        .video_track(vec![Clip::new(&input)])
        .build()?;
    println!(
        "rendering 1 clip {canvas_w}x{canvas_h} {fps:.2}fps -> {}",
        output.display()
    );
    timeline.render(&output, EncoderConfig::builder().build())?;

    // ── Verify the rendered output ────────────────────────────────────────────
    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let mut report = Report::new("single_clip_export");
    report.check("output file is non-empty", size > 0);

    match avio::open(&output) {
        Ok(out_info) => {
            let out_video = out_info.video_streams();
            let out_primary = out_video.first();
            let (out_w, out_h) = out_primary.map_or((0, 0), |v| (v.width(), v.height()));
            let out_duration = out_info.duration();
            let dur_tolerance = (in_duration.as_secs_f64() * 0.1).max(0.5);
            println!(
                "output: {out_w}x{out_h} dur={:.3}s size={size}B (input dur={:.3}s)",
                out_duration.as_secs_f64(),
                in_duration.as_secs_f64()
            );
            report.check("output has a video stream", out_primary.is_some());
            report.check(
                "output dims match canvas",
                out_w == canvas_w && out_h == canvas_h,
            );
            report.check(
                "output duration near input duration",
                (out_duration.as_secs_f64() - in_duration.as_secs_f64()).abs() <= dur_tolerance,
            );
        }
        Err(e) => {
            println!("  (could not re-probe output: {e})");
            report.check("output is probeable", false);
        }
    }

    if args.keep {
        let kept = tmp.keep();
        println!("kept temp dir: {}", kept.display());
    }
    report.finish()
}
