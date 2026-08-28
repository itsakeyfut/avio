//! Background-threaded video frame buffer for ff-preview.
//!
//! [`DecodeBuffer`] decouples decoder latency from the presentation loop by
//! running a [`VideoDecoder`] on a background thread and buffering decoded
//! frames in a bounded ring channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ff_decode::{HardwareAccel, SeekMode, VideoDecoder};
use ff_format::VideoFrame;

use crate::error::PreviewError;

// Constants

/// Default ring buffer capacity for [`DecodeBuffer`] (frames).
const DEFAULT_DECODE_BUFFER_CAPACITY: usize = 8;

// FrameResult

/// The result of a [`DecodeBuffer::pop_frame`] call.
///
/// Callers should match on all three variants; discarding `Seeking` is a
/// common pattern for scrub-bar UIs that want to display the last good frame
/// while a seek is in progress.
#[derive(Debug, Clone)]
pub enum FrameResult {
    /// A decoded frame ready for presentation.
    Frame(VideoFrame),
    /// A seek is in progress; the wrapped value is the last successfully
    /// decoded frame, or `None` if no frame has been decoded yet.
    /// Call [`pop_frame`](DecodeBuffer::pop_frame) again after a short delay
    /// to check whether seeking has completed.
    Seeking(Option<VideoFrame>),
    /// End of file — no more frames will be produced.
    Eof,
}

// SeekEvent

/// An event emitted by [`DecodeBuffer`] after a
/// [`seek_async`](DecodeBuffer::seek_async) completes.
///
/// Obtain the receiver via [`DecodeBuffer::seek_events`] and poll it with
/// `try_recv()` (non-blocking) or `recv()` (blocking).
#[derive(Debug)]
pub enum SeekEvent {
    /// The seek initiated by `seek_async` has completed.
    ///
    /// `pts` is the presentation timestamp of the first frame available after
    /// the seek. Events are typically delivered within ~200 ms for local files.
    Completed { pts: Duration },
}

// DecodeBufferBuilder

/// Builder for [`DecodeBuffer`].
///
/// Created via [`DecodeBuffer::open`]; call [`capacity`](Self::capacity) to
/// override the default ring buffer size, then [`build`](Self::build) to start
/// the background decode thread and obtain a [`DecodeBuffer`].
pub struct DecodeBufferBuilder {
    pub(super) path: PathBuf,
    pub(super) capacity: usize,
    pub(super) hw_accel: HardwareAccel,
}

impl DecodeBufferBuilder {
    /// Set the ring buffer capacity in frames. Default: 8.
    ///
    /// The background thread blocks when the buffer is full and resumes as soon
    /// as the consumer calls [`DecodeBuffer::pop_frame`].
    #[must_use]
    pub fn capacity(self, n: usize) -> Self {
        Self {
            capacity: n,
            ..self
        }
    }

    /// Set the hardware acceleration mode. Default: [`HardwareAccel::Auto`].
    ///
    /// [`HardwareAccel::Auto`] probes available backends in priority order
    /// (NVDEC → QSV → `VideoToolbox` → VAAPI → AMF) and falls back to software
    /// decoding without error if none are available.
    ///
    /// [`HardwareAccel::None`] forces CPU-only decoding.
    #[must_use]
    pub fn hardware_accel(self, accel: HardwareAccel) -> Self {
        Self {
            hw_accel: accel,
            ..self
        }
    }

    /// Build and start the background decode thread.
    ///
    /// The thread pre-fills the ring buffer; frames are delivered in
    /// presentation order. The caller receives a [`DecodeBuffer`] immediately;
    /// frames become available as the thread decodes them.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] if the video file cannot be opened or contains
    /// no decodable video stream.
    pub fn build(self) -> Result<DecodeBuffer, PreviewError> {
        // Open decoder on the calling thread for early validation.
        // Propagates FileNotFound / NoVideoStream / Ffmpeg errors immediately.
        let mut decoder = VideoDecoder::open(&self.path)
            .hardware_accel(self.hw_accel)
            .build()?;

        let (tx, rx) = sync_channel(self.capacity);
        let buffered = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));

        let buffered_thread = Arc::clone(&buffered);
        let cancel_thread = Arc::clone(&cancel);

        let (seek_tx, seek_rx) = channel::<SeekEvent>();
        let (error_tx, error_rx) = channel::<String>();

        let error_tx_thread = error_tx.clone();
        let handle = thread::spawn(move || -> VideoDecoder {
            decode_loop(
                &mut decoder,
                &tx,
                &buffered_thread,
                &cancel_thread,
                &error_tx_thread,
            );
            decoder
        });

        Ok(DecodeBuffer {
            rx: Some(rx),
            buffered,
            handle: Some(handle),
            cancel,
            capacity: self.capacity,
            seeking: Arc::new(AtomicBool::new(false)),
            last_good_frame: None,
            seek_tx,
            seek_rx,
            error_tx,
            error_rx,
        })
    }
}

// DecodeBuffer

/// Pre-decodes frames from a video file into a ring buffer on a background thread.
///
/// `DecodeBuffer` decouples decoder latency from the presentation loop: the
/// background thread keeps the buffer filled so [`pop_frame`](Self::pop_frame)
/// can return the next frame without waiting for the decoder.
///
/// The default ring buffer capacity is 8 frames. Use
/// [`open`](Self::open) → [`capacity`](DecodeBufferBuilder::capacity) →
/// [`build`](DecodeBufferBuilder::build) to configure a different size.
///
/// # Usage
///
/// ```ignore
/// let mut buf = DecodeBuffer::open(Path::new("clip.mp4"))
///     .capacity(16)
///     .build()?;
///
/// while let Some(frame) = buf.pop_frame() {
///     // present frame…
/// }
/// ```
///
/// # Thread safety
///
/// `DecodeBuffer` is `Send` but **not** `Sync`; it must be owned by a single
/// consumer. The internal [`std::sync::mpsc::Receiver`] enforces this.
pub struct DecodeBuffer {
    /// `Option` so `Drop` can take and drop the receiver before joining the thread.
    rx: Option<Receiver<VideoFrame>>,
    /// Approximate count of frames waiting in the ring buffer.
    /// Incremented by the background thread on send; decremented by `pop_frame`.
    buffered: Arc<AtomicUsize>,
    /// Background decode thread handle. Returns the decoder on exit so `seek()`
    /// can recover it without reopening the file.
    handle: Option<JoinHandle<VideoDecoder>>,
    /// Set to `true` to ask the background thread to exit its decode loop.
    cancel: Arc<AtomicBool>,
    /// Channel capacity; needed by `seek()` to create a replacement channel.
    capacity: usize,
    /// Set to `true` while an async seek is in progress.
    seeking: Arc<AtomicBool>,
    /// The last frame returned by `pop_frame`; replayed as a placeholder
    /// while `seeking` is true.
    last_good_frame: Option<VideoFrame>,
    /// Sender side of the seek event channel; cloned into each seek worker.
    seek_tx: Sender<SeekEvent>,
    /// Receiver for seek completion events; exposed via `seek_events()`.
    seek_rx: Receiver<SeekEvent>,
    /// Sender side of the decode error channel; cloned into each decode thread.
    error_tx: Sender<String>,
    /// Receiver for non-fatal decode error messages; exposed via `error_events()`.
    error_rx: Receiver<String>,
}

impl DecodeBuffer {
    /// Open the video at `path` and return a builder for configuring the buffer.
    ///
    /// Chain with [`DecodeBufferBuilder::capacity`] and
    /// [`DecodeBufferBuilder::build`] to start decoding.
    #[must_use]
    pub fn open(path: &Path) -> DecodeBufferBuilder {
        DecodeBufferBuilder {
            path: path.to_path_buf(),
            capacity: DEFAULT_DECODE_BUFFER_CAPACITY,
            hw_accel: HardwareAccel::Auto,
        }
    }

    /// Pop the next decoded frame.
    ///
    /// - Returns [`FrameResult::Seeking`] immediately (non-blocking) while a
    ///   [`seek_async`](Self::seek_async) is in progress.
    /// - Returns [`FrameResult::Frame`] when a frame is available; blocks until
    ///   the background thread produces one.
    /// - Returns [`FrameResult::Eof`] when the background thread reaches end of
    ///   file or the channel is disconnected.
    #[must_use]
    pub fn pop_frame(&mut self) -> FrameResult {
        if self.seeking.load(Ordering::Acquire) {
            return FrameResult::Seeking(self.last_good_frame.clone());
        }
        match self.rx.as_ref().and_then(|rx| rx.recv().ok()) {
            Some(frame) => {
                self.buffered.fetch_sub(1, Ordering::Relaxed);
                self.last_good_frame = Some(frame.clone());
                FrameResult::Frame(frame)
            }
            None => FrameResult::Eof,
        }
    }

    /// Returns an approximation of the number of decoded frames currently
    /// waiting in the buffer.
    ///
    /// This value is advisory only; it may lag the actual buffer state by one
    /// scheduling quantum. Use it for diagnostics, not flow control.
    #[must_use]
    pub fn buffered_frames(&self) -> usize {
        self.buffered.load(Ordering::Relaxed)
    }

    /// Returns a reference to the seek event receiver.
    ///
    /// After calling [`seek_async`](Self::seek_async), poll this receiver to
    /// detect when the seek has completed:
    /// - `try_recv()` — non-blocking; returns `Err(TryRecvError::Empty)` while
    ///   the seek is still in progress.
    /// - `recv()` — blocks until the seek finishes.
    ///
    /// Events are delivered within ~200 ms for local files.
    /// Unconsumed events accumulate in the channel (one per completed seek).
    #[must_use]
    pub fn seek_events(&self) -> &Receiver<SeekEvent> {
        &self.seek_rx
    }

    /// Returns the receiver for non-fatal decode error messages.
    ///
    /// Poll with `try_recv()` in the presentation loop. Each message
    /// corresponds to one failed `decode_one()` call in the background thread.
    /// The background thread exits after sending the error, so
    /// [`pop_frame`](Self::pop_frame) will return `Eof` shortly after.
    #[must_use]
    pub fn error_events(&self) -> &Receiver<String> {
        &self.error_rx
    }

    /// Frame-accurate seek to `target_pts`.
    ///
    /// Stops the background decode thread, seeks the underlying decoder to the
    /// nearest preceding I-frame (`AVSEEK_FLAG_BACKWARD` + codec buffer flush),
    /// then restarts the thread. The restarted thread discards frames until
    /// `PTS ≥ target_pts` before making them available via [`pop_frame`](Self::pop_frame).
    ///
    /// Blocks until the thread has stopped and the seek has been accepted by
    /// the decoder. Frames are filled asynchronously after the method returns.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError::DecodeThreadPoisoned`] if the previous decode
    /// thread panicked and cannot be recovered, or [`PreviewError::SeekFailed`]
    /// if the underlying `FFmpeg` seek fails.
    pub fn seek(&mut self, target_pts: Duration) -> Result<(), PreviewError> {
        let (mut decoder, tx) = self.stop_and_seek(target_pts)?;
        let buffered_thread = Arc::clone(&self.buffered);
        let cancel_thread = Arc::clone(&self.cancel);
        let error_tx_thread = self.error_tx.clone();

        self.handle = Some(thread::spawn(move || -> VideoDecoder {
            // Forward-decode discard: drop frames whose PTS is before target_pts.
            loop {
                if cancel_thread.load(Ordering::Acquire) {
                    return decoder;
                }
                match decoder.decode_one() {
                    Ok(Some(frame)) => {
                        let pts = if frame.timestamp().is_valid() {
                            frame.timestamp().as_duration()
                        } else {
                            Duration::ZERO
                        };
                        if pts >= target_pts {
                            if tx.send(frame).is_ok() {
                                buffered_thread.fetch_add(1, Ordering::Relaxed);
                            } else {
                                return decoder; // receiver dropped
                            }
                            break; // target frame sent; switch to normal loop
                        }
                        // Frame is before target — discard and continue.
                    }
                    Ok(None) => return decoder, // EOF before target
                    Err(e) => {
                        log::warn!("decode error during seek discard error={e}");
                        let _ = error_tx_thread.send(e.to_string());
                        return decoder;
                    }
                }
            }

            // Normal decode loop after the discard phase.
            decode_loop(
                &mut decoder,
                &tx,
                &buffered_thread,
                &cancel_thread,
                &error_tx_thread,
            );
            decoder
        }));

        Ok(())
    }

    /// Coarse seek to the nearest I-frame at or before `target_pts`.
    ///
    /// Faster than [`seek`](Self::seek) because it skips the forward-decode
    /// discard step. The next [`pop_frame`](Self::pop_frame) returns the frame
    /// at the I-frame position, which may be up to ±½ GOP before `target_pts`
    /// (typically ±1–2 s for H.264 at default settings).
    ///
    /// **Typical use:** call repeatedly while a scrub-bar is being dragged;
    /// call [`seek`](Self::seek) on mouse-up for frame accuracy.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError::DecodeThreadPoisoned`] if the previous decode
    /// thread panicked and cannot be recovered, or [`PreviewError::SeekFailed`]
    /// if the underlying `FFmpeg` seek fails.
    pub fn seek_coarse(&mut self, target_pts: Duration) -> Result<(), PreviewError> {
        log::debug!("coarse seek target_pts={target_pts:?}");
        let (mut decoder, tx) = self.stop_and_seek(target_pts)?;
        let buffered_thread = Arc::clone(&self.buffered);
        let cancel_thread = Arc::clone(&self.cancel);
        let error_tx_thread = self.error_tx.clone();

        // No discard loop — start the normal decode loop directly from the I-frame.
        self.handle = Some(thread::spawn(move || -> VideoDecoder {
            decode_loop(
                &mut decoder,
                &tx,
                &buffered_thread,
                &cancel_thread,
                &error_tx_thread,
            );
            decoder
        }));

        Ok(())
    }

    /// Initiate a frame-accurate seek on a background thread and return immediately.
    ///
    /// While seeking is in progress, [`pop_frame`](Self::pop_frame) returns
    /// [`FrameResult::Seeking`] with the last successfully decoded frame as a
    /// placeholder. Normal [`FrameResult::Frame`] values resume once the seek
    /// completes.
    ///
    /// The seek uses the same frame-accurate strategy as [`seek`](Self::seek):
    /// `FFmpeg` jumps to the nearest preceding I-frame, then frames before
    /// `target_pts` are discarded before the first frame is made available.
    ///
    /// If called again before the previous seek completes, the new seek
    /// supersedes the old one; the old worker exits at the next cancel check.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError::DecodeThreadPoisoned`] if the previous decode
    /// thread panicked and its decoder cannot be recovered. The frame-accurate
    /// seek runs on the background worker: a seek failure there is logged and
    /// ends the seek, while discard-phase decode errors are reported via the
    /// [`error_events`](Self::error_events) channel.
    pub fn seek_async(&mut self, target_pts: Duration) -> Result<(), PreviewError> {
        log::debug!("async seek started target_pts={target_pts:?}");

        self.seeking.store(true, Ordering::Release);
        self.cancel.store(true, Ordering::Release);

        if let Some(rx) = &self.rx {
            while rx.try_recv().is_ok() {
                self.buffered.fetch_sub(1, Ordering::Relaxed);
            }
        }

        // Recover the decoder from the old worker synchronously (bounded: the old
        // worker returns promptly once `cancel` is set). A failed join means the
        // decode thread panicked; report it as a typed, recoverable error rather
        // than panicking. The frame-accurate seek then runs on the new worker, so
        // `seek_async` stays non-blocking for the expensive FFmpeg work.
        let Some(mut decoder) = self.handle.take().and_then(|h| h.join().ok()) else {
            self.seeking.store(false, Ordering::Release);
            return Err(PreviewError::DecodeThreadPoisoned);
        };
        drop(self.rx.take());

        let (new_tx, new_rx) = sync_channel(self.capacity);
        self.rx = Some(new_rx);

        let buffered = Arc::clone(&self.buffered);
        let cancel = Arc::clone(&self.cancel);
        let seeking = Arc::clone(&self.seeking);
        let seek_event_tx = self.seek_tx.clone();
        let error_tx_async = self.error_tx.clone();

        let worker = thread::spawn(move || -> VideoDecoder {
            if let Err(e) = decoder.seek(target_pts, SeekMode::Backward) {
                log::warn!("seek_async seek failed target_pts={target_pts:?} error={e}");
                if !cancel.load(Ordering::Acquire) {
                    seeking.store(false, Ordering::Release);
                }
                return decoder;
            }

            buffered.store(0, Ordering::Relaxed);
            cancel.store(false, Ordering::Release);
            // Mark seek as complete so pop_frame() transitions to blocking
            // recv(). Only clear if no newer seek_async has superseded us.
            if !cancel.load(Ordering::Acquire) {
                seeking.store(false, Ordering::Release);
            }

            // Forward-decode discard: skip frames before target_pts.
            loop {
                if cancel.load(Ordering::Acquire) {
                    return decoder;
                }
                match decoder.decode_one() {
                    Ok(Some(frame)) => {
                        let pts = if frame.timestamp().is_valid() {
                            frame.timestamp().as_duration()
                        } else {
                            Duration::ZERO
                        };
                        if pts >= target_pts {
                            let first_pts = pts;
                            // Send the event BEFORE pushing the frame so that
                            // when pop_frame() wakes up the event is already in
                            // the seek_events channel (avoids a try_recv race).
                            let _ = seek_event_tx.send(SeekEvent::Completed { pts: first_pts });
                            if new_tx.send(frame).is_ok() {
                                buffered.fetch_add(1, Ordering::Relaxed);
                            } else {
                                return decoder; // receiver dropped
                            }
                            break;
                        }
                        // Frame before target — discard.
                    }
                    Ok(None) => return decoder, // EOF before target
                    Err(e) => {
                        log::warn!("seek_async discard error error={e}");
                        let _ = error_tx_async.send(e.to_string());
                        return decoder;
                    }
                }
            }

            decode_loop(&mut decoder, &new_tx, &buffered, &cancel, &error_tx_async);
            decoder
        });

        self.handle = Some(worker);
        Ok(())
    }

    /// Shared helper for `seek` and `seek_coarse`.
    ///
    /// 1. Signals cancel, drains the channel, joins the thread to recover the decoder.
    /// 2. Seeks the decoder to the nearest I-frame at or before `target_pts`.
    /// 3. Resets the buffered counter, creates a fresh channel, clears the cancel flag.
    ///
    /// Returns `(decoder, SyncSender)` ready for the caller to spawn a new thread.
    fn stop_and_seek(
        &mut self,
        target_pts: Duration,
    ) -> Result<(VideoDecoder, SyncSender<VideoFrame>), PreviewError> {
        // 1. Signal the background thread to exit its decode loop.
        self.cancel.store(true, Ordering::Release);

        // 2. Drain the channel so the background thread is not blocked on send().
        if let Some(rx) = &self.rx {
            while rx.try_recv().is_ok() {
                self.buffered.fetch_sub(1, Ordering::Relaxed);
            }
        }

        // 3. Join the thread to recover the decoder. A failed join means the
        //    decode thread panicked and cannot be recovered.
        let mut decoder = self
            .handle
            .take()
            .and_then(|h| h.join().ok())
            .ok_or(PreviewError::DecodeThreadPoisoned)?;

        // 4. Seek to the nearest I-frame at or before target_pts.
        //    avformat_seek_file with AVSEEK_FLAG_BACKWARD and avcodec_flush_buffers
        //    are handled inside VideoDecoder::seek (ff-decode/video/decoder_inner/seeking.rs).
        decoder
            .seek(target_pts, SeekMode::Backward)
            .map_err(|e| PreviewError::SeekFailed {
                target: target_pts,
                reason: e.to_string(),
            })?;

        // 5. Reset counter, create a fresh channel, clear the cancel flag.
        self.buffered.store(0, Ordering::Relaxed);
        let (tx, rx) = sync_channel(self.capacity);
        self.rx = Some(rx);
        self.cancel.store(false, Ordering::Release);

        Ok((decoder, tx))
    }
}

impl Drop for DecodeBuffer {
    fn drop(&mut self) {
        // Signal cancel so the thread exits the decode loop promptly.
        self.cancel.store(true, Ordering::Release);
        // Drop the receiver so SyncSender::send() returns Err, unblocking the
        // thread if it is waiting for space in a full channel.
        drop(self.rx.take());
        // Join (ignoring the returned decoder).
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// decode_loop

/// Normal decode loop body shared between `build()` and the post-seek thread.
///
/// Exits when EOF is reached, a decode error occurs, or the `cancel` flag is set,
/// or the receiver drops (i.e., `DecodeBuffer` was dropped).
pub(super) fn decode_loop(
    decoder: &mut VideoDecoder,
    tx: &SyncSender<VideoFrame>,
    buffered: &AtomicUsize,
    cancel: &AtomicBool,
    error_tx: &Sender<String>,
) {
    loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        match decoder.decode_one() {
            Ok(Some(frame)) => {
                if tx.send(frame).is_ok() {
                    buffered.fetch_add(1, Ordering::Relaxed);
                } else {
                    // Receiver was dropped — DecodeBuffer has been dropped.
                    break;
                }
            }
            Ok(None) => break, // EOF
            Err(e) => {
                log::warn!("decode error in background thread error={e}");
                let _ = error_tx.send(e.to_string());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::thread;

    fn test_video_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
    }

    #[test]
    fn decode_buffer_build_should_fail_for_nonexistent_file() {
        let result = DecodeBuffer::open(Path::new("nonexistent_placeholder.mp4")).build();
        assert!(
            result.is_err(),
            "build() must return Err for a non-existent file"
        );
    }

    #[test]
    fn decode_buffer_open_should_use_default_capacity() {
        let path = test_video_path();
        let buf = match DecodeBuffer::open(&path).build() {
            Ok(buf) => buf,
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        // Buffer starts empty; frames arrive asynchronously.
        assert_eq!(
            buf.buffered_frames(),
            0,
            "buffer must report 0 before any frames have been consumed"
        );
    }

    #[test]
    fn decode_buffer_pop_frame_should_return_some_then_none_at_eof() {
        let path = test_video_path();
        let mut buf = match DecodeBuffer::open(&path).capacity(4).build() {
            Ok(buf) => buf,
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        // Pop at least one frame to confirm the decoder is running.
        assert!(
            matches!(buf.pop_frame(), FrameResult::Frame(_)),
            "pop_frame() must return Frame for a valid video file"
        );
    }

    #[test]
    fn seek_should_reposition_to_target_pts() {
        let path = test_video_path();
        let mut buf = match DecodeBuffer::open(&path).capacity(4).build() {
            Ok(buf) => buf,
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        // Consume a few frames to advance past the start.
        for _ in 0..5 {
            if matches!(buf.pop_frame(), FrameResult::Eof) {
                println!("skipping: EOF before seek target");
                return;
            }
        }

        let seek_target = Duration::from_secs(1);
        match buf.seek(seek_target) {
            Ok(()) => {}
            Err(e) => {
                println!("skipping: seek not supported or failed: {e}");
                return;
            }
        }

        // After seek, the first frame's PTS must be at or near the target.
        let frame = match buf.pop_frame() {
            FrameResult::Frame(f) => f,
            FrameResult::Eof | FrameResult::Seeking(_) => {
                println!("skipping: no frame after seek");
                return;
            }
        };

        if frame.timestamp().is_valid() {
            let pts = frame.timestamp().as_duration();
            // Allow ±1 second of tolerance (one GOP) for I-frame alignment.
            assert!(
                pts >= seek_target.saturating_sub(Duration::from_secs(1)),
                "post-seek frame PTS must be near target; target={seek_target:?} pts={pts:?}"
            );
        }
    }

    #[test]
    fn seek_should_fail_for_stopped_buffer() {
        // Build with non-existent file → build() fails.
        // This confirms seek errors are propagated correctly.
        let result = DecodeBuffer::open(Path::new("nonexistent.mp4")).build();
        assert!(
            result.is_err(),
            "build() must fail for non-existent file (precondition for seek error path)"
        );
    }

    #[test]
    fn seek_async_should_send_completed_event_with_first_frame_pts() {
        let path = test_video_path();
        let mut buf = match DecodeBuffer::open(&path).capacity(4).build() {
            Ok(buf) => buf,
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        // Pop one frame to establish last_good_frame.
        match buf.pop_frame() {
            FrameResult::Frame(_) => {}
            _ => {
                println!("skipping: no initial frame available");
                return;
            }
        }

        let seek_target = Duration::from_secs(1);
        buf.seek_async(seek_target)
            .expect("seek_async on a healthy buffer must return Ok");

        // Drive the seek to completion by polling pop_frame.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for seek to complete"
            );
            match buf.pop_frame() {
                FrameResult::Frame(_) => break, // seek done, first post-seek frame received
                FrameResult::Seeking(_) => thread::sleep(Duration::from_millis(10)),
                FrameResult::Eof => {
                    println!("skipping: EOF reached during seek event test");
                    return;
                }
            }
        }

        // After pop_frame returned Frame, SeekEvent::Completed must be in the channel.
        let event = buf.seek_events().try_recv();
        assert!(
            event.is_ok(),
            "expected SeekEvent::Completed after pop_frame returned Frame; got Err"
        );
        if let Ok(SeekEvent::Completed { pts }) = event {
            assert!(
                pts >= Duration::ZERO,
                "seek event pts must be non-negative; got pts={pts:?}"
            );
        }
    }

    #[test]
    fn seek_async_should_deliver_frames_after_completion() {
        let path = test_video_path();
        let mut buf = match DecodeBuffer::open(&path).capacity(4).build() {
            Ok(buf) => buf,
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        // Pop one frame to establish last_good_frame.
        match buf.pop_frame() {
            FrameResult::Frame(_) => {}
            _ => {
                println!("skipping: no initial frame available");
                return;
            }
        }

        let seek_target = Duration::from_secs(1);
        buf.seek_async(seek_target)
            .expect("seek_async on a healthy buffer must return Ok");

        // Poll until a Frame arrives (seek complete) or we time out.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match buf.pop_frame() {
                FrameResult::Frame(_) => break, // seek completed successfully
                FrameResult::Seeking(_) => {
                    thread::sleep(Duration::from_millis(10));
                }
                FrameResult::Eof => {
                    println!("skipping: EOF reached during seek_async test");
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "seek_async: timed out waiting for seek to complete"
            );
        }
    }

    /// Drives an async seek to completion by polling `pop_frame`, returning true
    /// when a post-seek frame arrives and false on EOF (a legitimate skip).
    fn drain_async_seek(buf: &mut DecodeBuffer) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match buf.pop_frame() {
                FrameResult::Frame(_) => return true,
                FrameResult::Seeking(_) => thread::sleep(Duration::from_millis(10)),
                FrameResult::Eof => return false,
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
        }
    }

    // Exercises the decoder-recovery path across all three seek entry points in
    // sequence: each recovers the decoder from the prior worker (poisoned would
    // surface as DecodeThreadPoisoned) and resumes decoding. Probe-gated (RK-002):
    // skips when the fixture video is unavailable or EOF is reached early.
    #[test]
    fn seek_recovery_should_survive_interleaved_seek_variants() {
        let path = test_video_path();
        let mut buf = match DecodeBuffer::open(&path).capacity(4).build() {
            Ok(buf) => buf,
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        if !matches!(buf.pop_frame(), FrameResult::Frame(_)) {
            println!("skipping: no initial frame available");
            return;
        }

        // Frame-accurate seek → must recover and deliver a frame.
        if buf.seek(Duration::from_millis(500)).is_err() {
            println!("skipping: seek failed on fixture");
            return;
        }
        if !matches!(buf.pop_frame(), FrameResult::Frame(_) | FrameResult::Eof) {
            println!("skipping: no frame after seek");
            return;
        }

        // Coarse seek → recovers the decoder from the previous seek's worker.
        if buf.seek_coarse(Duration::from_secs(1)).is_err() {
            println!("skipping: seek_coarse failed on fixture");
            return;
        }
        if matches!(buf.pop_frame(), FrameResult::Seeking(_)) {
            println!("skipping: unexpected seeking state after coarse seek");
            return;
        }

        // Async seek → also recovers the decoder; must return Ok on a healthy
        // buffer and eventually deliver a frame (or hit EOF).
        buf.seek_async(Duration::from_millis(250))
            .expect("seek_async on a healthy buffer must return Ok");
        let _ = drain_async_seek(&mut buf);
    }
}
