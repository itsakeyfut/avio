//! Apply advanced audio effects using `FilterGraph` + manual encode loop.
//!
//! Available effects:
//!   `pitch-up`     — shift pitch up by 12 semitones (one octave)
//!   `pitch-down`   — shift pitch down by 6 semitones
//!   `time-stretch` — slow down audio to half speed while preserving pitch
//!   `noise-reduce` — attenuate broadband noise by 30 dB
//!   `reverb`       — add a short reverb / echo tail
//!   `speed-up`     — double playback speed (audio + video together)
//!
//! # Usage
//!
//! ```bash
//! cargo run --example advanced_audio_effects -- \
//!   --input   input.mp4  \
//!   --output  out.mp4    \
//!   --effect  pitch-up
//! ```

use std::{path::Path, process};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_filter::{FilterGraph, NoiseType};
use ff_format::{AudioCodec, VideoCodec};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut effect = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: advanced_audio_effects --input <file> --output <file> \
             --effect pitch-up|pitch-down|time-stretch|noise-reduce|reverb|speed-up"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    // ── Probe source ──────────────────────────────────────────────────────────

    let vprobe = VideoDecoder::open(&input).build().ok();
    let (src_w, src_h, fps) = vprobe.as_ref().map_or((1280, 720, 25.0_f64), |d| {
        (d.width(), d.height(), d.frame_rate())
    });
    drop(vprobe);

    // ── Build FilterGraph and apply the chosen audio effect ───────────────────
    //
    // Audio effects are methods on FilterGraph that add FFmpeg filter steps.
    // They must be called before the first push_audio call.

    let mut filter = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    match effect.as_str() {
        "pitch-up" => {
            println!("Effect:  pitch-up  (+12 semitones = 1 octave)");
            if let Err(e) = filter.pitch_shift(12.0) {
                eprintln!("pitch_shift setup error: {e}");
                process::exit(1);
            }
        }
        "pitch-down" => {
            println!("Effect:  pitch-down  (-6 semitones)");
            if let Err(e) = filter.pitch_shift(-6.0) {
                eprintln!("pitch_shift setup error: {e}");
                process::exit(1);
            }
        }
        "time-stretch" => {
            println!("Effect:  time-stretch  (factor=0.5 — half speed)");
            if let Err(e) = filter.time_stretch(0.5) {
                eprintln!("time_stretch setup error: {e}");
                process::exit(1);
            }
        }
        "noise-reduce" => {
            println!("Effect:  noise-reduce  (White noise, -30 dB)");
            filter.noise_reduce(NoiseType::White, 30.0);
        }
        "reverb" => {
            println!("Effect:  reverb  (0.8 gain, 100 ms delay)");
            if let Err(e) = filter.reverb_echo(0.8, 0.8, &[100.0], &[0.5]) {
                eprintln!("reverb_echo setup error: {e}");
                process::exit(1);
            }
        }
        "speed-up" => {
            println!("Effect:  speed-up  (factor=2.0)");
            if let Err(e) = filter.speed_change(2.0) {
                eprintln!("speed_change setup error: {e}");
                process::exit(1);
            }
        }
        other => {
            eprintln!(
                "Unknown effect '{other}' \
                 (try pitch-up, pitch-down, time-stretch, noise-reduce, reverb, speed-up)"
            );
            process::exit(1);
        }
    }

    println!("Input:   {in_name}");
    println!("Output:  {out_name}");
    println!();

    // ── Build encoder ─────────────────────────────────────────────────────────

    let audio_probe = AudioDecoder::open(&input).build().ok();
    let (sample_rate, channels) = audio_probe
        .as_ref()
        .map_or((48_000, 2), |d| (d.sample_rate(), d.channels()));
    drop(audio_probe);

    if sample_rate == 0 {
        eprintln!("No audio stream found in {in_name}");
        process::exit(1);
    }

    let mut encoder = match VideoEncoder::create(&output)
        .video(src_w, src_h, fps)
        .video_codec(VideoCodec::H264)
        .audio(sample_rate, channels)
        .audio_codec(AudioCodec::Aac)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    println!("Encoding...");

    // ── Video pass-through ────────────────────────────────────────────────────

    let mut video_frames: u64 = 0;

    if let Ok(mut vdec) = VideoDecoder::open(&input).build() {
        loop {
            let raw = match vdec.decode_one() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(_) => break,
            };
            if let Ok(()) = encoder.push_video(&raw) {
                video_frames += 1;
            }
        }
    }

    // ── Audio loop: decode → filter → encode ─────────────────────────────────

    let mut audio_frames: u64 = 0;

    let mut adec = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening audio decoder: {e}");
            process::exit(1);
        }
    };

    loop {
        let raw = match adec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Audio decode error: {e}");
                process::exit(1);
            }
        };

        match filter.push_audio(0, &raw) {
            Ok(()) => {}
            Err(ff_filter::FilterError::BuildFailed) => {
                println!("Note: filter not available in this FFmpeg build — passing through.");
                if let Ok(()) = encoder.push_audio(&raw) {
                    audio_frames += 1;
                }
                continue;
            }
            Err(e) => {
                eprintln!("Filter push_audio error: {e}");
                process::exit(1);
            }
        }

        loop {
            match filter.pull_audio() {
                Ok(Some(filtered)) => {
                    if let Err(e) = encoder.push_audio(&filtered) {
                        eprintln!("Encode push_audio error: {e}");
                        process::exit(1);
                    }
                    audio_frames += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Filter pull_audio error: {e}");
                    process::exit(1);
                }
            }
        }
    }

    // ── Finalise ──────────────────────────────────────────────────────────────

    if let Err(e) = encoder.finish() {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

    let size_str = match std::fs::metadata(&output) {
        Ok(m) => {
            #[allow(clippy::cast_precision_loss)]
            let kb = m.len() as f64 / 1024.0;
            if kb < 1024.0 {
                format!("{kb:.0} KB")
            } else {
                format!("{:.1} MB", kb / 1024.0)
            }
        }
        Err(_) => "(unknown size)".to_string(),
    };

    println!(
        "Done. {out_name}  {size_str}  \
         video_frames={video_frames}  audio_frames={audio_frames}"
    );
}
