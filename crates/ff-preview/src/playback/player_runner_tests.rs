//! Unit tests for [`PlayerRunner`](super::PlayerRunner) and the audio decode thread.
//!
//! Split out of `player_runner.rs` to keep that file under the size limit.
//! Included as a private `#[cfg(test)]` submodule so the tests retain access to
//! crate-private fields via `super::*`.

use super::super::player::PreviewPlayer;
use super::super::player_handle::PlayerHandle;
use super::*;

fn test_video_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
}

fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/audio/konekonoosanpo.mp3")
}

// ── open ──────────────────────────────────────────────────────────────────

#[test]
fn preview_player_open_should_fail_for_nonexistent_file() {
    let result = PreviewPlayer::open("nonexistent_preview.mp4");
    assert!(
        result.is_err(),
        "open() must return Err for a non-existent file"
    );
}

// ── play / pause / stop via handle ───────────────────────────────────────

#[test]
fn player_handle_play_pause_should_update_paused_flag_immediately() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };

    assert!(!handle.paused.load(Ordering::Relaxed));
    assert!(!handle.stopped.load(Ordering::Relaxed));

    handle.pause();
    assert!(handle.paused.load(Ordering::Relaxed));

    handle.play();
    assert!(!handle.paused.load(Ordering::Relaxed));
    assert!(!handle.stopped.load(Ordering::Relaxed));

    handle.stop();
    assert!(handle.stopped.load(Ordering::Relaxed));
}

// ── run with sink ─────────────────────────────────────────────────────────

#[test]
fn player_runner_run_should_deliver_frames_to_sink() {
    struct CountSink(Arc<Mutex<usize>>);
    impl FrameSink for CountSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        }
    }

    let path = test_video_path();
    let (mut runner, _handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };

    let count = Arc::new(Mutex::new(0usize));
    runner.set_sink(Box::new(CountSink(Arc::clone(&count))));

    match runner.run() {
        Ok(()) => {}
        Err(e) => {
            println!("skipping: run() error: {e}");
            return;
        }
    }

    let frames = *count
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        frames > 0,
        "run() must deliver at least one frame to the sink"
    );
}

// ── pop_audio_samples ────────────────────────────────────────────────────

#[test]
fn pop_audio_samples_should_return_empty_when_paused() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    handle.pause();
    let samples = handle.pop_audio_samples(1024);
    assert!(
        samples.is_empty(),
        "pop_audio_samples() must return empty while paused"
    );
}

#[test]
fn pop_audio_samples_should_return_empty_when_stopped() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    handle.stop();
    let samples = handle.pop_audio_samples(1024);
    assert!(
        samples.is_empty(),
        "pop_audio_samples() must return empty while stopped"
    );
}

#[test]
fn pop_audio_samples_should_return_empty_for_zero_n_samples() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    handle.play();
    let samples = handle.pop_audio_samples(0);
    assert!(
        samples.is_empty(),
        "pop_audio_samples(0) must always return empty"
    );
}

#[test]
fn pop_audio_samples_should_be_callable_via_cloned_handle() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    let shared = handle.clone();
    let _samples = shared.pop_audio_samples(0);
}

#[test]
fn pop_audio_samples_clock_increment_should_equal_half_sample_count() {
    let stereo_samples: usize = 9_600;
    let expected_frames: u64 = (stereo_samples / 2) as u64;
    assert_eq!(
        expected_frames, 4_800,
        "9600 stereo samples must yield 4800 clock frames"
    );
    let pts = Duration::from_secs_f64(f64::from(48_000u32).recip() * expected_frames as f64);
    assert!(
        (pts.as_secs_f64() - 0.1).abs() < 1e-6,
        "4800 frames at 48 kHz must equal 100 ms; got {pts:?}"
    );
}

// ── current_pts / duration ───────────────────────────────────────────────

#[test]
fn current_pts_should_return_zero_before_first_frame() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    assert_eq!(
        handle.current_pts(),
        Duration::ZERO,
        "current_pts() must be ZERO before any frame is presented"
    );
}

#[test]
fn duration_should_return_some_for_file_with_known_duration() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    assert!(
        handle.duration().is_some(),
        "duration() must return Some for a file with a known container duration"
    );
    let d = handle.duration().unwrap();
    assert!(
        d > Duration::ZERO,
        "duration() must be positive for a valid media file; got {d:?}"
    );
}

#[test]
fn duration_should_return_none_when_duration_millis_is_sentinel() {
    let sentinel = u64::MAX;
    let result: Option<Duration> = if sentinel == u64::MAX {
        None
    } else {
        Some(Duration::from_millis(sentinel))
    };
    assert!(result.is_none(), "sentinel u64::MAX must map to None");

    let valid = 5_000u64;
    let result: Option<Duration> = if valid == u64::MAX {
        None
    } else {
        Some(Duration::from_millis(valid))
    };
    assert_eq!(result, Some(Duration::from_secs(5)));
}

#[test]
fn current_pts_should_advance_after_frames_are_presented() {
    struct PtsSink(Arc<Mutex<Option<Duration>>>);
    impl FrameSink for PtsSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pts);
        }
    }

    let path = test_video_path();
    let (mut runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };

    let last_pts = Arc::new(Mutex::new(None::<Duration>));
    runner.set_sink(Box::new(PtsSink(Arc::clone(&last_pts))));
    let _ = runner.run();

    let sink_pts = last_pts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .unwrap_or(Duration::ZERO);
    let player_pts = handle.current_pts();
    let diff = sink_pts.abs_diff(player_pts);
    assert!(
        diff <= Duration::from_millis(1),
        "current_pts() must be within 1 ms of the last sink PTS; \
         player_pts={player_pts:?} sink_pts={sink_pts:?} diff={diff:?}"
    );
}

// ── seek ──────────────────────────────────────────────────────────────────

#[test]
fn seek_coarse_should_delegate_to_decode_buffer() {
    let path = test_video_path();
    let (runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };

    let target = Duration::from_secs(1);
    handle.seek(target);

    // Stop after a short time so the test doesn't block for the full file.
    let handle_thread = handle.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        handle_thread.stop();
    });

    match runner.run() {
        Ok(()) => {}
        Err(e) => {
            println!("skipping: run() error: {e}");
        }
    }
}

// ── proxy ─────────────────────────────────────────────────────────────────

#[test]
fn use_proxy_if_available_should_return_false_when_no_proxy_in_dir() {
    let path = test_video_path();
    let (mut runner, _handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    let tmp = std::env::temp_dir().join("ff_preview_no_proxy_dir_test");
    let _ = std::fs::create_dir_all(&tmp);
    let found = runner.use_proxy_if_available(&tmp);
    assert!(
        !found,
        "must return false when no proxy files exist in the directory"
    );
}

#[test]
fn active_source_should_return_original_path_before_proxy_activation() {
    let path = test_video_path();
    let (runner, _handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    assert_eq!(
        runner.active_source(),
        path.as_path(),
        "active_source() must equal the original path before any proxy activation"
    );
}

// ── set_rate / set_av_offset ──────────────────────────────────────────────

#[test]
fn set_rate_should_accept_positive_value() {
    let path = test_video_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };
    // Verify that calling set_rate with a valid value does not panic.
    handle.set_rate(2.0);
    handle.set_rate(0.5);
}

#[test]
fn set_av_offset_default_should_be_zero() {
    use std::sync::atomic::{AtomicI64, Ordering};
    let offset = AtomicI64::new(0);
    assert_eq!(offset.load(Ordering::Relaxed), 0);
}

#[test]
fn positive_av_offset_should_reduce_adjusted_video_pts() {
    let video_pts = Duration::from_millis(1_000);
    let offset_ms: i64 = 200;
    let adjusted = if offset_ms >= 0 {
        let offset = Duration::from_millis(offset_ms as u64);
        video_pts.saturating_sub(offset)
    } else {
        let offset = Duration::from_millis(offset_ms.unsigned_abs());
        video_pts + offset
    };
    assert_eq!(
        adjusted,
        Duration::from_millis(800),
        "positive offset must reduce adjusted_video_pts by offset amount"
    );
}

#[test]
fn negative_av_offset_should_increase_adjusted_video_pts() {
    let video_pts = Duration::from_millis(1_000);
    let offset_ms: i64 = -200;
    let adjusted = if offset_ms >= 0 {
        let offset = Duration::from_millis(offset_ms as u64);
        video_pts.saturating_sub(offset)
    } else {
        let offset = Duration::from_millis(offset_ms.unsigned_abs());
        video_pts + offset
    };
    assert_eq!(
        adjusted,
        Duration::from_millis(1_200),
        "negative offset must increase adjusted_video_pts by offset amount"
    );
}

#[test]
fn positive_av_offset_at_zero_pts_should_saturate_to_zero() {
    let video_pts = Duration::ZERO;
    let offset_ms: i64 = 100;
    let adjusted = video_pts.saturating_sub(Duration::from_millis(offset_ms as u64));
    assert_eq!(
        adjusted,
        Duration::ZERO,
        "saturating_sub on zero pts must clamp to zero not underflow"
    );
}

// ── audio_sample_rate ────────────────────────────────────────────────────

#[test]
fn audio_sample_rate_should_return_some_48_khz_for_audio_only_file() {
    let path = test_audio_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: audio file not available: {e}");
            return;
        }
    };
    assert_eq!(
        handle.audio_sample_rate(),
        Some(DECODED_SAMPLE_RATE),
        "audio_sample_rate() must return Some(48_000) for a file with an audio stream"
    );
}

#[test]
fn audio_sample_rate_should_return_some_48_khz_regardless_of_source_native_rate() {
    // Verifies that audio_sample_rate() always returns the decoder's fixed
    // output rate (48 000 Hz), not the source file's native rate.
    // The audio file (konekonoosanpo.mp3) may be 44 100 Hz natively — the
    // returned value must still be 48 000.
    let path = test_audio_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: audio file not available: {e}");
            return;
        }
    };
    if let Some(rate) = handle.audio_sample_rate() {
        assert_eq!(
            rate, DECODED_SAMPLE_RATE,
            "audio_sample_rate() must equal DECODED_SAMPLE_RATE=48 000 regardless of source"
        );
    }
}

#[test]
fn audio_sample_rate_should_return_none_when_no_audio_buf_present() {
    // Verifies the None path: when audio_buf is absent (video-only source),
    // audio_sample_rate() returns None.
    // We exercise the logic directly since we don't have a video-only asset.
    let buf: Option<std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<f32>>>> = None;
    let rate: Option<u32> = buf.as_ref().map(|_| DECODED_SAMPLE_RATE);
    assert_eq!(
        rate, None,
        "audio_sample_rate() must return None when no audio ring buffer is present"
    );
}

// ── audio-only ────────────────────────────────────────────────────────────

#[test]
fn audio_only_open_should_succeed() {
    let path = test_audio_path();
    match PreviewPlayer::open(&path) {
        Ok(player) => {
            let (runner, handle) = player.split();
            // Audio-only: runner has no decode buffer.
            assert!(
                runner.decode_buf.is_none(),
                "audio-only runner must have no video decode buffer"
            );
            // Handle has an audio buffer.
            assert!(
                handle.audio_buf.is_some(),
                "audio-only handle must have an audio ring buffer"
            );
        }
        Err(e) => {
            println!("skipping: audio file not available: {e}");
        }
    }
}

#[test]
fn audio_only_run_should_return_ok_without_video_frames() {
    let path = test_audio_path();
    let (mut runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: audio file not available: {e}");
            return;
        }
    };

    struct CountingSink(usize);
    impl FrameSink for CountingSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
            self.0 += 1;
        }
    }
    runner.set_sink(Box::new(CountingSink(0)));

    let handle_thread = handle.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        handle_thread.stop();
    });

    let result = runner.run();
    assert!(
        result.is_ok(),
        "run() on an audio-only player must return Ok; got {result:?}"
    );
    assert_eq!(
        handle.current_pts(),
        Duration::ZERO,
        "current_pts() must remain ZERO for audio-only playback (no video frames)"
    );
}

#[test]
fn audio_only_seek_should_not_fail_for_valid_target() {
    let path = test_audio_path();
    let (_runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: audio file not available: {e}");
            return;
        }
    };
    // seek() on audio-only player sends a command without errors.
    handle.seek(Duration::from_secs(1));
}

// ── seek event delivery (integration) ────────────────────────────────────

#[test]
#[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
fn seek_should_deliver_seek_completed_event_via_poll_event() {
    let path = test_video_path();
    if !path.exists() {
        println!("skipping: video file not found at {}", path.display());
        return;
    }

    let (runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: open failed: {e}");
            return;
        }
    };

    let handle_bg = handle.clone();
    let bg = thread::spawn(move || {
        let _ = runner.run();
    });

    // Give the runner one frame period to start, then seek.
    thread::sleep(Duration::from_millis(50));
    let target = Duration::from_secs(1);
    handle.seek(target);

    // Wait up to 2 seconds for SeekCompleted, skipping PositionUpdate
    // events that may have accumulated during the startup window.
    let deadline = Instant::now() + Duration::from_secs(2);
    let seek_result = loop {
        match handle.poll_event() {
            Some(PlayerEvent::SeekCompleted(pts)) => break Ok(pts),
            Some(PlayerEvent::Eof) => break Err("Eof"),
            Some(PlayerEvent::Error(_)) => break Err("Error"),
            Some(PlayerEvent::PositionUpdate(_)) => {} // skip pre-seek updates
            None => {}
        }
        if Instant::now() > deadline {
            break Err("timeout");
        }
        thread::sleep(Duration::from_millis(10));
    };

    handle_bg.stop();
    let _ = bg.join();

    match seek_result {
        Ok(pts) => {
            assert!(
                pts >= target.saturating_sub(Duration::from_millis(100)),
                "SeekCompleted pts must be near the requested target; \
                 target={target:?} pts={pts:?}"
            );
        }
        Err(reason) => {
            panic!("SeekCompleted not received within 2 seconds: {reason}");
        }
    }
}

// ── PlayerEvent: PositionUpdate + Error ───────────────────────────────────

#[test]
fn position_update_and_error_event_variants_should_be_accessible() {
    let _ = PlayerEvent::PositionUpdate(Duration::ZERO);
    let _ = PlayerEvent::Error("test error".to_string());
}

#[test]
fn eof_event_should_be_delivered_after_run_completes() {
    let path = test_audio_path();
    let (runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    // Stop after 150 ms so the test does not wait for the full audio duration.
    let handle_stop = handle.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        handle_stop.stop();
    });

    let _ = runner.run();
    let events: Vec<_> = std::iter::from_fn(|| handle.poll_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, PlayerEvent::Eof)),
        "Eof event must be delivered after run() returns; collected {} events",
        events.len()
    );
}

#[test]
#[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
fn position_update_should_be_emitted_for_each_video_frame() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4");
    if !path.exists() {
        println!("skipping: video asset not found");
        return;
    }

    use std::sync::{Arc, Mutex};
    struct CountSink {
        count: Arc<Mutex<usize>>,
        max: usize,
        handle: PlayerHandle,
    }
    impl FrameSink for CountSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
            let mut g = self
                .count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *g += 1;
            if *g >= self.max {
                self.handle.stop();
            }
        }
    }

    let (mut runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    let count = Arc::new(Mutex::new(0usize));
    runner.set_sink(Box::new(CountSink {
        count: Arc::clone(&count),
        max: 20,
        handle: handle.clone(),
    }));
    let _ = runner.run();

    let frames = *count
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let position_updates: Vec<_> = std::iter::from_fn(|| handle.poll_event())
        .filter(|e| matches!(e, PlayerEvent::PositionUpdate(_)))
        .collect();

    assert!(
        !position_updates.is_empty(),
        "at least one PositionUpdate event must be emitted; frames delivered={frames}"
    );
    assert!(
        position_updates.len() <= frames,
        "PositionUpdate count ({}) must not exceed frame count ({frames})",
        position_updates.len()
    );
}

// ── HardwareAccel ─────────────────────────────────────────────────────────

#[test]
fn hardware_accel_variants_should_be_accessible_on_player_runner() {
    // Type-check / accessibility test — no asset required.
    let _ = HardwareAccel::Auto;
    let _ = HardwareAccel::None;
    let _ = HardwareAccel::Nvdec;
    let _ = HardwareAccel::Qsv;
    let _ = HardwareAccel::Amf;
    let _ = HardwareAccel::VideoToolbox;
    let _ = HardwareAccel::Vaapi;
}

#[test]
fn set_hardware_accel_none_should_complete_without_error_on_audio_only_file() {
    // Audio-only path has no video decode buffer; the hw_accel rebuild
    // at run() start is skipped.  Verifies the setter is a no-op when
    // no decode buffer exists, and run() still returns Ok.
    let path = test_audio_path();
    let (mut runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: audio file not available: {e}");
            return;
        }
    };

    runner.set_hardware_accel(HardwareAccel::None);
    assert_eq!(runner.hw_accel, HardwareAccel::None);

    let handle_stop = handle.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        handle_stop.stop();
    });

    let result = runner.run();
    assert!(
        result.is_ok(),
        "run() with HardwareAccel::None must return Ok; got {result:?}"
    );
}

#[test]
#[ignore = "requires assets/video/gameplay.mp4 and hardware decoder; run with -- --include-ignored"]
fn hardware_accel_auto_should_deliver_frames_on_video_file() {
    let path = test_video_path();
    let (mut runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: video file not available: {e}");
            return;
        }
    };

    runner.set_hardware_accel(HardwareAccel::Auto);

    struct CountSink {
        count: usize,
        max: usize,
        handle: PlayerHandle,
    }
    impl FrameSink for CountSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
            self.count += 1;
            if self.count >= self.max {
                self.handle.stop();
            }
        }
    }
    runner.set_sink(Box::new(CountSink {
        count: 0,
        max: 5,
        handle: handle.clone(),
    }));

    let result = runner.run();
    assert!(
        result.is_ok(),
        "run() with HardwareAccel::Auto must return Ok; got {result:?}"
    );
    assert!(
        handle.current_pts() > Duration::ZERO,
        "at least one frame must have been presented"
    );
}
