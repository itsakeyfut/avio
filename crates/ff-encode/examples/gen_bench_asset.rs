//! Generate `assets/test/preview_bench_1080p.mp4` — a 60-second, 1920×1080,
//! 30 fps synthetic video used as the reference asset for the `ff-preview`
//! Criterion benchmark (issue #389).
//!
//! Video codec: VP9 (LGPL-compatible; requires `gpl` feature for libx264).
//! Audio codec: AAC 128 kbps stereo 48 kHz.
//!
//! Run from the workspace root:
//! ```bash
//! cargo run --example gen_bench_asset -p ff-encode
//! ```

use std::f32::consts::TAU;
use std::path::PathBuf;

use ff_encode::{AudioCodec, BitrateMode, Preset, VideoCodec, VideoEncoder};
use ff_format::{
    AudioFrame, PixelFormat, PooledBuffer, Rational, SampleFormat, Timestamp, VideoFrame,
};

// Constants

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: f64 = 30.0;
const DURATION_SECS: u64 = 60;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const SINE_HZ: f32 = 440.0;
/// AAC encoder requires exactly 1024 samples per frame.
const AAC_FRAME_SAMPLES: usize = 1024;

const TOTAL_FRAMES: u64 = DURATION_SECS * FPS as u64; // 1800
const TOTAL_AUDIO_SAMPLES: usize = DURATION_SECS as usize * SAMPLE_RATE as usize; // 2_880_000

// Entry point

fn main() {
    let out_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/preview_bench_1080p.mp4");

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create assets/test/");
    }

    println!(
        "Generating {DURATION_SECS}s 1920×1080 {FPS}fps → {}",
        out_path.display()
    );

    let mut encoder = VideoEncoder::create(&out_path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(VideoCodec::H264) // falls back to VP9 when GPL feature absent
        .bitrate_mode(BitrateMode::Crf(18))
        .preset(Preset::Fast)
        .audio(SAMPLE_RATE, CHANNELS)
        .audio_codec(AudioCodec::Aac)
        .audio_bitrate(128_000)
        .on_progress(|p| {
            if p.frames_encoded % 300 == 0 {
                println!(
                    "  video: {} / {} frames ({:.0}%)",
                    p.frames_encoded,
                    TOTAL_FRAMES,
                    p.percent()
                );
            }
        })
        .build()
        .expect("failed to build encoder");

    let video_time_base = Rational::new(1, FPS as i32);
    let stride = WIDTH as usize * 4; // RGBA: 4 bytes per pixel

    // Push all video frames

    for frame_idx in 0..TOTAL_FRAMES {
        let t = frame_idx as f32 / TOTAL_FRAMES as f32; // 0.0 → 1.0
        let (r, g, b) = hue_to_rgb(t * 360.0);

        // Build RGBA pixel buffer: background hue + moving white scanline.
        let scanline_y =
            (frame_idx as usize * HEIGHT as usize / TOTAL_FRAMES as usize).min(HEIGHT as usize - 1);
        let mut pixels = vec![0u8; stride * HEIGHT as usize];
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let base = y * stride + x * 4;
                if y == scanline_y {
                    pixels[base] = 255;
                    pixels[base + 1] = 255;
                    pixels[base + 2] = 255;
                    pixels[base + 3] = 255;
                } else {
                    pixels[base] = r;
                    pixels[base + 1] = g;
                    pixels[base + 2] = b;
                    pixels[base + 3] = 255;
                }
            }
        }

        let video_ts = Timestamp::new(frame_idx as i64, video_time_base);
        let vframe = VideoFrame::new(
            vec![PooledBuffer::standalone(pixels)],
            vec![stride],
            WIDTH,
            HEIGHT,
            PixelFormat::Rgba,
            video_ts,
            frame_idx % 30 == 0, // I-frame every 30 frames
        )
        .expect("failed to create VideoFrame");

        encoder.push_video(&vframe).expect("push_video failed");
    }

    // Push all audio in 1024-sample chunks (AAC frame size)

    let audio_time_base = Rational::new(1, SAMPLE_RATE as i32);
    let mut sample_offset: usize = 0;

    println!("  encoding audio ({TOTAL_AUDIO_SAMPLES} samples)…");

    while sample_offset + AAC_FRAME_SAMPLES <= TOTAL_AUDIO_SAMPLES {
        let mut raw = vec![0.0f32; AAC_FRAME_SAMPLES * CHANNELS as usize];
        for i in 0..AAC_FRAME_SAMPLES {
            let t = (sample_offset + i) as f32 / SAMPLE_RATE as f32;
            let v = (TAU * SINE_HZ * t).sin() * 0.3;
            raw[i * 2] = v; // L
            raw[i * 2 + 1] = v; // R
        }

        let bytes: Vec<u8> = raw.iter().flat_map(|s| s.to_le_bytes()).collect();
        let aframe = AudioFrame::new(
            vec![bytes],
            AAC_FRAME_SAMPLES,
            CHANNELS,
            SAMPLE_RATE,
            SampleFormat::F32,
            Timestamp::new(sample_offset as i64, audio_time_base),
        )
        .expect("failed to create AudioFrame");

        encoder.push_audio(&aframe).expect("push_audio failed");
        sample_offset += AAC_FRAME_SAMPLES;
    }

    encoder.finish().expect("encoder finish failed");
    println!("Done → {}", out_path.display());

    // Also generate video-only av_sync_test_60s.mp4
    //
    // This file has NO audio so `PreviewPlayer` uses `MasterClock::System`
    // (wall-clock pacing), which is required for the A/V sync integration test
    // to measure absolute wall-time-to-PTS drift correctly.

    let sync_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/av_sync_test_60s.mp4");

    println!(
        "Generating {DURATION_SECS}s 1920×1080 {FPS}fps (video-only) → {}",
        sync_path.display()
    );

    let mut sync_encoder = VideoEncoder::create(&sync_path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Crf(18))
        .preset(Preset::Fast)
        .on_progress(|p| {
            if p.frames_encoded % 300 == 0 {
                println!(
                    "  video: {} / {} frames ({:.0}%)",
                    p.frames_encoded,
                    TOTAL_FRAMES,
                    p.percent()
                );
            }
        })
        .build()
        .expect("failed to build sync encoder");

    for frame_idx in 0..TOTAL_FRAMES {
        let t = frame_idx as f32 / TOTAL_FRAMES as f32;
        let (r, g, b) = hue_to_rgb(t * 360.0);
        let scanline_y =
            (frame_idx as usize * HEIGHT as usize / TOTAL_FRAMES as usize).min(HEIGHT as usize - 1);
        let mut pixels = vec![0u8; stride * HEIGHT as usize];
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let base = y * stride + x * 4;
                if y == scanline_y {
                    pixels[base] = 255;
                    pixels[base + 1] = 255;
                    pixels[base + 2] = 255;
                    pixels[base + 3] = 255;
                } else {
                    pixels[base] = r;
                    pixels[base + 1] = g;
                    pixels[base + 2] = b;
                    pixels[base + 3] = 255;
                }
            }
        }
        let video_ts = Timestamp::new(frame_idx as i64, video_time_base);
        let vframe = VideoFrame::new(
            vec![PooledBuffer::standalone(pixels)],
            vec![stride],
            WIDTH,
            HEIGHT,
            PixelFormat::Rgba,
            video_ts,
            frame_idx % 30 == 0,
        )
        .expect("failed to create VideoFrame");
        sync_encoder.push_video(&vframe).expect("push_video failed");
    }

    sync_encoder.finish().expect("sync encoder finish failed");
    println!("Done → {}", sync_path.display());
}

// Helpers

/// Convert a hue angle (0–360°) to an sRGB triplet (saturation = value = 1).
fn hue_to_rgb(hue: f32) -> (u8, u8, u8) {
    let h = hue % 360.0;
    let i = (h / 60.0) as u32;
    let f = h / 60.0 - i as f32;
    let (r, g, b) = match i {
        0 => (1.0, f, 0.0),
        1 => (1.0 - f, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, 1.0 - f, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, 1.0 - f),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}
