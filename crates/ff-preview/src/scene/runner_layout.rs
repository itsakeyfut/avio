//! In-place timeline layout update for [`SceneRunner`].
//!
//! Split out of `runner.rs` to keep that file within the size limit.

use std::time::Duration;

use crate::error::PreviewError;

use super::runner::SceneRunner;
use super::state::ClipVideoSource;
use super::types::{Scene, SceneSource};

/// Rebuilds a generated clip's held frame when the resolved canvas size changed: a
/// held frame is rendered *at* the canvas size, so a live resize leaves it stale
/// (a decoded file is canvas-independent, so `held_frame_dims` is `None` and it is
/// skipped). Rebuilt via [`ClipVideoSource::held`] so the synthetic advancing PTS
/// is preserved (RK-019). A `(0, 0)` canvas (no size to render into) is a no-op.
fn rebuild_held_on_resize(
    decode_buf: &mut ClipVideoSource,
    source: &SceneSource,
    in_pt: Duration,
    canvas: (u32, u32),
    fps: f64,
) {
    if canvas != (0, 0)
        && let Some(dims) = decode_buf.held_frame_dims()
        && dims != canvas
    {
        let frame = super::generated_held_frame(source, canvas.0, canvas.1, fps);
        *decode_buf = ClipVideoSource::held(frame, in_pt, fps);
    }
}

impl SceneRunner {
    /// Update clip positions in place from a new [`Scene`] without stopping the
    /// runner or replacing audio infrastructure.
    ///
    /// Only the position metadata (`timeline_start`, `timeline_end`,
    /// `in_point`, `out_point`, `xfade_dur`) of existing `ClipState` and
    /// `AudioOnlyTrack` objects is changed. The `AudioMixer` and all
    /// `AudioTrackHandle`s are reused unchanged; only the decode positions are
    /// updated by calling `seek_timeline(resume_pts)` at the end.
    ///
    /// Returns an error when the new scene is structurally incompatible with the
    /// running runner (different V1 clip count or different source paths). In
    /// that case the runner's state is untouched.
    #[allow(clippy::too_many_lines)]
    pub(super) fn update_layout_in_place(
        &mut self,
        scene: &Scene,
        resume_pts: Duration,
    ) -> Result<(), PreviewError> {
        let v_tracks = &scene.video_tracks;

        // Validate V1
        let new_v1_len = v_tracks.first().map_or(0, |t| t.placements.len());
        if new_v1_len != self.clips.len() {
            return Err(PreviewError::Ffmpeg {
                code: -1,
                message: format!(
                    "V1 clip count mismatch: runner={} timeline={new_v1_len}",
                    self.clips.len()
                ),
            });
        }
        for (i, p) in v_tracks[0].placements.iter().enumerate() {
            if p.source != self.clips[i].source {
                return Err(PreviewError::Ffmpeg {
                    code: -1,
                    message: format!(
                        "V1 clip[{i}] source mismatch: runner={:?} timeline={:?}",
                        self.clips[i].source, p.source,
                    ),
                });
            }
        }

        // Canvas size for rebuilding generated held frames on a live resize.
        let canvas = super::resolve_canvas_dims(scene);
        let fps = scene.fps.max(1.0);

        // Update V1 clip positions
        for (i, p) in v_tracks[0].placements.iter().enumerate() {
            let new_speed = p.speed;
            let old_scaled_dur = self.clips[i]
                .timeline_end
                .saturating_sub(self.clips[i].timeline_start);
            // Recover unscaled (source) duration from the stored scaled duration and old speed.
            let old_unscaled = old_scaled_dur.mul_f64(self.clips[i].speed);
            // `in_point` is pre-resolved in the Scene, so this equals the old
            // `match (in_point, out_point)` (the `_` arm reused `old_unscaled`).
            let new_unscaled = p
                .out_point
                .map_or(old_unscaled, |op| op.saturating_sub(p.in_point));
            let new_dur = if (new_speed - 1.0).abs() < 1e-9 {
                new_unscaled
            } else {
                new_unscaled.div_f64(new_speed)
            };
            self.clips[i].timeline_start = p.offset;
            self.clips[i].timeline_end = p.offset + new_dur;
            self.clips[i].in_point = p.in_point;
            self.clips[i].out_point = p.out_point;
            self.clips[i].speed = new_speed;
            self.clips[i].xfade_dur = p.xfade_dur;
            self.clips[i].xfade_kind = p.xfade_kind;
            rebuild_held_on_resize(
                &mut self.clips[i].decode_buf,
                &p.source,
                p.in_point,
                canvas,
                fps,
            );
        }

        // Update overlay layers (V2+)
        let new_overlay_count = v_tracks.len().saturating_sub(1);
        if new_overlay_count == self.overlay_layers.len() {
            for (layer_i, v_track) in v_tracks.iter().skip(1).enumerate() {
                let layer = &mut self.overlay_layers[layer_i];
                if v_track.placements.len() == layer.clips.len() {
                    for (j, p) in v_track.placements.iter().enumerate() {
                        let old_dur = layer.clips[j]
                            .timeline_end
                            .saturating_sub(layer.clips[j].timeline_start);
                        let new_dur = p
                            .out_point
                            .map_or(old_dur, |op| op.saturating_sub(p.in_point));
                        layer.clips[j].timeline_start = p.offset;
                        layer.clips[j].timeline_end = p.offset + new_dur;
                        layer.clips[j].in_point = p.in_point;
                        layer.clips[j].out_point = p.out_point;
                        rebuild_held_on_resize(
                            &mut layer.clips[j].decode_buf,
                            &p.source,
                            p.in_point,
                            canvas,
                            fps,
                        );
                    }
                }
            }
        }

        // Update audio-only tracks (A1+)
        // Collect new (timeline_start, in_point, out_point) from the scene's audio
        // tracks, matched positionally. Mismatched counts are skipped rather than
        // returning an error because audio tracks are optional.
        let new_a_positions: Vec<(Duration, Duration, Option<Duration>)> = scene
            .audio_tracks
            .iter()
            .flat_map(|track| track.placements.iter())
            .map(|p| (p.offset, p.in_point, p.out_point))
            .collect();

        if new_a_positions.len() == self.audio_only_tracks.len() {
            for (i, (new_tl_start, new_in, new_out)) in new_a_positions.iter().enumerate() {
                let old_dur = self.audio_only_tracks[i]
                    .timeline_end
                    .saturating_sub(self.audio_only_tracks[i].timeline_start);
                let new_dur = if let Some(op) = new_out {
                    op.saturating_sub(*new_in)
                } else {
                    old_dur
                };
                self.audio_only_tracks[i].timeline_start = *new_tl_start;
                self.audio_only_tracks[i].timeline_end = *new_tl_start + new_dur;
                self.audio_only_tracks[i].in_point = *new_in;
            }
        }

        // Seek everything to resume_pts
        // seek_timeline invalidates all mixer buffers, stops audio-only threads,
        // and repositions the active clip's DecodeBuffer to the correct
        // source-file PTS. Audio-only threads restart on the next frame tick
        // based on the updated timeline_start/timeline_end values.
        self.seek_timeline(resume_pts)?;

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ff_filter::{AnimatedValue, BlendMode, CompositeOp, RealtimeLayerDescriptor};
    use ff_format::Color;

    use super::super::ScenePlayer;
    use super::super::types::{Scene, ScenePlacement, SceneSource, SceneVideoTrack};
    use super::*;

    /// A minimal one-clip Solid `Scene` at `w`x`h` (a generated V1 base clip).
    fn solid_scene(w: u32, h: u32) -> Scene {
        let layer = RealtimeLayerDescriptor {
            effects: vec![],
            opacity: AnimatedValue::Static(1.0),
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            blend_mode: BlendMode::Normal,
            composite_op: CompositeOp::Over,
        };
        let placement = ScenePlacement {
            source: SceneSource::Solid(Color::rgb(20, 40, 200)),
            offset: Duration::ZERO,
            in_point: Duration::ZERO,
            out_point: Some(Duration::from_secs(2)),
            speed: 1.0,
            xfade_dur: Duration::ZERO,
            xfade_kind: None,
            opacity: 1.0,
            layer,
            fade_in: Duration::ZERO,
            fade_out: Duration::ZERO,
            volume: AnimatedValue::Static(0.0),
            pitch: 0.0,
            pan: AnimatedValue::Static(0.0),
        };
        Scene {
            fps: 30.0,
            canvas: Some((w, h)),
            lavfi_overlay: None,
            video_tracks: vec![SceneVideoTrack {
                placements: vec![placement],
            }],
            audio_tracks: vec![],
        }
    }

    #[test]
    #[ignore = "requires the color filter; run with -- --include-ignored"]
    fn layout_update_should_rebuild_generated_held_frame_on_canvas_resize() {
        // #1619: a live canvas resize of an already-open generated clip must rebuild
        // its held frame at the new size. Probe-gated (RK-002): the color filter is
        // absent on a minimal FFmpeg, so open yields no held frame and the test skips.
        let (mut runner, _handle) = match ScenePlayer::open(&solid_scene(16, 16)) {
            Ok(rh) => rh,
            Err(e) => {
                println!("skipping: open failed: {e}");
                return;
            }
        };
        if runner.clips[0].decode_buf.held_frame_dims() != Some((16, 16)) {
            println!("skipping: color filter unavailable (no held frame at open)");
            return;
        }
        // Same Solid colour, new canvas -> the source-equality check passes, so the
        // in-place fast path runs and must rebuild the stale held frame.
        runner
            .update_layout_in_place(&solid_scene(32, 32), Duration::ZERO)
            .expect("in-place update should succeed for the same source");
        assert_eq!(
            runner.clips[0].decode_buf.held_frame_dims(),
            Some((32, 32)),
            "the generated held frame must be rebuilt at the new canvas size"
        );
    }
}
