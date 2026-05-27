//! Overlay two video layers onto a canvas and mix two audio tracks.
//!
//! Demonstrates [`MultiTrackComposer`] for video layer composition and
//! [`MultiTrackAudioMixer`] for audio track mixing. Both produce source-only
//! [`FilterGraph`] instances whose frames are pulled in a loop and fed directly
//! into a [`VideoEncoder`].
//!
//! The overlay layer is placed at the top-right corner at half scale.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example multi_track_compose --features filter,pipeline -- \
//!   --base    base.mp4       \
//!   --overlay overlay.mp4   \
//!   --output  composed.mp4  \
//!   [--audio-a audio_a.mp4] \
//!   [--audio-b audio_b.mp4] \
//!   [--width 1280] [--height 720] [--fps 30]
//! ```

use std::{path::PathBuf, process, time::Duration};

use avio::{
    AnimatedValue, AudioCodec, AudioTrack, ChannelLayout, MultiTrackAudioMixer, MultiTrackComposer,
    VideoCodec, VideoEncoder, VideoLayer,
};

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    base: PathBuf,
    overlay: PathBuf,
    output: PathBuf,
    audio_a: Option<PathBuf>,
    audio_b: Option<PathBuf>,
    width: u32,
    height: u32,
    fps: f64,
}

fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let get = |flag: &str| -> Option<String> {
        args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    };

    let base = if let Some(p) = get("--base") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --base <path> is required");
        process::exit(1);
    };
    let overlay = if let Some(p) = get("--overlay") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --overlay <path> is required");
        process::exit(1);
    };
    let output = if let Some(p) = get("--output") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --output <path> is required");
        process::exit(1);
    };

    Args {
        base,
        overlay,
        output,
        audio_a: get("--audio-a").map(PathBuf::from),
        audio_b: get("--audio-b").map(PathBuf::from),
        width: get("--width").and_then(|v| v.parse().ok()).unwrap_or(1280),
        height: get("--height").and_then(|v| v.parse().ok()).unwrap_or(720),
        fps: get("--fps").and_then(|v| v.parse().ok()).unwrap_or(30.0),
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // ── 1. Build video composition graph ─────────────────────────────────────

    // The overlay occupies the top-right quadrant at half the canvas width.
    let overlay_x = args.width / 2;

    let mut video_graph = match MultiTrackComposer::new(args.width, args.height)
        .add_layer(VideoLayer {
            source: args.base.clone(),
            input_format: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            z_order: 0,
            time_offset: Duration::ZERO,
            in_point: None,
            out_point: None,
            in_transition: None,
            blend_mode: ff_filter::BlendMode::Normal,
            effects: vec![],
        })
        .add_layer(VideoLayer {
            source: args.overlay.clone(),
            input_format: None,
            x: AnimatedValue::Static(f64::from(overlay_x)),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(0.5),
            scale_y: AnimatedValue::Static(0.5),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(0.85),
            z_order: 1,
            time_offset: Duration::ZERO,
            in_point: None,
            out_point: None,
            in_transition: None,
            blend_mode: ff_filter::BlendMode::Normal,
            effects: vec![],
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: failed to build video composition graph: {e}");
            process::exit(1);
        }
    };

    // ── 2. Build audio mix graph (optional) ───────────────────────────────────

    let has_audio = args.audio_a.is_some() && args.audio_b.is_some();
    let mut audio_graph = if let (Some(audio_a), Some(audio_b)) = (&args.audio_a, &args.audio_b) {
        match MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: audio_a.clone(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(-0.2), // slight left pan
                time_offset: Duration::ZERO,
                effects: vec![],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .add_track(AudioTrack {
                source: audio_b.clone(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.2), // slight right pan
                time_offset: Duration::ZERO,
                effects: vec![],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build()
        {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("error: failed to build audio mix graph: {e}");
                process::exit(1);
            }
        }
    } else {
        None
    };

    // ── 3. Create encoder ─────────────────────────────────────────────────────

    let mut enc_builder = VideoEncoder::create(&args.output)
        .video(args.width, args.height, args.fps)
        .video_codec(VideoCodec::H264);

    if has_audio {
        enc_builder = enc_builder
            .audio(48_000, 2)
            .audio_codec(AudioCodec::Aac)
            .audio_bitrate(192_000);
    }

    let mut encoder = match enc_builder.build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: failed to create encoder: {e}");
            process::exit(1);
        }
    };

    // ── 4. Drain video graph → encoder ────────────────────────────────────────

    loop {
        match video_graph.pull_video() {
            Ok(Some(frame)) => {
                if let Err(e) = encoder.push_video(&frame) {
                    eprintln!("error: push_video failed: {e}");
                    process::exit(1);
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: pull_video failed: {e}");
                process::exit(1);
            }
        }
    }

    // ── 5. Drain audio graph → encoder ────────────────────────────────────────

    if let Some(ref mut agraph) = audio_graph {
        loop {
            match agraph.pull_audio() {
                Ok(Some(frame)) => {
                    if let Err(e) = encoder.push_audio(&frame) {
                        eprintln!("error: push_audio failed: {e}");
                        process::exit(1);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("error: pull_audio failed: {e}");
                    process::exit(1);
                }
            }
        }
    }

    // ── 6. Finish ─────────────────────────────────────────────────────────────

    if let Err(e) = encoder.finish() {
        eprintln!("error: encoder.finish() failed: {e}");
        process::exit(1);
    }

    println!(
        "composed: {} → {}  ({}x{} @ {:.0}fps{})",
        args.base.display(),
        args.output.display(),
        args.width,
        args.height,
        args.fps,
        if has_audio { " + audio mix" } else { "" },
    );
}
