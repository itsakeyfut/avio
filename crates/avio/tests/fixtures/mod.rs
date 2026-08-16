//! Test fixtures and helpers for ff-pipeline integration tests.

#![allow(dead_code)]

use std::path::PathBuf;

use ff_encode::{AudioCodec, VideoCodec, VideoEncoder};
use ff_format::{AudioFrame, PixelFormat, PooledBuffer, SampleFormat, Timestamp, VideoFrame};

/// Returns the path to the shared test assets directory.
pub fn assets_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/../../assets", manifest_dir))
}

/// Returns the path to the test video file (contains both video and audio).
pub fn test_video_path() -> PathBuf {
    assets_dir().join("video/gameplay.mp4")
}

/// Returns the path to the test audio file.
pub fn test_audio_path() -> PathBuf {
    assets_dir().join("audio/konekonoosanpo.mp3")
}

/// Returns the directory used for test output files.
pub fn test_output_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/target/test-output", manifest_dir))
}

/// Creates a path inside the test output directory.
///
/// The directory is created automatically if it does not exist.
pub fn test_output_path(filename: &str) -> PathBuf {
    let dir = test_output_dir();
    std::fs::create_dir_all(&dir).ok();
    dir.join(filename)
}

/// RAII guard that deletes a file when dropped.
///
/// Ensures test output files are cleaned up even when tests panic.
pub struct FileGuard {
    path: PathBuf,
}

impl FileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
        // Remove empty ancestor directories (test-output/, then target/).
        // remove_dir is a no-op when the directory is not empty, so this is
        // safe when multiple tests run in parallel.
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
            if let Some(grandparent) = parent.parent() {
                let _ = std::fs::remove_dir(grandparent);
            }
        }
    }
}

// ── Synthetic frame factories ──────────────────────────────────────────────────

/// YUV420P frame filled with a solid colour specified as (Y, U, V).
pub fn yuv420p_frame(width: u32, height: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    let y_plane = PooledBuffer::standalone(vec![y; (width * height) as usize]);
    let u_plane = PooledBuffer::standalone(vec![u; ((width / 2) * (height / 2)) as usize]);
    let v_plane = PooledBuffer::standalone(vec![v; ((width / 2) * (height / 2)) as usize]);
    VideoFrame::new(
        vec![y_plane, u_plane, v_plane],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("failed to create test frame")
}

/// Stereo F32 audio frame filled with silence.
pub fn silent_audio_frame(samples: usize, sample_rate: u32) -> AudioFrame {
    AudioFrame::empty(samples, 2, sample_rate, SampleFormat::F32)
        .expect("failed to create silent audio frame")
}

// ── Source file generator ─────────────────────────────────────────────────────

/// Encodes `frame_count` synthetic frames to `path` as an MP4 with MPEG-4 video
/// and AAC audio.  Returns `None` (and prints a skip message) if the encoder
/// cannot be built — callers should treat this as "skip the test".
///
/// * `width` / `height` — video dimensions (must be even)
/// * `fps` — frame rate
/// * `frame_count` — number of video frames to write
/// * `y`, `u`, `v` — solid fill colour for every frame
pub fn make_source_file(
    path: &PathBuf,
    width: u32,
    height: u32,
    fps: f64,
    frame_count: usize,
    y: u8,
    u: u8,
    v: u8,
) -> Option<()> {
    let sample_rate = 48_000u32;
    let audio_frame_samples = 1024usize;
    let total_audio_samples = (sample_rate as f64 * frame_count as f64 / fps) as usize;
    let audio_frames = total_audio_samples.div_ceil(audio_frame_samples);

    let mut encoder = match VideoEncoder::create(path)
        .video(width, height, fps)
        .video_codec(VideoCodec::Mpeg4)
        .audio(sample_rate, 2)
        .audio_codec(AudioCodec::Aac)
        .audio_bitrate(128_000)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: cannot build source encoder: {e}");
            return None;
        }
    };

    for _ in 0..frame_count {
        let frame = yuv420p_frame(width, height, y, u, v);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: push_video failed: {e}");
            return None;
        }
    }

    for _ in 0..audio_frames {
        let frame = silent_audio_frame(audio_frame_samples, sample_rate);
        if let Err(e) = encoder.push_audio(&frame) {
            println!("Skipping: push_audio failed: {e}");
            return None;
        }
    }

    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder finish failed: {e}");
        return None;
    }

    Some(())
}
