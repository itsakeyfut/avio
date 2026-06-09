//! Player event types emitted by [`PlayerRunner`](crate::playback::player_runner::PlayerRunner).

use std::time::Duration;

/// Events emitted by [`PlayerRunner::run`](crate::playback::player_runner::PlayerRunner::run)
/// and delivered to callers via
/// [`PlayerHandle::poll_event`](crate::playback::player_handle::PlayerHandle::poll_event).
pub enum PlayerEvent {
    /// A seek initiated via [`PlayerHandle::seek`](crate::playback::player_handle::PlayerHandle::seek)
    /// has completed.
    ///
    /// `pts` is the actual presentation timestamp of the first frame available
    /// after the seek, which may differ slightly from the requested target due
    /// to I-frame boundaries.
    SeekCompleted(Duration),

    /// The media file has been fully decoded; `run()` is about to return.
    Eof,

    /// Current playback position; emitted once per decoded and presented video frame.
    ///
    /// Not emitted during seeking (while [`FrameResult::Seeking`](crate::playback::FrameResult)
    /// is being returned) — only for fully decoded frames.
    PositionUpdate(Duration),

    /// A non-fatal decode error encountered by the background decode thread.
    ///
    /// Playback continues until EOF; [`PlayerEvent::Eof`] follows shortly after.
    Error(String),
}
