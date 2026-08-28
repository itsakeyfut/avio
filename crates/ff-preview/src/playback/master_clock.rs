//! Internal A/V sync reference clock.
//!
//! [`MasterClock`] is the crate-internal reference clock used by the
//! `PlayerRunner` pacing loop. It has two variants:
//!
//! - `Audio`: driven by consumed audio samples ÷ sample-rate.
//! - `System`: driven by [`std::time::Instant`] (video-only files).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// MasterClock

/// Reference clock for the A/V sync loop in [`PreviewPlayer::run`].
///
/// - `Audio`: driven by consumed audio samples ÷ `sample_rate`.
/// - `System`: driven by [`std::time::Instant`] (video-only files).
pub(crate) enum MasterClock {
    Audio {
        samples_consumed: Arc<AtomicU64>,
        sample_rate: u32,
        /// Playback rate multiplier. 1.0 = real-time.
        rate: f64,
        /// Snapshot of `samples_consumed` at the last rate-change or seek.
        samples_base: u64,
        /// Media PTS at the last rate-change or seek.
        pts_base: Duration,
        /// Wall-clock fallback activated after the first presented frame when no
        /// audio consumer has called `pop_audio_samples()`. Tuple: `(wall start, base PTS)`.
        ///
        /// When `Some`, `current_pts()` returns `base_pts + elapsed` instead of
        /// `Duration::ZERO`, so video pacing runs at real time even without a cpal
        /// consumer. If `samples_consumed` becomes non-zero later (a consumer
        /// connects mid-playback), `current_pts()` automatically switches to the
        /// audio-clock path with no additional coordination.
        fallback: Option<(Instant, Duration)>,
    },
    System {
        started_at: Instant,
        base_pts: Duration,
        /// Playback rate multiplier. 1.0 = real-time.
        rate: f64,
    },
}

impl MasterClock {
    /// Current master clock position.
    ///
    /// For `Audio`: returns the maximum of the sample-based clock and the
    /// wall-clock fallback (when set). Taking the maximum ensures that the
    /// clock continues advancing at wall-clock rate after the audio ring
    /// buffer drains (audio track ends before video), while also allowing
    /// a late-connecting cpal consumer to drive the clock forward once it
    /// overtakes the initial fallback.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn current_pts(&self) -> Duration {
        match self {
            Self::Audio {
                samples_consumed,
                sample_rate,
                rate,
                samples_base,
                pts_base,
                fallback,
            } => {
                let s = samples_consumed.load(Ordering::Relaxed);
                let delta = s.saturating_sub(*samples_base);
                let sample_pts = if delta > 0 || !pts_base.is_zero() {
                    Some(
                        *pts_base
                            + Duration::from_secs_f64(
                                delta as f64 / f64::from(*sample_rate) * *rate,
                            ),
                    )
                } else {
                    None // before first sample consumed (no sync yet)
                };
                let fallback_pts = fallback
                    .as_ref()
                    .map(|(started_at, base_pts)| *base_pts + started_at.elapsed().mul_f64(*rate));
                match (sample_pts, fallback_pts) {
                    // Both present: use whichever is further ahead.
                    // - During normal playback the sample clock is ahead → sample wins.
                    // - After audio EOF (samples frozen) the wall-clock fallback
                    //   overtakes → fallback wins.
                    (Some(sp), Some(fp)) => sp.max(fp),
                    (Some(sp), None) => sp,
                    (None, Some(fp)) => fp,
                    (None, None) => Duration::ZERO,
                }
            }
            Self::System {
                started_at,
                base_pts,
                rate,
            } => *base_pts + started_at.elapsed().mul_f64(*rate),
        }
    }

    /// Whether A/V sync should be applied for the current frame.
    ///
    /// - `System`: always `true` — wall clock drives FPS pacing.
    /// - `Audio`: `true` once any of the following holds:
    ///   - `samples_consumed > 0` (a cpal consumer has called `pop_audio_samples`), or
    ///   - `fallback.is_some()` (the wall-clock fallback was armed after the first frame).
    ///
    ///   Returns `false` only in the brief window between `run()` starting and the
    ///   first frame being presented — this prevents an indefinite sleep before any
    ///   clock reference is available.
    pub(crate) fn should_sync(&self) -> bool {
        match self {
            Self::System { .. } => true,
            Self::Audio {
                samples_consumed,
                samples_base,
                pts_base,
                fallback,
                ..
            } => {
                let s = samples_consumed.load(Ordering::Relaxed);
                s > *samples_base || !pts_base.is_zero() || fallback.is_some()
            }
        }
    }

    /// Activate the wall-clock fallback at `base_pts` if no audio samples have
    /// been consumed yet and the fallback has not already been armed.
    ///
    /// Called by [`PlayerRunner::run`] immediately after the first
    /// `present_frame()` call. Once armed, `should_sync()` returns `true` and
    /// `current_pts()` advances in real time even when no cpal consumer is
    /// connected.
    ///
    /// Idempotent: subsequent calls are no-ops.  If `samples_consumed` becomes
    /// non-zero (a consumer connects mid-playback), `current_pts()` automatically
    /// switches to the audio-clock path without any additional coordination.
    ///
    /// No-op for [`MasterClock::System`].
    pub(crate) fn activate_fallback_if_no_audio(&mut self, base_pts: Duration) {
        if let Self::Audio {
            samples_consumed,
            samples_base,
            fallback,
            ..
        } = self
            && samples_consumed.load(Ordering::Relaxed) == *samples_base
            && fallback.is_none()
        {
            *fallback = Some((Instant::now(), base_pts));
        }
    }

    /// Re-arm the wall-clock fallback at `base_pts`, even when
    /// `samples_consumed > 0`.
    ///
    /// Unlike [`activate_fallback_if_no_audio`](Self::activate_fallback_if_no_audio),
    /// this method activates unconditionally and is intended to be called by
    /// the pacing loop when it detects that audio has gone silent (audio track
    /// ended before video). After re-arming, [`current_pts`](Self::current_pts)
    /// returns the `max` of the frozen sample position and the advancing
    /// wall-clock, so video continues at its native frame rate.
    ///
    /// No-op for [`MasterClock::System`].
    pub(crate) fn rearm_fallback_at(&mut self, base_pts: Duration) {
        if let Self::Audio { fallback, .. } = self {
            *fallback = Some((Instant::now(), base_pts));
        }
    }

    /// Update the playback rate multiplier.
    ///
    /// Re-baselines the clock so that `current_pts()` does not jump at the
    /// moment of the rate change.  Values ≤ 0.0 are ignored.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn set_rate(&mut self, new_rate: f64) {
        if new_rate <= 0.0 {
            return;
        }
        match self {
            Self::Audio {
                samples_consumed,
                sample_rate,
                rate,
                pts_base,
                samples_base,
                fallback,
            } => {
                // Re-baseline at current pts so the clock doesn't jump.
                let s = samples_consumed.load(Ordering::Relaxed);
                let delta = s.saturating_sub(*samples_base);
                let current = *pts_base
                    + Duration::from_secs_f64(delta as f64 / f64::from(*sample_rate) * *rate);
                *samples_base = s;
                *pts_base = current;
                *rate = new_rate;
                if let Some((started_at, base)) = fallback.as_mut() {
                    *base = current;
                    *started_at = Instant::now();
                }
            }
            Self::System {
                started_at,
                base_pts,
                rate,
            } => {
                let current = *base_pts + started_at.elapsed().mul_f64(*rate);
                *base_pts = current;
                *started_at = Instant::now();
                *rate = new_rate;
            }
        }
    }

    /// Current value of the audio sample counter, or `0` for a `System` clock.
    ///
    /// Used by the pacing loop to detect stalls: if this value stops
    /// advancing for several consecutive frames while `> 0`, the audio track
    /// has ended and `rearm_fallback_at` should be called.
    pub(crate) fn audio_samples_snapshot(&self) -> u64 {
        if let Self::Audio {
            samples_consumed, ..
        } = self
        {
            samples_consumed.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Reset the clock to start ticking from `base` right now.
    ///
    /// For [`MasterClock::System`]: re-anchors `started_at` and sets `base_pts`.
    ///
    /// For [`MasterClock::Audio`]: if the wall-clock fallback is active (i.e. no
    /// audio consumer is present), re-anchors the fallback at `(Instant::now(), base)`
    /// so that post-seek pacing starts from the correct position. If the fallback
    /// is not yet armed (pre-first-frame) or if `samples_consumed > 0` (audio
    /// consumer active), this is a no-op — the seek position is reflected in the
    /// audio buffer restart performed by `restart_audio_from`.
    pub(crate) fn reset(&mut self, base: Duration) {
        match self {
            Self::System {
                started_at,
                base_pts,
                ..
            } => {
                *started_at = Instant::now();
                *base_pts = base;
            }
            Self::Audio {
                samples_consumed,
                samples_base,
                pts_base,
                fallback,
                ..
            } => {
                let s = samples_consumed.load(Ordering::Relaxed);
                *samples_base = s;
                *pts_base = base;
                if fallback.is_some() {
                    *fallback = Some((Instant::now(), base));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn master_clock_system_should_advance_from_base_pts() {
        let clock = MasterClock::System {
            started_at: Instant::now(),
            base_pts: Duration::from_secs(5),
            rate: 1.0,
        };
        let pts = clock.current_pts();
        assert!(
            pts >= Duration::from_secs(5),
            "pts must be >= base_pts; got {pts:?}"
        );
        assert!(
            pts < Duration::from_secs(6),
            "pts must not advance 1 s in a unit test; got {pts:?}"
        );
        assert!(clock.should_sync(), "System clock must always sync");
    }

    #[test]
    fn master_clock_system_reset_should_update_base_and_time_reference() {
        let mut clock = MasterClock::System {
            started_at: Instant::now() - Duration::from_secs(10),
            base_pts: Duration::ZERO,
            rate: 1.0,
        };
        assert!(
            clock.current_pts() >= Duration::from_secs(9),
            "clock should show ~10 s before reset"
        );
        clock.reset(Duration::from_secs(5));
        let pts = clock.current_pts();
        assert!(
            pts >= Duration::from_secs(5),
            "pts must be >= new base after reset; got {pts:?}"
        );
        assert!(
            pts < Duration::from_secs(6),
            "pts must not advance 1 s in a unit test after reset; got {pts:?}"
        );
    }

    #[test]
    fn master_clock_audio_should_not_sync_before_first_sample() {
        let clock = MasterClock::Audio {
            samples_consumed: Arc::new(AtomicU64::new(0)),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        assert!(
            !clock.should_sync(),
            "audio clock must not sync before any samples are consumed and before fallback is armed"
        );
        assert_eq!(
            clock.current_pts(),
            Duration::ZERO,
            "audio clock PTS must be zero before any samples and before fallback is armed"
        );
    }

    #[test]
    fn master_clock_audio_should_sync_and_report_pts_after_samples_consumed() {
        let consumed = Arc::new(AtomicU64::new(48_000));
        let clock = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        assert!(
            clock.should_sync(),
            "audio clock must sync when samples > 0"
        );
        assert_eq!(
            clock.current_pts(),
            Duration::from_secs(1),
            "48000 samples at 48000 Hz must equal 1 second"
        );
    }

    #[test]
    fn master_clock_audio_should_sync_after_fallback_activated() {
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::new(AtomicU64::new(0)),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        assert!(
            !clock.should_sync(),
            "must not sync before fallback is armed"
        );
        clock.activate_fallback_if_no_audio(Duration::from_secs(1));
        assert!(
            clock.should_sync(),
            "must sync after fallback is activated even when samples_consumed == 0"
        );
    }

    #[test]
    fn master_clock_audio_fallback_current_pts_should_advance_from_base_pts() {
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::new(AtomicU64::new(0)),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        let base = Duration::from_secs(5);
        clock.activate_fallback_if_no_audio(base);
        let pts = clock.current_pts();
        assert!(
            pts >= base,
            "fallback current_pts must be >= base_pts; got {pts:?}"
        );
        assert!(
            pts < base + Duration::from_secs(1),
            "fallback must not advance 1 s in a unit test; got {pts:?}"
        );
    }

    #[test]
    fn master_clock_audio_max_of_sample_and_fallback_should_prefer_further_ahead() {
        let consumed = Arc::new(AtomicU64::new(0));
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        clock.activate_fallback_if_no_audio(Duration::from_secs(2));
        assert!(clock.should_sync(), "fallback must enable sync");
        // Audio consumer processes 1 s of audio.
        consumed.store(48_000, Ordering::Relaxed);
        // sample_pts=1 s, fallback_pts≈2 s → max returns ≈2 s.
        let pts = clock.current_pts();
        assert!(
            pts >= Duration::from_secs(2),
            "max() must return fallback when fallback is further ahead; got {pts:?}"
        );
        assert!(
            pts < Duration::from_secs(3),
            "fallback must not be wildly ahead of 2 s; got {pts:?}"
        );
    }

    #[test]
    fn master_clock_audio_activate_fallback_should_be_idempotent() {
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::new(AtomicU64::new(0)),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        clock.activate_fallback_if_no_audio(Duration::from_secs(1));
        let pts1 = clock.current_pts();
        thread::sleep(Duration::from_millis(5));
        // Second call with a different base must be ignored.
        clock.activate_fallback_if_no_audio(Duration::from_secs(100));
        let pts2 = clock.current_pts();
        assert!(
            pts2 > pts1,
            "clock must keep advancing from the first base after second activate; \
             pts1={pts1:?} pts2={pts2:?}"
        );
        assert!(
            pts2 < Duration::from_secs(5),
            "second activate must not reset clock to base=100 s; pts2={pts2:?}"
        );
    }

    #[test]
    fn master_clock_audio_reset_should_update_fallback_base_pts() {
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::new(AtomicU64::new(0)),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        clock.activate_fallback_if_no_audio(Duration::from_secs(5));
        // Simulate a seek to 10 s.
        clock.reset(Duration::from_secs(10));
        let pts = clock.current_pts();
        assert!(
            pts >= Duration::from_secs(10),
            "after reset, fallback must advance from the new base_pts; got {pts:?}"
        );
        assert!(
            pts < Duration::from_secs(11),
            "fallback must not advance 1 s in a unit test after reset; got {pts:?}"
        );
    }

    #[test]
    fn master_clock_audio_reset_should_not_arm_fallback_if_not_yet_active() {
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::new(AtomicU64::new(0)),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        // reset() before the first frame must not arm the fallback.
        clock.reset(Duration::ZERO);
        assert!(
            !clock.should_sync(),
            "reset() before activate_fallback_if_no_audio must not arm the fallback"
        );
        assert_eq!(
            clock.current_pts(),
            Duration::ZERO,
            "PTS must remain ZERO when fallback is not yet armed"
        );
    }

    #[test]
    fn master_clock_audio_rearm_should_advance_past_frozen_sample_pts() {
        let frozen_frames: u64 = (45_222 * 48_000) / 1_000; // frames for 45.222 s
        let consumed = Arc::new(AtomicU64::new(frozen_frames));
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        let frozen_pts = Duration::from_secs_f64(frozen_frames as f64 / 48_000.0);
        // Before rearm: clock is frozen at the audio EOF position.
        assert_eq!(
            clock.current_pts(),
            frozen_pts,
            "clock must be frozen at audio EOF position before rearm"
        );
        // Re-arm at the frozen position.
        clock.rearm_fallback_at(frozen_pts);
        thread::sleep(Duration::from_millis(10));
        // After rearm: clock must have advanced past the frozen value.
        let pts_after = clock.current_pts();
        assert!(
            pts_after > frozen_pts,
            "clock must advance past frozen sample_pts after rearm; \
             frozen={frozen_pts:?} after={pts_after:?}"
        );
        assert!(
            pts_after < frozen_pts + Duration::from_secs(1),
            "clock must not advance 1 s in a unit test after rearm; got {pts_after:?}"
        );
    }

    #[test]
    fn master_clock_audio_rearm_should_be_noop_for_system_clock() {
        let mut clock = MasterClock::System {
            started_at: Instant::now(),
            base_pts: Duration::ZERO,
            rate: 1.0,
        };
        // Must not panic and System behaviour must be unchanged.
        clock.rearm_fallback_at(Duration::from_secs(99));
        assert!(
            clock.should_sync(),
            "System clock must always sync after rearm_fallback_at"
        );
    }

    #[test]
    fn audio_samples_snapshot_should_return_current_counter_for_audio_clock() {
        let consumed = Arc::new(AtomicU64::new(12_345));
        let clock = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        assert_eq!(
            clock.audio_samples_snapshot(),
            12_345,
            "audio_samples_snapshot must reflect the current AtomicU64 value"
        );
    }

    #[test]
    fn audio_samples_snapshot_should_return_zero_for_system_clock() {
        let clock = MasterClock::System {
            started_at: Instant::now(),
            base_pts: Duration::ZERO,
            rate: 1.0,
        };
        assert_eq!(
            clock.audio_samples_snapshot(),
            0,
            "audio_samples_snapshot must return 0 for System clock"
        );
    }

    #[test]
    fn master_clock_audio_current_pts_should_advance_one_second_after_48k_frames() {
        let consumed = Arc::new(AtomicU64::new(48_000));
        let clock = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        assert_eq!(
            clock.current_pts(),
            Duration::from_secs(1),
            "48 000 consumed frames / 48 000 Hz must equal exactly 1.0 s"
        );
    }

    #[test]
    fn master_clock_audio_native_rate_mismatch_demonstrates_bug() {
        let consumed = Arc::new(AtomicU64::new(48_000));
        let clock_wrong = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 44_100, // wrong: source native rate, not decoder output rate
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };
        let pts_wrong = clock_wrong.current_pts();
        // 48 000 / 44 100 ≈ 1.0884 s — NOT 1.0 s
        assert!(
            pts_wrong > Duration::from_secs(1),
            "using native rate produces a clock that runs too fast; got {pts_wrong:?}"
        );
        assert!(
            pts_wrong < Duration::from_millis(1_100),
            "drift must be bounded to ~8.8 %; got {pts_wrong:?}"
        );
    }

    #[test]
    fn master_clock_system_activate_fallback_should_be_noop() {
        let mut clock = MasterClock::System {
            started_at: Instant::now(),
            base_pts: Duration::ZERO,
            rate: 1.0,
        };
        // Must not panic and must not change System behaviour.
        clock.activate_fallback_if_no_audio(Duration::from_secs(99));
        assert!(
            clock.should_sync(),
            "System clock must always sync regardless of activate_fallback_if_no_audio"
        );
    }

    #[test]
    fn set_rate_should_scale_audio_clock_pts() {
        let consumed = Arc::new(AtomicU64::new(48_000));
        let mut clock = MasterClock::Audio {
            samples_consumed: Arc::clone(&consumed),
            sample_rate: 48_000,
            rate: 1.0,
            samples_base: 0,
            pts_base: Duration::ZERO,
            fallback: None,
        };

        assert_eq!(
            clock.current_pts(),
            Duration::from_secs(1),
            "before set_rate clock must report 1 s"
        );

        clock.set_rate(2.0);
        consumed.fetch_add(48_000, Ordering::Relaxed); // total = 96 000

        let pts = clock.current_pts();
        let expected = Duration::from_secs(3);
        let tolerance = Duration::from_millis(1);
        assert!(
            pts >= expected.saturating_sub(tolerance) && pts <= expected + tolerance,
            "1 s at 1× + 1 real-s at 2× must equal ≈3 s; got {pts:?}"
        );
    }

    #[test]
    fn set_rate_system_clock_should_scale_elapsed() {
        let mut clock = MasterClock::System {
            started_at: Instant::now(),
            base_pts: Duration::ZERO,
            rate: 1.0,
        };

        thread::sleep(Duration::from_millis(10));
        let pts_before_rate_change = clock.current_pts();
        assert!(
            pts_before_rate_change >= Duration::from_millis(5),
            "clock must have advanced ~10 ms before rate change; got {pts_before_rate_change:?}"
        );

        clock.set_rate(2.0);
        let pts_at_rate_change = clock.current_pts();
        assert!(
            pts_at_rate_change >= pts_before_rate_change,
            "clock must not go backward on set_rate; before={pts_before_rate_change:?} at={pts_at_rate_change:?}"
        );

        thread::sleep(Duration::from_millis(10));
        let pts_after = clock.current_pts();

        let media_elapsed = pts_after.saturating_sub(pts_at_rate_change);
        assert!(
            media_elapsed >= Duration::from_millis(15),
            "2× rate: 10 ms wall time must produce ≥15 ms media time; got media_elapsed={media_elapsed:?}"
        );
        assert!(
            pts_after > Duration::from_millis(20),
            "total PTS after ~10ms at 1× + ~10ms at 2× must be >20ms; got {pts_after:?}"
        );
    }
}
