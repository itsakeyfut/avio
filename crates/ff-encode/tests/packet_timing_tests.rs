//! Packet timing at the muxer boundary.
//!
//! An encoder hands back packets with no `duration` and timestamps in its own
//! time base. Both have to be dealt with before the packet is written, or the
//! container ends one frame short (#1810) and every timestamp is wrong wherever
//! the stream's time base differs from the codec's (#1807).
//!
//! The round-trip test doubles as the regression for #1836: reading a file back
//! only returns every frame if the decoder drains itself after the demuxer ends,
//! and the condition that exposes it (a reorder buffer holding more than one
//! frame at EOF) is what an ordinary CRF encode produces.
//!
//! These tests assert against the container, never against its `nb_frames`:
//! that field reports what was written rather than what can be read back, and
//! it is what made the first two attributions of #1810 wrong.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

mod fixtures;
use fixtures::{FileGuard, test_output_path};

use ff_encode::{AudioCodec, AudioEncoder, BitrateMode, VideoCodec, VideoEncoder};
use ff_format::{AudioFrame, PixelFormat, PooledBuffer, SampleFormat, Timestamp, VideoFrame};

const FPS: f64 = 30.0;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;

/// A flat YUV420P frame whose luma carries `marker`, so a decoded frame can be
/// matched back to the frame that was written.
fn marker_frame(marker: u8) -> VideoFrame {
    let y_size = (WIDTH * HEIGHT) as usize;
    let uv_size = ((WIDTH / 2) * (HEIGHT / 2)) as usize;
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(vec![marker; y_size]),
            PooledBuffer::standalone(vec![128; uv_size]),
            PooledBuffer::standalone(vec![128; uv_size]),
        ],
        vec![WIDTH as usize, (WIDTH / 2) as usize, (WIDTH / 2) as usize],
        WIDTH,
        HEIGHT,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("frame construction should succeed")
}

/// The luma written for frame `i`, spaced so quantisation cannot confuse two.
fn marker_for(i: usize) -> u8 {
    (16 + i * 16) as u8
}

/// Writes `n` marker frames of `codec` to `path` at `fps`.
///
/// Returns `false` only when this FFmpeg build has no encoder for `codec`,
/// which is the skip condition: CI's Linux FFmpeg builds only a handful of
/// encoders. Once the encoder exists, a write failure is a test failure and not
/// a skip, because bad timestamps are exactly what makes the muxer refuse a
/// packet: swallowing that would hide the defect these tests exist for.
fn encode_at(path: &Path, codec: VideoCodec, fps: f64, n: usize) -> bool {
    let Ok(mut encoder) = VideoEncoder::create(path)
        .video(WIDTH, HEIGHT, fps)
        .video_codec(codec)
        .bitrate_mode(BitrateMode::Crf(18))
        .build()
    else {
        return false;
    };
    for i in 0..n {
        encoder
            .push_video(&marker_frame(marker_for(i)))
            .expect("the encoder accepted the frame rate, so it must accept the frames");
    }
    encoder
        .finish()
        .expect("the muxer must accept the packets the encoder produced");
    true
}

/// [`encode_at`] at the default [`FPS`].
fn encode(path: &Path, codec: VideoCodec, n: usize) -> bool {
    encode_at(path, codec, FPS, n)
}

/// The duration one frame occupies at [`FPS`], in seconds.
fn frame_period() -> f64 {
    1.0 / FPS
}

#[test]
fn mp4_duration_should_cover_every_frame_written() {
    for n in 1..=5usize {
        let path = test_output_path(&format!("timing_duration_{n}.mp4"));
        let _guard = FileGuard::new(path.clone());
        if !encode(&path, VideoCodec::Mpeg4, n) {
            println!("skipped: no mpeg4 encoder in this build");
            return;
        }

        let info = ff_probe::open(&path).expect("output should be probeable");
        let seconds = info.duration().as_secs_f64();
        let expected = n as f64 * frame_period();

        // Half a frame of slack absorbs the container's own timestamp rounding.
        // A frame that was written but left out of the duration is a whole frame
        // away, so it cannot hide inside this.
        assert!(
            (seconds - expected).abs() < frame_period() / 2.0,
            "wrote {n} frames at {FPS} fps, so the container should last {expected:.4}s, \
             but it reports {seconds:.4}s"
        );
    }
}

#[test]
fn a_fractional_frame_rate_should_not_round_the_duration_away() {
    // The per-frame length is built as `1000 / (fps * 1000)` precisely so that
    // 29.97 does not become 29. At 30 frames that difference is a whole frame,
    // so this catches the rounding the implementation was written to avoid.
    const NTSC: f64 = 30000.0 / 1001.0;
    let n = 30usize;
    let path = test_output_path("timing_ntsc.mp4");
    let _guard = FileGuard::new(path.clone());
    if !encode_at(&path, VideoCodec::Mpeg4, NTSC, n) {
        println!("skipped: no mpeg4 encoder in this build");
        return;
    }

    let info = ff_probe::open(&path).expect("output should be probeable");
    let seconds = info.duration().as_secs_f64();
    let expected = n as f64 / NTSC;
    assert!(
        (seconds - expected).abs() < (1.0 / NTSC) / 2.0,
        "wrote {n} frames at {NTSC} fps, so the container should last {expected:.4}s, \
         but it reports {seconds:.4}s"
    );
}

#[test]
fn matroska_should_report_the_requested_frame_rate() {
    let n = 4usize;
    let path = test_output_path("timing_rate.mkv");
    let _guard = FileGuard::new(path.clone());
    if !encode(&path, VideoCodec::Mpeg4, n) {
        println!("skipped: no mpeg4 encoder, or this build cannot write Matroska");
        return;
    }

    let info = ff_probe::open(&path).expect("output should be probeable");
    let stream = info
        .video_streams()
        .first()
        .cloned()
        .expect("the output should carry a video stream");

    // Matroska fixes its time base at 1/1000, so a stream written without
    // rescaling reads back at 1 fps. The tolerance covers that time base's
    // rounding over a short clip, not a factor-of-thirty error.
    assert!(
        (stream.fps() - FPS).abs() < 1.0,
        "asked for {FPS} fps and the container reports {}",
        stream.fps()
    );

    let seconds = info.duration().as_secs_f64();
    let expected = n as f64 * frame_period();
    assert!(
        (seconds - expected).abs() < frame_period(),
        "wrote {n} frames at {FPS} fps, so the container should last {expected:.4}s, \
         but it reports {seconds:.4}s"
    );
}

#[test]
fn mp4_round_trip_should_return_every_frame_with_its_marker() {
    for n in 1..=13usize {
        let path = test_output_path(&format!("timing_round_trip_{n}.mp4"));
        let _guard = FileGuard::new(path.clone());

        // H.264 rather than mpeg4: this test has to read its own output back,
        // and a build can have one codec's encoder without its decoder.
        if !encode(&path, VideoCodec::H264, n) {
            println!("skipped: no H.264 encoder in this build");
            return;
        }

        let Ok(mut decoder) = ff_decode::VideoDecoder::open(&path)
            .output_format(PixelFormat::Yuv420p)
            .build()
        else {
            println!("skipped: this build cannot decode what it just wrote");
            return;
        };

        let mut markers = Vec::new();
        while let Some(frame) = decoder.decode_one().expect("decoding should not fail") {
            markers.push(frame.planes()[0].as_ref()[0]);
            // A decoder that reports the end of the stream while it is still
            // handing frames back has told its caller to stop too early (#1836).
            assert!(
                !decoder.is_eof(),
                "the decoder reported end of stream after returning frame {}",
                markers.len()
            );
        }
        assert!(
            decoder.is_eof(),
            "the decoder returned None without reporting end of stream"
        );

        assert_eq!(
            markers.len(),
            n,
            "wrote {n} frames and read back {}: {markers:?}",
            markers.len()
        );
        for (i, decoded) in markers.iter().enumerate() {
            let expected = marker_for(i);
            assert!(
                decoded.abs_diff(expected) <= 4,
                "frame {i} came back carrying {decoded}, not the {expected} it was written with: \
                 {markers:?}"
            );
        }
    }
}

// ============================================================================
// Audio
// ============================================================================

const SAMPLE_RATE: u32 = 48_000;
/// AAC's fixed frame size, so the expected duration can be stated exactly.
const SAMPLES_PER_FRAME: usize = 1024;

/// Silence, one encoder frame long.
fn audio_frame() -> AudioFrame {
    AudioFrame::empty(SAMPLES_PER_FRAME, 2, SAMPLE_RATE, SampleFormat::F32)
        .expect("frame construction should succeed")
}

/// How far a probed audio duration may sit from the requested one.
///
/// Three encoder frames. An encoder is entitled to emit a frame of priming
/// ahead of the signal, so the tolerance cannot be tight; it does not need to
/// be, because the defect is a time base read as the wrong unit, which puts the
/// duration out by a factor of tens.
fn audio_tolerance() -> f64 {
    3.0 * SAMPLES_PER_FRAME as f64 / SAMPLE_RATE as f64
}

fn expected_audio_seconds(frames: usize) -> f64 {
    (frames * SAMPLES_PER_FRAME) as f64 / SAMPLE_RATE as f64
}

#[test]
fn standalone_audio_duration_should_match_what_was_pushed() {
    for ext in ["mp4", "mkv"] {
        let frames = 10usize;
        let path = test_output_path(&format!("timing_audio_only.{ext}"));
        let _guard = FileGuard::new(path.clone());

        let Ok(mut encoder) = AudioEncoder::create(&path)
            .audio(SAMPLE_RATE, 2)
            .audio_codec(AudioCodec::Aac)
            .build()
        else {
            println!("skipped: no AAC encoder, or this build cannot write .{ext}");
            continue;
        };
        for _ in 0..frames {
            encoder
                .push(&audio_frame())
                .expect("the encoder accepted the configuration, so it must accept the frames");
        }
        encoder
            .finish()
            .expect("the muxer must accept the packets the encoder produced");

        let info = ff_probe::open(&path).expect("output should be probeable");
        let seconds = info.duration().as_secs_f64();
        let expected = expected_audio_seconds(frames);
        assert!(
            (seconds - expected).abs() < audio_tolerance(),
            "pushed {expected:.4}s of audio into .{ext} and the container reports {seconds:.4}s"
        );
    }
}

#[test]
fn muxed_audio_duration_should_match_what_was_pushed() {
    // The audio drain inside VideoEncoder is a third code path, separate from
    // both the video drain and the standalone AudioEncoder.
    let frames = 10usize;
    let path = test_output_path("timing_muxed.mkv");
    let _guard = FileGuard::new(path.clone());

    let Ok(mut encoder) = VideoEncoder::create(&path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Crf(18))
        .audio(SAMPLE_RATE, 2)
        .audio_codec(AudioCodec::Aac)
        .build()
    else {
        println!("skipped: this build cannot encode mpeg4 video with AAC audio");
        return;
    };

    // Roughly the same span of video alongside the audio, so a wrong audio time
    // base cannot hide behind the video track's duration.
    let video_frames = (expected_audio_seconds(frames) * FPS).round() as usize;
    for i in 0..video_frames {
        encoder
            .push_video(&marker_frame(marker_for(i.min(13))))
            .expect("the encoder must accept the frames");
    }
    for _ in 0..frames {
        encoder
            .push_audio(&audio_frame())
            .expect("the encoder must accept the frames");
    }
    encoder
        .finish()
        .expect("the muxer must accept the packets the encoder produced");

    let info = ff_probe::open(&path).expect("output should be probeable");
    let seconds = info.duration().as_secs_f64();
    let expected = expected_audio_seconds(frames);
    assert!(
        (seconds - expected).abs() < audio_tolerance(),
        "pushed {expected:.4}s of audio and video, and the container reports {seconds:.4}s"
    );
}
