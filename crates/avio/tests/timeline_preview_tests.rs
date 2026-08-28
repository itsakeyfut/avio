//! Real-time preview integration tests for the editing model
//! (`avio::TimelinePlayer` -> `ff_preview` runner). Relocated from `ff-preview`
//! when the model moved into `avio` (#1329). Asset-gated (`#[ignore]`).

#![cfg(feature = "preview")]

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use avio::{Clip, PlayerHandle, Timeline, TimelinePlayer};
// `FrameSink` / `PlayerEvent` are ff-preview primitives (avio no longer re-exports
// standalone preview types; it keeps only the TimelinePlayer engine surface).
use ff_preview::{FrameSink, PlayerEvent};

fn test_video_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
}

#[test]
#[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
fn timeline_runner_run_should_deliver_frames_for_single_clip() {
    let path = test_video_path();
    if !path.exists() {
        println!("skipping: video asset not found");
        return;
    }

    struct CountSink(usize, PlayerHandle);
    impl FrameSink for CountSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
            self.0 += 1;
            if self.0 >= 20 {
                self.1.stop();
            }
        }
    }

    let timeline = Timeline::builder()
        .canvas(1280, 720)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new(&path).trim(Duration::ZERO, Duration::from_secs(2)),
        ])
        .build()
        .expect("timeline build failed");

    let (mut runner, handle) = match TimelinePlayer::open(&timeline) {
        Ok(p) => p,
        Err(e) => {
            println!("skipping: open failed: {e}");
            return;
        }
    };

    runner.set_sink(Box::new(CountSink(0, handle.clone())));
    let _ = runner.run();

    let events: Vec<_> = std::iter::from_fn(|| handle.poll_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, PlayerEvent::Eof)),
        "Eof event must be delivered after run() completes"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PlayerEvent::PositionUpdate(_))),
        "PositionUpdate events must be emitted during playback"
    );
}

/// Regression test for the MasterClock::System pause-drift bug.
///
/// After pause -> seek -> sleep N seconds -> play, the first PositionUpdate
/// must carry a PTS close to the seek target (<= target + 2 frame periods),
/// not target + N.
#[test]
#[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
fn timeline_runner_resume_after_seek_while_paused_should_not_drift() {
    let path = test_video_path();
    if !path.exists() {
        println!("skipping: video asset not found");
        return;
    }

    let fps = 30.0_f64;
    let seek_target = Duration::from_secs(1);
    let two_frame_periods = Duration::from_secs_f64(2.0 / fps);

    let timeline = Timeline::builder()
        .canvas(1280, 720)
        .frame_rate(fps)
        .video_track(vec![
            Clip::new(&path).trim(Duration::ZERO, Duration::from_secs(5)),
        ])
        .build()
        .expect("timeline build failed");

    let (runner, handle) = match TimelinePlayer::open(&timeline) {
        Ok(p) => p,
        Err(e) => {
            println!("skipping: open failed: {e}");
            return;
        }
    };

    let handle_bg = handle.clone();
    let bg = thread::spawn(move || {
        let _ = runner.run();
    });

    // Let the runner start, then pause, seek, wait 500 ms, play.
    thread::sleep(Duration::from_millis(50));
    handle.pause();
    thread::sleep(Duration::from_millis(20));
    handle.seek(seek_target);
    thread::sleep(Duration::from_millis(500));
    handle.play();

    // Collect the first PositionUpdate after play.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let first_pts = loop {
        if let Some(PlayerEvent::PositionUpdate(pts)) = handle.poll_event() {
            break Some(pts);
        }
        if std::time::Instant::now() > deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(5));
    };

    handle_bg.stop();
    let _ = bg.join();

    let pts = first_pts.expect("no PositionUpdate received within 5 seconds");
    assert!(
        pts <= seek_target + two_frame_periods,
        "first frame after seek-while-paused should be near seek target; \
         got {pts:?}, expected <= {:?}",
        seek_target + two_frame_periods,
    );
}

#[test]
#[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
fn timeline_runner_seek_should_deliver_seek_completed_event() {
    let path = test_video_path();
    if !path.exists() {
        println!("skipping: video asset not found");
        return;
    }

    let timeline = Timeline::builder()
        .canvas(1280, 720)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new(&path).trim(Duration::ZERO, Duration::from_secs(10)),
        ])
        .build()
        .expect("timeline build failed");

    let (runner, handle) = match TimelinePlayer::open(&timeline) {
        Ok(p) => p,
        Err(e) => {
            println!("skipping: open failed: {e}");
            return;
        }
    };

    let handle_bg = handle.clone();
    let bg = thread::spawn(move || {
        let _ = runner.run();
    });

    thread::sleep(Duration::from_millis(50));
    handle.seek(Duration::from_secs(1));

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let found = loop {
        if let Some(e) = handle.poll_event() {
            if matches!(e, PlayerEvent::SeekCompleted(_)) {
                break true;
            }
        }
        if std::time::Instant::now() > deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };

    handle_bg.stop();
    let _ = bg.join();

    assert!(
        found,
        "SeekCompleted must be delivered within 3 seconds of seek"
    );
}
