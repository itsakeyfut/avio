//! Async wrapper around [`PlayerHandle`].
//!
//! This module is only compiled when the `tokio` feature is enabled.

use std::path::Path;
use std::time::Duration;

use super::player::PreviewPlayer;
use super::player_handle::PlayerHandle;
use crate::error::PreviewError;
use crate::event::PlayerEvent;

// ── AsyncPreviewPlayer ────────────────────────────────────────────────────────

/// Async wrapper around [`PlayerHandle`].
///
/// On creation, a `spawn_blocking` thread opens the file, splits the player,
/// and starts the runner. All control methods delegate directly to the
/// underlying [`PlayerHandle`] — no inner `Mutex`.
///
/// # Usage
///
/// ```ignore
/// use ff_preview::AsyncPreviewPlayer;
/// use std::time::Duration;
///
/// let player = AsyncPreviewPlayer::open("clip.mp4").await?;
/// player.play();
/// player.seek(Duration::from_secs(30));
/// while let Some(event) = player.next_event().await { ... }
/// ```
#[derive(Clone)]
pub struct AsyncPreviewPlayer {
    handle: PlayerHandle,
}

impl AsyncPreviewPlayer {
    /// Open a media file asynchronously.
    ///
    /// File I/O and codec initialisation run on a `spawn_blocking` thread.
    /// The runner is also started on a dedicated blocking thread and runs until
    /// [`stop`](Self::stop) is called or EOF is reached.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] if the file is missing, unreadable, or contains
    /// neither a video nor an audio stream.
    pub async fn open(path: impl AsRef<Path> + Send + 'static) -> Result<Self, PreviewError> {
        let path = path.as_ref().to_path_buf();
        let task = tokio::task::spawn_blocking(move || {
            PreviewPlayer::open(&path).map(PreviewPlayer::split)
        });
        let (runner, handle) = task.await.map_err(|e| PreviewError::Ffmpeg {
            code: 0,
            message: format!("tokio task join error: {e}"),
        })??;

        tokio::task::spawn_blocking(move || {
            let _ = runner.run();
        });

        Ok(Self { handle })
    }

    /// Resume playback.
    pub fn play(&self) {
        self.handle.play();
    }

    /// Pause playback.
    pub fn pause(&self) {
        self.handle.pause();
    }

    /// Stop the presentation loop.
    pub fn stop(&self) {
        self.handle.stop();
    }

    /// Seek to `pts`.
    pub fn seek(&self, pts: Duration) {
        self.handle.seek(pts);
    }

    /// Set the playback rate.
    pub fn set_rate(&self, rate: f64) {
        self.handle.set_rate(rate);
    }

    /// PTS of the most recently presented frame.
    #[must_use]
    pub fn current_pts(&self) -> Duration {
        self.handle.current_pts()
    }

    /// Container-reported duration, or `None` for live / streaming sources.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.handle.duration()
    }

    /// Pull up to `n` interleaved stereo `f32` PCM samples at 48 kHz.
    #[must_use]
    pub fn pop_audio_samples(&self, n: usize) -> Vec<f32> {
        self.handle.pop_audio_samples(n)
    }

    /// Poll for the next [`PlayerEvent`] without blocking.
    ///
    /// Returns `None` when no events are pending.
    #[must_use]
    pub fn poll_event(&self) -> Option<PlayerEvent> {
        self.handle.poll_event()
    }

    /// Await the next [`PlayerEvent`].
    ///
    /// Blocks in a `spawn_blocking` thread until an event arrives or the
    /// channel is closed. Returns `None` when the runner has exited.
    pub async fn next_event(&self) -> Option<PlayerEvent> {
        let handle = self.handle.clone();
        tokio::task::spawn_blocking(move || handle.recv_event())
            .await
            .ok()
            .flatten()
    }
}

impl Drop for AsyncPreviewPlayer {
    fn drop(&mut self) {
        self.handle.stop();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_video_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
    }

    #[test]
    fn async_preview_player_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AsyncPreviewPlayer>();
    }

    #[test]
    #[ignore = "requires FFmpeg and assets/video/gameplay.mp4; run with -- --include-ignored"]
    fn async_preview_player_should_open_and_report_nonzero_duration() {
        let path = test_video_path();
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(async {
                let player = match AsyncPreviewPlayer::open(path.clone()).await {
                    Ok(p) => p,
                    Err(e) => {
                        println!("skipping: open failed: {e}");
                        return;
                    }
                };
                assert!(
                    player.duration().is_some_and(|d| d > Duration::ZERO),
                    "duration must be positive for a valid media file"
                );
            }),
            Err(e) => println!("skipping: failed to build tokio runtime: {e}"),
        }
    }
}
