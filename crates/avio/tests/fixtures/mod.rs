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
        // The output directory is deliberately left in place. Removing it here
        // when it happened to be empty raced with every other test between
        // `test_output_path`'s `create_dir_all` and its own `File::create`, so a
        // test could be handed a path whose directory a finishing test had just
        // deleted, and fail with `NotFound` on a file it was about to write.
        // `remove_dir` refusing to delete a non-empty directory does not help:
        // the window is that the directory is momentarily empty. It lives under
        // `target/`, which `cargo clean` owns, so leaving it costs nothing.
    }
}

// Synthetic frame factories

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

// Source file generator

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

/// Writes a PCM WAV holding a 440 Hz tone at half scale (`bits` is 16 or 24).
///
/// Hand-written rather than encoded, so a test using it does not pass merely
/// because avio's encoder and decoder agree with each other. `make_source_file`
/// writes silent audio, which cannot show whether an audio effect changed the
/// signal, so tests that measure level use this instead.
pub fn write_tone_wav(
    path: &std::path::Path,
    sample_rate: u32,
    channels: u16,
    bits: u16,
    secs: f64,
) {
    use std::io::Write;

    let frames = (f64::from(sample_rate) * secs) as u32;
    let bytes_per_sample = u32::from(bits / 8);
    let block_align = u32::from(channels) * bytes_per_sample;
    let data_len = frames * block_align;

    let mut b: Vec<u8> = Vec::with_capacity(44 + data_len as usize);
    b.extend(b"RIFF");
    b.extend(&(36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(&16u32.to_le_bytes());
    b.extend(&1u16.to_le_bytes()); // PCM
    b.extend(&channels.to_le_bytes());
    b.extend(&sample_rate.to_le_bytes());
    b.extend(&(sample_rate * block_align).to_le_bytes());
    b.extend(&u16::try_from(block_align).unwrap_or(u16::MAX).to_le_bytes());
    b.extend(&bits.to_le_bytes());
    b.extend(b"data");
    b.extend(&data_len.to_le_bytes());

    for i in 0..frames {
        let t = f64::from(i) / f64::from(sample_rate);
        let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.5;
        for _ in 0..channels {
            if bits == 16 {
                b.extend(&((v * f64::from(i16::MAX)) as i16).to_le_bytes());
            } else {
                b.extend(&((v * 8_388_607.0) as i32).to_le_bytes()[0..3]);
            }
        }
    }

    std::fs::File::create(path)
        .expect("create wav")
        .write_all(&b)
        .expect("write wav");
}

/// Writes a WAV of digital silence, for a test that needs a source carrying no signal.
///
/// Separate from `write_tone_wav` rather than an amplitude on it: a caller asking for
/// silence is asking for a different fixture, not a quieter tone, and the two are read
/// by different assertions.
pub fn write_silence_wav(path: &std::path::Path, sample_rate: u32, secs: f64) {
    use std::io::Write as _;

    let (channels, bits) = (2u16, 16u16);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (f64::from(sample_rate) * secs) as u32;
    let block_align = u32::from(channels) * u32::from(bits / 8);
    let data_len = frames * block_align;

    let mut b: Vec<u8> = Vec::with_capacity(44 + data_len as usize);
    b.extend(b"RIFF");
    b.extend(&(36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(&16u32.to_le_bytes());
    b.extend(&1u16.to_le_bytes()); // PCM
    b.extend(&channels.to_le_bytes());
    b.extend(&sample_rate.to_le_bytes());
    b.extend(&(sample_rate * block_align).to_le_bytes());
    b.extend(&u16::try_from(block_align).unwrap_or(u16::MAX).to_le_bytes());
    b.extend(&bits.to_le_bytes());
    b.extend(b"data");
    b.extend(&data_len.to_le_bytes());
    // Silence is the zero sample for signed PCM, so the body needs no loop.
    b.resize(44 + data_len as usize, 0);

    std::fs::File::create(path)
        .expect("create wav")
        .write_all(&b)
        .expect("write wav");
}

/// The frame rate and the mean luma of every decoded video frame, or `None` where
/// this build cannot decode the file.
///
/// Where a clip's picture *starts* cannot be read from a duration any more: the
/// composition's background canvas sets the length, so a layer placed at the wrong
/// time shows black in the wrong places while the file stays exactly as long as it
/// should be. Reading the frames is the only instrument that sees the placement.
pub fn video_luma_per_frame(path: &std::path::Path) -> Option<(f64, Vec<f64>)> {
    let mut decoder = ff_decode::VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Yuv420p)
        .build()
        .ok()?;
    let fps = decoder.frame_rate();
    let mut luma = Vec::new();
    while let Ok(Some(frame)) = decoder.decode_one() {
        let stride = frame.stride(0).unwrap_or(frame.width() as usize);
        let plane = frame.plane(0)?;
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let mut sum = 0u64;
        for y in 0..h {
            let row = &plane[y * stride..y * stride + w];
            sum += row.iter().map(|&v| u64::from(v)).sum::<u64>();
        }
        luma.push(sum as f64 / (w * h) as f64);
    }
    (fps > 0.0 && !luma.is_empty()).then_some((fps, luma))
}

/// The time of the first video frame whose mean luma clears `floor`, in seconds.
///
/// The background canvas is black, so a floor between it and the fixture's own luma
/// finds where the clip's picture begins.
pub fn first_visible_secs(path: &std::path::Path, floor: f64) -> Option<f64> {
    let (fps, luma) = video_luma_per_frame(path)?;
    luma.iter().position(|&v| v > floor).map(|i| i as f64 / fps)
}

/// The peak, RMS and decoded length in seconds of a file's audio, or `None`
/// where this build cannot decode it.
///
/// A duration check alone passes on silence, so tests that care whether an
/// effect reached the signal measure level with this. The length is the decoded
/// one on purpose: a container's own duration follows its longest stream, so it
/// stays at the video length even when most of the audio never arrived.
pub fn measure_audio(path: &std::path::Path) -> Option<(f64, f64, f64)> {
    let mut decoder = ff_decode::AudioDecoder::open(path)
        .output_format(SampleFormat::F32)
        .build()
        .ok()?;
    let (mut peak, mut sum, mut count) = (0.0f64, 0.0f64, 0usize);
    let mut samples_per_channel = 0usize;
    let mut sample_rate = 0u32;
    while let Ok(Some(frame)) = decoder.decode_one() {
        samples_per_channel += frame.samples();
        sample_rate = frame.sample_rate();
        for chunk in frame.planes()[0].chunks_exact(4) {
            let v = f64::from(f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            peak = peak.max(v.abs());
            sum += v * v;
            count += 1;
        }
    }
    if count == 0 || sample_rate == 0 {
        return None;
    }
    let secs = samples_per_channel as f64 / f64::from(sample_rate);
    Some((peak, (sum / count as f64).sqrt(), secs))
}

/// The time of the first audio sample whose level clears `floor`, in seconds, or
/// `None` where this build cannot decode the file or the audio never rises above it.
///
/// Where the sound *starts* is the measurement a placement test needs, and neither
/// `measure_audio` nor a duration can give it: a clip placed late and a clip placed
/// early carry the same peak, the same RMS and the same length. The floor is a
/// parameter because the caller knows its own fixture: a synthesised tone wants a
/// value well above the codec's noise, while a quiet source wants a lower one.
pub fn first_sound_secs(path: &std::path::Path, floor: f64) -> Option<f64> {
    let mut decoder = ff_decode::AudioDecoder::open(path)
        .output_format(SampleFormat::F32)
        .build()
        .ok()?;
    let mut elapsed = 0usize;
    while let Ok(Some(frame)) = decoder.decode_one() {
        let sample_rate = frame.sample_rate();
        if sample_rate == 0 {
            return None;
        }
        let channels = frame.channels().max(1) as usize;
        for (i, chunk) in frame.planes()[0].chunks_exact(4).enumerate() {
            let v = f64::from(f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            if v.abs() > floor {
                // `output_format(SampleFormat::F32)` above forces a packed layout, so
                // `planes()[0]` interleaves every channel and the sample index advances
                // once per channel. This would be wrong for a planar format, where
                // plane 0 holds channel 0 alone while `channels` still counts them all.
                let frames_in = i / channels;
                return Some((elapsed + frames_in) as f64 / f64::from(sample_rate));
            }
        }
        elapsed += frame.samples();
    }
    None
}

/// The dominant frequency of a file's audio over a window, by zero-crossing rate.
///
/// `None` where this build cannot decode the file or the window is out of range.
/// The estimate is crude but sufficient for a single tone, and a caller should
/// measure the untouched source first as calibration: a known 440 Hz tone reads
/// about 438 Hz here, so a reading that lands far off says the measurement broke
/// rather than the audio.
pub fn dominant_hz(path: &std::path::Path, start_secs: f64, window_secs: f64) -> Option<f64> {
    let mut decoder = ff_decode::AudioDecoder::open(path)
        .output_format(SampleFormat::F32)
        .build()
        .ok()?;
    let mut samples: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    while let Ok(Some(frame)) = decoder.decode_one() {
        rate = frame.sample_rate();
        let channels = frame.channels().max(1) as usize;
        // One channel is enough for a frequency estimate, so take the first of
        // each interleaved group rather than mixing.
        for group in frame.planes()[0].chunks_exact(4 * channels) {
            samples.push(f32::from_ne_bytes([group[0], group[1], group[2], group[3]]));
        }
    }
    if rate == 0 {
        return None;
    }
    let begin = (start_secs * f64::from(rate)) as usize;
    let end = ((start_secs + window_secs) * f64::from(rate)) as usize;
    let slice = samples.get(begin..end.min(samples.len()))?;
    if slice.len() < 2 {
        return None;
    }
    let crossings = slice
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    Some(crossings as f64 / (2.0 * window_secs))
}
