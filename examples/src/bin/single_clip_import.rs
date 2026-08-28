//! Verify importing a single clip: probe its metadata, then decode every frame,
//! and check that probe and decode agree and that frames actually come out.
//!
//! This is the most basic editing operation ("bring a source clip in"), exercised
//! against avio alone through the public facade.
//!
//! ```bash
//! cargo run -p avio-examples --bin single_clip_import            # synthetic clip
//! cargo run -p avio-examples --bin single_clip_import -- --input clip.mp4 --keep
//! ```

use std::time::Duration;

use avio_examples::{BoxResult, Report, parse_args, resolve_input};
use ff_decode::VideoDecoder;

fn main() -> BoxResult<()> {
    let args = parse_args();
    let tmp = tempfile::tempdir()?;
    let input = resolve_input(&args, tmp.path())?;

    // ── Probe metadata ────────────────────────────────────────────────────────
    let info = avio::open(&input)?;
    let video = info.video_streams();
    let primary = video.first();
    let (probe_w, probe_h, probe_fps) =
        primary.map_or((0, 0, 0.0), |v| (v.width(), v.height(), v.fps()));
    let duration = info.duration();

    // ── Decode every frame ────────────────────────────────────────────────────
    let mut decoder = VideoDecoder::open(&input).build()?;
    let dec_w = decoder.width();
    let dec_h = decoder.height();
    let mut frames: u64 = 0;
    let mut first_dims: Option<(u32, u32)> = None;
    while let Some(frame) = decoder.decode_one()? {
        if first_dims.is_none() {
            first_dims = Some((frame.width(), frame.height()));
        }
        frames += 1;
    }

    let expected = (duration.as_secs_f64() * probe_fps).round() as i64;
    let tolerance = (expected / 5).max(2);
    println!(
        "probe: {probe_w}x{probe_h} {probe_fps:.2}fps dur={:.3}s | decoded {frames} frames (expected ~{expected})",
        duration.as_secs_f64()
    );

    // ── Verify ────────────────────────────────────────────────────────────────
    let mut report = Report::new("single_clip_import");
    report.check("primary video stream present", primary.is_some());
    report.check("probe fps > 0", probe_fps > 0.0);
    report.check("probe duration > 0", duration > Duration::ZERO);
    report.check(
        "probe dims match decoder dims",
        probe_w == dec_w && probe_h == dec_h,
    );
    report.check("decoded at least one frame", frames > 0);
    report.check(
        "first frame dims match stream dims",
        first_dims == Some((dec_w, dec_h)),
    );
    report.check(
        "decoded frame count near duration*fps",
        (frames as i64 - expected).abs() <= tolerance,
    );

    if args.keep {
        let kept = tmp.keep();
        println!("kept temp dir: {}", kept.display());
    }
    report.finish()
}
