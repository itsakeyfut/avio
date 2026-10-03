//! Verify the typed effect model: add a `ColorCorrect` effect to a clip through the
//! editing model, keyframe its brightness 0 -> 0.4 over the first second, render, and
//! measure that the output brightened over time.
//!
//! This exercises the v0.18 effect surface (`Command::AddEffect` -> `ClipEffect` ->
//! `EffectKind::ColorCorrect` with a keyframed `Param`) against avio alone, through the
//! public facade a consumer has.
//!
//! The measurement is **differential**: the same timeline is rendered twice, once
//! without the effect and once with it, and the two are compared frame by frame. The
//! synthetic fixture sweeps its hue over its length, so comparing the first and last
//! frame of a single render would measure the fixture, not the effect.
//!
//! ```bash
//! cargo run -p avio-examples --bin effect_color_correct
//! cargo run -p avio-examples --bin effect_color_correct -- --input clip.mp4 --keep
//! ```

use std::time::Duration;

use avio::{
    AnimationTrack, Clip, Command, Easing, EditError, Editor, EffectId, EffectKind, Keyframe,
    Param, Timeline,
};
use avio_examples::{
    BoxResult, Report, decoded_frame_means, mean_channel, parse_args, render_or_skip, resolve_input,
};

/// Brightness the keyframe track reaches. `eq` takes brightness in `[-1, 1]`, so this
/// is a large but unsaturating step: big enough to survive a lossy encode, small enough
/// that a bright fixture does not clip to white and hide the difference.
const BRIGHTNESS: f64 = 0.4;
/// How long the brightness ramp takes. Shorter than the fixture, so the last frame sits
/// on the held end value rather than mid-ramp.
const RAMP: Duration = Duration::from_secs(1);
/// Mean-channel difference allowed between the two renders at frame 0, where the keyframe
/// is still neutral. Not zero: the effected render carries an `eq` step evaluating to
/// brightness 0 that the baseline does not have at all, and the two files are encoded
/// independently. Measured 0.00 on the synthetic fixture here (the two encodes come
/// out identical while the parameter is neutral); the margin is for content and builds
/// that are less forgiving.
const TOL_NEUTRAL: f64 = 6.0;
/// Mean-channel difference the effected render must gain by its last frame. Measured 68.34
/// on the synthetic fixture; this is a floor any content should clear, set well above
/// `TOL_NEUTRAL` so "the effect applied" cannot be satisfied by encoder noise.
const MIN_END_GAIN: f64 = 12.0;

/// The keyframed colour correction this script installs: brightness ramps from neutral
/// to [`BRIGHTNESS`], every other parameter neutral.
///
/// `temperature` and `tint` stay neutral deliberately. They are a GPU-only enrichment
/// that the CPU `eq` fallback does not reproduce, so a script using them would measure
/// a different amount on each route.
fn keyframed_color_correct() -> EffectKind {
    let brightness = AnimationTrack::new()
        .push(Keyframe::new(Duration::ZERO, 0.0, Easing::Linear))
        .push(Keyframe::new(RAMP, BRIGHTNESS, Easing::Linear));
    EffectKind::ColorCorrect {
        brightness: Param::Animated(brightness),
        contrast: Param::Const(1.0),
        saturation: Param::Const(1.0),
        temperature: Param::Const(0.0),
        tint: Param::Const(0.0),
    }
}

/// The id of the first clip on the first video track of `timeline`.
fn first_clip_id(timeline: &Timeline) -> Option<avio::ClipId> {
    timeline
        .video_tracks()
        .first()
        .and_then(|t| t.clips.first())
        .map(|c| c.id)
}

/// The effect list of that clip, after the edit.
fn first_clip_effects(timeline: &Timeline) -> &[avio::ClipEffect] {
    timeline
        .video_tracks()
        .first()
        .and_then(|t| t.clips.first())
        .map_or(&[], |c| c.effects.as_slice())
}

fn main() -> BoxResult<()> {
    let args = parse_args();
    let tmp = tempfile::tempdir()?;
    let mut report = Report::new("effect_color_correct");
    // A machine with no encoder cannot even make the fixture; that is the environment,
    // not a regression, so it skips like the render legs below.
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

    let baseline = Timeline::builder()
        .canvas(canvas_w, canvas_h)
        .frame_rate(rate)
        .video_track(vec![Clip::new(&input)])
        .build()?;
    let Some(clip) = first_clip_id(&baseline) else {
        return Err("the built timeline has no clip to edit".into());
    };

    // The editing model's own path: the document stamps the effect with an id and the
    // change lands on the undo stack, which a bare `clip.effects.push` does neither of.
    let mut editor = Editor::new(baseline.clone());
    match editor.apply(&Command::AddEffect {
        clip,
        kind: keyframed_color_correct(),
    }) {
        Ok(_) => {}
        Err(EditError::ClipNotFound { id }) => {
            return Err(format!("the timeline lost clip {id:?} between build and edit").into());
        }
        Err(e) => return Err(Box::new(e)),
    }
    let edited = editor.current().clone();

    let effects = first_clip_effects(&edited);
    report.check("the clip carries exactly one effect", effects.len() == 1);
    report.check(
        "the document stamped the effect with an id",
        effects.first().is_some_and(|e| e.id != EffectId::UNSET),
    );
    report.check(
        "the effect is a keyframed ColorCorrect",
        effects.first().is_some_and(|e| {
            matches!(
                &e.kind,
                EffectKind::ColorCorrect { brightness, .. } if !brightness.is_const()
            )
        }),
    );
    report.check("the edit is undoable", editor.can_undo());

    let base_out = tmp.path().join("baseline.mp4");
    let effect_out = tmp.path().join("color_corrected.mp4");
    println!("rendering {canvas_w}x{canvas_h} {fps:.2}fps twice (baseline, colour corrected)");
    for (label, timeline, out) in [
        ("baseline render", baseline.clone(), &base_out),
        ("colour-corrected render", edited.clone(), &effect_out),
    ] {
        if let Some(reason) = render_or_skip(timeline, out)? {
            report.skip(label, &reason);
            return report.finish();
        }
    }

    // Measure the two renders against each other.
    let (base_means, effect_means) = match (
        decoded_frame_means(&base_out),
        decoded_frame_means(&effect_out),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            report.skip("decode both renders", &e.to_string());
            return report.finish();
        }
    };
    let frames = base_means.len().min(effect_means.len());
    report.check("both renders decoded to frames", frames >= 2);
    if frames < 2 {
        return report.finish();
    }

    let gain = |i: usize| mean_channel(effect_means[i]) - mean_channel(base_means[i]);
    let (first_gain, last_gain) = (gain(0), gain(frames - 1));
    println!(
        "mean-channel gain: frame 0 = {first_gain:+.2}, frame {} = {last_gain:+.2} ({frames} frames)",
        frames - 1
    );

    report.check(
        "the two renders agree where the keyframe is neutral",
        first_gain.abs() <= TOL_NEUTRAL,
    );
    report.check(
        "the effect brightened the end of the clip",
        last_gain >= MIN_END_GAIN,
    );
    report.check(
        "the keyframed parameter animated (the gain grew)",
        last_gain > first_gain + TOL_NEUTRAL,
    );

    match avio::open(&effect_out) {
        Ok(out_info) => {
            let out_video = out_info.video_streams();
            let (out_w, out_h) = out_video
                .first()
                .map_or((0, 0), |v| (v.width(), v.height()));
            report.check(
                "output dims match canvas",
                out_w == canvas_w && out_h == canvas_h,
            );
        }
        Err(e) => {
            report.skip("re-probe the corrected output", &e.to_string());
        }
    }

    if args.keep {
        println!("kept temp dir: {}", tmp.keep().display());
    }
    report.finish()
}
