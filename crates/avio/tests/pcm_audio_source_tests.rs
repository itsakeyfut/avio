//! Regression: a PCM audio source on a timeline must render (#1812), and the
//! render must never take the process down with it (#1849).
//!
//! `av_frame_to_audio_frame` copied a packed audio plane without its channel
//! factor, so a frame declared `samples * channels` long carried only
//! `samples`. Downstream `swr_convert` was told to read the declared amount and
//! ran off the end of the buffer: libswresample returned `EINVAL` on the runs
//! where it validated, and read unmapped memory on the runs where it did not.
//! PCM is the trigger because it decodes to a packed format; AAC and MP3 decode
//! to planar and took the correct branch all along.
//!
//! The WAV fixtures are written here as raw bytes rather than encoded, so the
//! test cannot pass because avio's own encoder happens to agree with its decoder.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod fixtures;

use std::path::{Path, PathBuf};

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use fixtures::{FileGuard, make_source_file, test_output_path, write_tone_wav};

/// Renders `audio_source` on an audio track beside a short video track.
///
/// `None` means this FFmpeg build could not take part, so the caller skips: the
/// source could not be encoded, or the composition graph could not be *built*
/// because the build has no filters, which is CI's Linux FFmpeg
/// (`--disable-everything`).
///
/// Any other failure panics. The gate turns on graph *construction* only, so it
/// cannot swallow the defect under test: reverting the fix makes the render fail
/// with `FilterError::ProcessFailed`, since `AudioFrame::new` rejects the short
/// plane while the frame is being pulled, and that lands in the panic arm.
/// Measured, not assumed.
fn render_with_audio(tag: &str, audio_source: &PathBuf, out: &PathBuf) -> Option<()> {
    // Each test gets its own source file: a shared path would let one test's
    // `FileGuard` delete the file another is still reading, since the suite runs
    // at default parallelism.
    let video = test_output_path(&format!("pcm_src_video_{tag}.mp4"));
    let _gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, 30.0, 30, 80, 90, 120)?;

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video)])
        .audio_track(vec![Clip::new(audio_source)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    match timeline.render(out, EncoderConfig::builder().build()) {
        Ok(()) => Some(()),
        Err(TimelineError::Filter(
            FilterError::BuildFailed | FilterError::CompositionFailed { .. },
        )) => {
            println!("Skipping: the composition graph needs filters this build lacks");
            None
        }
        Err(e) => panic!("render failed for a reason other than a missing filter: {e}"),
    }
}

/// The RMS of the output's audio, or `None` where this build cannot decode it.
/// A duration check alone would pass on a silent file, and the defect being
/// guarded here delivered half the samples rather than none.
fn output_rms(out: &Path) -> Option<f64> {
    let mut decoder = ff_decode::AudioDecoder::open(out)
        .output_format(ff_format::SampleFormat::F32)
        .build()
        .ok()?;
    let mut sum = 0.0f64;
    let mut count = 0usize;
    while let Ok(Some(frame)) = decoder.decode_one() {
        for chunk in frame.planes()[0].chunks_exact(4) {
            let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            sum += f64::from(v) * f64::from(v);
            count += 1;
        }
    }
    (count > 0).then(|| (sum / count as f64).sqrt())
}

/// Asserts the rendered audio is not silence. The sources carry a 440 Hz tone at
/// half scale, whose RMS is about 0.35; AAC keeps it far above this floor.
fn assert_audio_is_audible(out: &Path) {
    let Some(rms) = output_rms(out) else {
        println!("Skipping the level check: cannot decode the rendered audio here");
        return;
    };
    assert!(
        rms > 0.05,
        "the rendered audio is silent or near-silent: rms={rms}"
    );
}

/// Asserts the output carries audio whose duration is close to `expected_secs`.
fn assert_audio_of_length(out: &Path, expected_secs: f64) {
    let info = avio::open(out).expect("the rendered file must probe");
    let streams = info.audio_streams();
    assert!(
        !streams.is_empty(),
        "the rendered file must carry an audio stream"
    );
    let secs = info.duration().as_secs_f64();
    assert!(
        (secs - expected_secs).abs() < 0.35,
        "expected about {expected_secs} s of output, got {secs} s"
    );
}

#[test]
fn a_pcm16_stereo_source_should_render_with_its_audio() {
    let wav = test_output_path("pcm1812_s16_stereo.wav");
    let _gw = FileGuard::new(wav.clone());
    write_tone_wav(&wav, 48_000, 2, 16, 1.0);

    let out = test_output_path("pcm1812_s16_stereo_out.mp4");
    let _go = FileGuard::new(out.clone());
    let Some(()) = render_with_audio("s16_stereo", &wav, &out) else {
        return;
    };
    assert_audio_of_length(&out, 1.0);
    assert_audio_is_audible(&out);
}

#[test]
fn a_pcm16_mono_source_should_render_with_its_audio() {
    let wav = test_output_path("pcm1812_s16_mono.wav");
    let _gw = FileGuard::new(wav.clone());
    write_tone_wav(&wav, 48_000, 1, 16, 1.0);

    let out = test_output_path("pcm1812_s16_mono_out.mp4");
    let _go = FileGuard::new(out.clone());
    let Some(()) = render_with_audio("s16_mono", &wav, &out) else {
        return;
    };
    assert_audio_of_length(&out, 1.0);
    assert_audio_is_audible(&out);
}

#[test]
fn a_pcm24_stereo_source_should_render_with_its_audio() {
    let wav = test_output_path("pcm1812_s24_stereo.wav");
    let _gw = FileGuard::new(wav.clone());
    write_tone_wav(&wav, 48_000, 2, 24, 1.0);

    let out = test_output_path("pcm1812_s24_stereo_out.mp4");
    let _go = FileGuard::new(out.clone());
    let Some(()) = render_with_audio("s24_stereo", &wav, &out) else {
        return;
    };
    assert_audio_of_length(&out, 1.0);
    assert_audio_is_audible(&out);
}

/// FLAC cannot be written by hand, so it is encoded here and the test skips where
/// this FFmpeg build has no FLAC encoder. FLAC decodes to a packed format too, so
/// it exercises the same extraction branch as the WAV cases above.
#[test]
fn a_flac_source_should_render_with_its_audio() {
    use ff_encode::{AudioCodec, AudioEncoder};
    use ff_format::{AudioFrame, SampleFormat};

    let flac = test_output_path("pcm1812_source.flac");
    let _gf = FileGuard::new(flac.clone());

    let sample_rate = 48_000u32;
    let mut encoder = match AudioEncoder::create(&flac)
        .audio(sample_rate, 2)
        .audio_codec(AudioCodec::Flac)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: no FLAC encoder in this build: {e}");
            return;
        }
    };
    // 1024 samples per frame, so 47 frames is just under a second.
    for i in 0..47i64 {
        let pts_ms = i * 1024 * 1000 / i64::from(sample_rate);
        let frame = AudioFrame::new_silent(sample_rate, 2, SampleFormat::I16, pts_ms);
        if let Err(e) = encoder.push(&frame) {
            println!("Skipping: FLAC encode failed: {e}");
            return;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: FLAC finish failed: {e}");
        return;
    }

    let out = test_output_path("pcm1812_flac_out.mp4");
    let _go = FileGuard::new(out.clone());
    let Some(()) = render_with_audio("flac", &flac, &out) else {
        return;
    };
    assert_audio_of_length(&out, 1.0);
}

/// #1849: this path used to abort the process in roughly one run in five, so a
/// single render could pass on a lucky run. Repeat it in one process.
#[test]
fn rendering_a_pcm_source_repeatedly_should_not_abort_the_process() {
    let wav = test_output_path("pcm1849_repeat.wav");
    let _gw = FileGuard::new(wav.clone());
    write_tone_wav(&wav, 48_000, 2, 16, 0.5);

    for i in 0..8 {
        let out = test_output_path(&format!("pcm1849_repeat_out_{i}.mp4"));
        let _go = FileGuard::new(out.clone());
        let Some(()) = render_with_audio(&format!("repeat_{i}"), &wav, &out) else {
            return;
        };
    }
}
