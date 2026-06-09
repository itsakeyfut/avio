//! In-place timeline layout update for [`TimelineRunner`].
//!
//! Split out of `runner.rs` to keep that file within the size limit.

use std::time::Duration;

use crate::error::PreviewError;

use super::runner::TimelineRunner;

impl TimelineRunner {
    /// Update clip positions in place from a new `Timeline` without stopping
    /// the runner or replacing audio infrastructure.
    ///
    /// Only the position metadata (`timeline_start`, `timeline_end`,
    /// `in_point`, `out_point`, `transition_dur`) of existing `ClipState` and
    /// `AudioOnlyTrack` objects is changed. The `AudioMixer` and all
    /// `AudioTrackHandle`s are reused unchanged; only the decode positions are
    /// updated by calling `seek_timeline(resume_pts)` at the end.
    ///
    /// Returns an error when the new timeline is structurally incompatible with
    /// the running runner (different V1 clip count or different source paths).
    /// In that case the runner's state is untouched.
    #[allow(clippy::too_many_lines)]
    pub(super) fn update_layout_in_place(
        &mut self,
        timeline: &ff_pipeline::timeline::Timeline,
        resume_pts: Duration,
    ) -> Result<(), PreviewError> {
        let v_tracks = timeline.video_tracks();

        // ── Validate V1 ────────────────────────────────────────────────────────
        let new_v1_len = v_tracks.first().map_or(0, Vec::len);
        if new_v1_len != self.clips.len() {
            return Err(PreviewError::Ffmpeg {
                code: -1,
                message: format!(
                    "V1 clip count mismatch: runner={} timeline={new_v1_len}",
                    self.clips.len()
                ),
            });
        }
        for (i, clip) in v_tracks[0].iter().enumerate() {
            if clip.source != self.clips[i].source {
                return Err(PreviewError::Ffmpeg {
                    code: -1,
                    message: format!(
                        "V1 clip[{i}] source mismatch: runner={} timeline={}",
                        self.clips[i].source.display(),
                        clip.source.display(),
                    ),
                });
            }
        }

        // ── Update V1 clip positions ───────────────────────────────────────────
        for (i, clip) in v_tracks[0].iter().enumerate() {
            let new_speed = clip.speed.max(0.01);
            let old_scaled_dur = self.clips[i]
                .timeline_end
                .saturating_sub(self.clips[i].timeline_start);
            // Recover unscaled (source) duration from the stored scaled duration and old speed.
            let old_unscaled = old_scaled_dur.mul_f64(self.clips[i].speed);
            let new_unscaled = match (clip.in_point, clip.out_point) {
                (Some(ip), Some(op)) => op.saturating_sub(ip),
                (None, Some(op)) => op,
                _ => old_unscaled,
            };
            let new_dur = if (new_speed - 1.0).abs() < 1e-9 {
                new_unscaled
            } else {
                new_unscaled.div_f64(new_speed)
            };
            self.clips[i].timeline_start = clip.timeline_offset;
            self.clips[i].timeline_end = clip.timeline_offset + new_dur;
            self.clips[i].in_point = clip.in_point.unwrap_or(Duration::ZERO);
            self.clips[i].out_point = clip.out_point;
            self.clips[i].speed = new_speed;
            self.clips[i].transition_dur = if clip.transition.is_some() {
                clip.transition_duration
            } else {
                Duration::ZERO
            };
        }

        // ── Update overlay layers (V2+) ────────────────────────────────────────
        let new_overlay_count = v_tracks.len().saturating_sub(1);
        if new_overlay_count == self.overlay_layers.len() {
            for (layer_i, v_track) in v_tracks.iter().skip(1).enumerate() {
                let layer = &mut self.overlay_layers[layer_i];
                if v_track.len() == layer.clips.len() {
                    for (j, clip) in v_track.iter().enumerate() {
                        let old_dur = layer.clips[j]
                            .timeline_end
                            .saturating_sub(layer.clips[j].timeline_start);
                        let new_dur = match (clip.in_point, clip.out_point) {
                            (Some(ip), Some(op)) => op.saturating_sub(ip),
                            (None, Some(op)) => op,
                            _ => old_dur,
                        };
                        layer.clips[j].timeline_start = clip.timeline_offset;
                        layer.clips[j].timeline_end = clip.timeline_offset + new_dur;
                        layer.clips[j].in_point = clip.in_point.unwrap_or(Duration::ZERO);
                        layer.clips[j].out_point = clip.out_point;
                    }
                }
            }
        }

        // ── Update audio-only tracks (A1+) ─────────────────────────────────────
        // Collect new (timeline_start, in_point, out_point) from the timeline's
        // audio tracks, matched positionally. Mismatched counts are skipped
        // rather than returning an error because audio tracks are optional.
        let new_a_positions: Vec<(Duration, Duration, Option<Duration>)> = timeline
            .audio_tracks()
            .iter()
            .flat_map(|track| track.iter())
            .map(|clip| {
                (
                    clip.timeline_offset,
                    clip.in_point.unwrap_or(Duration::ZERO),
                    clip.out_point,
                )
            })
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

        // ── Seek everything to resume_pts ──────────────────────────────────────
        // seek_timeline invalidates all mixer buffers, stops audio-only threads,
        // and repositions the active clip's DecodeBuffer to the correct
        // source-file PTS. Audio-only threads restart on the next frame tick
        // based on the updated timeline_start/timeline_end values.
        self.seek_timeline(resume_pts)?;

        Ok(())
    }
}
