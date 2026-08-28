//! Playback clock types for ff-preview.
//!
//! [`PlaybackClock`] is the public wall-clock API for callers that need
//! independent timing queries. The crate-internal A/V sync reference clock is
//! [`MasterClock`](super::master_clock::MasterClock), defined in
//! `playback/master_clock.rs`.

use std::time::{Duration, Instant};

// ClockState

/// Internal state machine for `PlaybackClock`.
///
/// Transitions:
/// - `Stopped  → Running`:  `start()`
/// - `Running  → Paused`:   `pause()`
/// - `Running  → Stopped`:  `stop()`
/// - `Paused   → Running`:  `resume()`
/// - `Paused   → Stopped`:  `stop()`
/// - `Running  → Running`:  `start()` is a no-op
/// - `Paused   → Paused`:   `pause()` is a no-op
enum ClockState {
    Stopped,
    Running { started_at: Instant, base: Duration },
    Paused { frozen_at: Duration },
}

// PlaybackClock

/// A monotonic clock that tracks elapsed playback time.
///
/// The clock supports start, stop, pause, resume, and playback-rate scaling.
/// It is used by `PreviewPlayer` internally to drive frame presentation timing
/// and A/V synchronisation. Callers may also query it directly.
///
/// `PlaybackClock` is a value type — it is not `Arc<Mutex<...>>` internally.
/// When multi-thread access is required, wrap it in a `Mutex`.
///
/// # Usage
///
/// ```ignore
/// let mut clock = PlaybackClock::new();
/// clock.start();
/// let pts = clock.current_pts();
/// clock.pause();
/// // current_pts() is now frozen
/// clock.resume();
/// // current_pts() continues advancing from the frozen point
/// clock.set_rate(2.0);          // fast-forward at 2×
/// clock.set_position(Duration::from_secs(30)); // seek to 30 s
/// ```
pub struct PlaybackClock {
    state: ClockState,
    /// Playback rate multiplier. 1.0 = real-time.
    rate: f64,
    /// Pending seek position. Applied as the `base` when `start()` is called
    /// from the `Stopped` state. Cleared by `stop()`.
    seek_offset: Duration,
}

impl PlaybackClock {
    /// Create a new clock in the `Stopped` state with a rate of 1.0.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: ClockState::Stopped,
            rate: 1.0,
            seek_offset: Duration::ZERO,
        }
    }

    /// Start the clock from the current position.
    ///
    /// - If the clock is `Stopped`, it starts from the position last set by
    ///   [`set_position`](Self::set_position), or `Duration::ZERO` if no seek
    ///   has been performed.
    /// - If the clock is `Paused`, it starts from the frozen position.
    /// - If the clock is already `Running`, this is a no-op.
    pub fn start(&mut self) {
        let base = match &self.state {
            ClockState::Running { .. } => return,
            ClockState::Stopped => self.seek_offset,
            ClockState::Paused { frozen_at } => *frozen_at,
        };
        self.state = ClockState::Running {
            started_at: Instant::now(),
            base,
        };
    }

    /// Stop the clock and reset the position to `Duration::ZERO`.
    ///
    /// `current_time()` and `current_pts()` will return `Duration::ZERO`
    /// until `start()` or `set_position()` is called again.
    pub fn stop(&mut self) {
        self.state = ClockState::Stopped;
        self.seek_offset = Duration::ZERO;
    }

    /// Pause the clock at the current position.
    ///
    /// `current_time()` and `current_pts()` are frozen until
    /// [`resume`](Self::resume) is called. If already `Paused` or `Stopped`, no-op.
    pub fn pause(&mut self) {
        if let ClockState::Running { started_at, base } = &self.state {
            let elapsed = started_at.elapsed().mul_f64(self.rate);
            self.state = ClockState::Paused {
                frozen_at: *base + elapsed,
            };
        }
    }

    /// Resume from a paused position. No-op if not paused.
    pub fn resume(&mut self) {
        if let ClockState::Paused { frozen_at } = self.state {
            self.state = ClockState::Running {
                started_at: Instant::now(),
                base: frozen_at,
            };
        }
    }

    /// Current wall-clock elapsed time since start (affected by rate).
    ///
    /// Equivalent to [`current_pts`](Self::current_pts) for clocks that
    /// start at zero; use `current_pts()` when a seek offset has been set.
    #[must_use]
    pub fn current_time(&self) -> Duration {
        match &self.state {
            ClockState::Stopped => Duration::ZERO,
            ClockState::Paused { frozen_at } => *frozen_at,
            ClockState::Running { started_at, base } => {
                *base + started_at.elapsed().mul_f64(self.rate)
            }
        }
    }

    /// Current presentation timestamp (elapsed time since position zero).
    ///
    /// Identical to `current_time()` when the clock was started from zero.
    /// When a `set_position(t)` was called before `start()`, the clock
    /// advances from `t` and this method returns values ≥ `t`.
    #[must_use]
    pub fn current_pts(&self) -> Duration {
        match &self.state {
            ClockState::Stopped => self.seek_offset,
            _ => self.current_time(),
        }
    }

    /// Returns `true` if the clock is actively advancing.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.state, ClockState::Running { .. })
    }

    /// Set the playback rate. Values ≤ 0.0 are ignored (rate stays unchanged).
    ///
    /// When the clock is `Running`, the current position is re-anchored at
    /// `Instant::now()` so that `current_time()` does not jump.
    pub fn set_rate(&mut self, rate: f64) {
        if rate <= 0.0 {
            return;
        }
        if let ClockState::Running { started_at, base } = &mut self.state {
            // Re-anchor to now so the position does not jump on rate change.
            let elapsed = started_at.elapsed().mul_f64(self.rate);
            *base += elapsed;
            *started_at = Instant::now();
        }
        self.rate = rate;
    }

    /// Current playback rate (default: 1.0).
    #[must_use]
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Jump to an arbitrary position in media time.
    ///
    /// - `Running`: the clock continues advancing from `pts` immediately.
    /// - `Paused`: the frozen position is updated to `pts`.
    /// - `Stopped`: `pts` is stored and applied as the starting position when
    ///   [`start`](Self::start) is next called.
    ///
    /// After `set_position(t)` + `start()`, [`current_pts`](Self::current_pts)
    /// will immediately return values ≥ `t`.
    pub fn set_position(&mut self, pts: Duration) {
        // seek_offset is always updated so current_pts() is consistent for all states.
        self.seek_offset = pts;
        if matches!(self.state, ClockState::Running { .. }) {
            // Re-anchor the running base at the new position.
            self.state = ClockState::Running {
                started_at: Instant::now(),
                base: pts,
            };
        } else if matches!(self.state, ClockState::Paused { .. }) {
            self.state = ClockState::Paused { frozen_at: pts };
        }
        // Stopped: seek_offset is set above; start() will use it as the initial base.
    }
}

impl Default for PlaybackClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn clock_stopped_should_return_zero() {
        // Newly created clock returns zero.
        let clock = PlaybackClock::new();
        assert_eq!(clock.current_time(), Duration::ZERO);

        // Clock returns zero after stop.
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(5));
        clock.stop();
        assert_eq!(
            clock.current_time(),
            Duration::ZERO,
            "current_time() must be ZERO after stop()"
        );
    }

    #[test]
    fn clock_paused_should_freeze_at_pause_time() {
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(10));
        clock.pause();

        let t1 = clock.current_time();
        thread::sleep(Duration::from_millis(10));
        let t2 = clock.current_time();

        assert_eq!(t1, t2, "current_time() must not advance while paused");
        assert!(
            !clock.is_running(),
            "clock must not report running while paused"
        );
    }

    #[test]
    fn clock_resumed_should_continue_from_pause() {
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(10));
        clock.pause();
        let t_paused = clock.current_time();

        // Wait while paused — time must not advance.
        thread::sleep(Duration::from_millis(10));
        assert_eq!(clock.current_time(), t_paused);

        clock.resume();
        assert!(clock.is_running());
        thread::sleep(Duration::from_millis(10));

        let t_after = clock.current_time();
        assert!(
            t_after > t_paused,
            "current_time() must advance after resume(); paused={t_paused:?} after={t_after:?}"
        );
    }

    #[test]
    fn clock_start_should_be_noop_when_already_running() {
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(10));
        let t_before = clock.current_time();

        // Second start() should not reset the clock.
        clock.start();
        let t_after = clock.current_time();

        assert!(
            t_after >= t_before,
            "second start() must not reset the clock; before={t_before:?} after={t_after:?}"
        );
    }

    #[test]
    fn clock_resume_should_be_noop_when_not_paused() {
        // resume() on a stopped clock: stays stopped.
        let mut clock = PlaybackClock::new();
        clock.resume();
        assert!(!clock.is_running());
        assert_eq!(clock.current_time(), Duration::ZERO);

        // resume() on a running clock: no effect.
        clock.start();
        thread::sleep(Duration::from_millis(5));
        let t = clock.current_time();
        clock.resume(); // no-op
        assert!(clock.is_running());
        assert!(clock.current_time() >= t);
    }

    #[test]
    fn clock_default_should_equal_new() {
        let a = PlaybackClock::new();
        let b = PlaybackClock::default();
        assert_eq!(a.current_time(), b.current_time());
        assert_eq!(a.is_running(), b.is_running());
    }

    #[test]
    fn set_rate_should_reject_non_positive_values() {
        let mut clock = PlaybackClock::new();

        clock.set_rate(0.0);
        assert!(
            (clock.rate() - 1.0).abs() < f64::EPSILON,
            "rate must remain 1.0 after set_rate(0.0)"
        );

        clock.set_rate(-1.0);
        assert!(
            (clock.rate() - 1.0).abs() < f64::EPSILON,
            "rate must remain 1.0 after set_rate(-1.0)"
        );
    }

    #[test]
    fn set_rate_should_update_rate_when_stopped_or_paused() {
        // Stopped → rate updates.
        let mut clock = PlaybackClock::new();
        clock.set_rate(0.5);
        assert!((clock.rate() - 0.5).abs() < f64::EPSILON);

        // Paused → rate updates without resuming.
        let mut clock = PlaybackClock::new();
        clock.start();
        clock.pause();
        clock.set_rate(2.0);
        assert!((clock.rate() - 2.0).abs() < f64::EPSILON);
        assert!(
            !clock.is_running(),
            "clock must remain paused after set_rate"
        );
    }

    #[test]
    fn set_rate_running_should_not_jump_current_time() {
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(10));
        let before = clock.current_time();
        clock.set_rate(2.0);
        let after = clock.current_time();

        assert!(
            after >= before,
            "current_time() must not go backward on set_rate; before={before:?} after={after:?}"
        );
        assert!(
            after - before < Duration::from_millis(20),
            "current_time() must not jump forward on set_rate; before={before:?} after={after:?}"
        );
        assert!((clock.rate() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    #[ignore = "performance thresholds are environment-dependent; run explicitly with -- --include-ignored"]
    fn rate_two_x_should_advance_at_double_speed() {
        let mut clock = PlaybackClock::new();
        clock.set_rate(2.0);
        clock.start();
        thread::sleep(Duration::from_millis(50));
        let elapsed = clock.current_time();

        // At 2×, 50 ms wall time should produce ≥80 ms of media time.
        assert!(
            elapsed >= Duration::from_millis(80),
            "2× rate: expected ≥80 ms after 50 ms wall time, got {elapsed:?}"
        );
    }

    #[test]
    fn set_position_should_shift_pts_by_seek_offset() {
        let seek_target = Duration::from_secs(30);

        // Stopped: current_pts() returns the offset immediately.
        let mut clock = PlaybackClock::new();
        clock.set_position(seek_target);
        assert_eq!(
            clock.current_pts(),
            seek_target,
            "current_pts() must reflect seek_offset when stopped"
        );

        // start() must begin from the seek position.
        clock.start();
        let pts = clock.current_pts();
        assert!(
            pts >= seek_target,
            "current_pts() must be ≥ seek target after start(); target={seek_target:?} pts={pts:?}"
        );
        assert!(
            clock.is_running(),
            "clock must be running after set_position + start()"
        );
    }

    #[test]
    fn set_position_while_paused_should_update_frozen_time() {
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(5));
        clock.pause();

        let seek_target = Duration::from_secs(10);
        clock.set_position(seek_target);

        let pts = clock.current_pts();
        assert_eq!(
            pts, seek_target,
            "frozen time must update to seek target; expected={seek_target:?} got={pts:?}"
        );
        assert!(
            !clock.is_running(),
            "clock must remain paused after set_position"
        );

        // resume() must continue advancing from the new position.
        clock.resume();
        thread::sleep(Duration::from_millis(5));
        let pts_after = clock.current_pts();
        assert!(
            pts_after > seek_target,
            "current_pts() must advance past seek target after resume(); target={seek_target:?} after={pts_after:?}"
        );
    }

    #[test]
    fn set_position_while_running_should_continue_from_new_position() {
        let mut clock = PlaybackClock::new();
        clock.start();
        thread::sleep(Duration::from_millis(5));

        let seek_target = Duration::from_secs(60);
        clock.set_position(seek_target);

        let pts = clock.current_pts();
        assert!(
            pts >= seek_target,
            "current_pts() must be ≥ seek target immediately after set_position while running; \
             target={seek_target:?} pts={pts:?}"
        );
        assert!(
            clock.is_running(),
            "clock must remain running after set_position"
        );
    }

    #[test]
    fn stop_should_clear_seek_offset() {
        let mut clock = PlaybackClock::new();
        clock.set_position(Duration::from_secs(30));
        clock.stop();

        assert_eq!(
            clock.current_pts(),
            Duration::ZERO,
            "stop() must reset seek_offset to ZERO"
        );
    }
}
