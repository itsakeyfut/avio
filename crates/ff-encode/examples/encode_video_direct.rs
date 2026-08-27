//! Encode video directly using `VideoEncoder` without the pipeline abstraction.
//!
//! Demonstrates the low-level encode loop:
//! `VideoDecoder` → `VideoEncoder::create()` → `push_video()` → `finish()`.
//!
//! Also covers:
//! - `Preset` — speed/quality tradeoff (ultrafast → veryslow)
//! - `BitrateMode::Vbr` — variable bitrate with target and ceiling
//! - `CRF_MAX` — the upper-bound constant for `BitrateMode::Crf` (value: 51)
//! - `HardwareEncoder::available()` — runtime hardware encoder detection
//! - `VideoCodecEncodeExt::is_lgpl_compatible()` — codec license check
//! - `VideoEncoder::actual_video_codec()` — actual codec used after open
//! - `VideoEncoder::is_hardware_encoding()` — whether HW path is active
//!
//! # Usage
//!
//! ```bash
//! cargo run --example encode_video_direct --features "decode encode" -- \
//!   --input   input.mp4   \
//!   --output  output.mp4  \
//!   [--preset medium]     # ultrafast | faster | fast | medium | slow | slower | veryslow
//!   [--crf    23]         # CRF quality value (default: 23)
//!   [--vbr    4000000]    # VBR target bps — overrides --crf
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, CRF_MAX, HardwareEncoder, Preset, VideoCodecEncodeExt, VideoEncoder};
use ff_format::VideoCodec;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut preset_str = "medium".to_string();
    let mut crf: u32 = 23;
    let mut vbr: Option<u64> = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--preset" => preset_str = args.next().unwrap_or_else(|| "medium".to_string()),
            "--crf" => {
                let v = args.next().unwrap_or_default();
                crf = v.parse().unwrap_or(23);
            }
            "--vbr" => {
                let v = args.next().unwrap_or_default();
                vbr = v.parse().ok();
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: encode_video_direct --input <file> --output <file> \
             [--preset ultrafast|faster|fast|medium|slow|slower|veryslow] \
             [--crf N] [--vbr N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let preset = match preset_str.to_lowercase().as_str() {
        "ultrafast" => Preset::Ultrafast,
        "faster" => Preset::Faster,
        "fast" => Preset::Fast,
        "medium" => Preset::Medium,
        "slow" => Preset::Slow,
        "slower" => Preset::Slower,
        "veryslow" => Preset::Veryslow,
        other => {
            eprintln!(
                "Unknown preset '{other}' \
                 (try ultrafast, faster, fast, medium, slow, slower, veryslow)"
            );
            process::exit(1);
        }
    };

    // CRF_MAX (= 51) is the upper bound for BitrateMode::Crf.
    // Values above it are rejected by the encoder; lower = better quality.
    let crf = crf.min(CRF_MAX);

    // BitrateMode::Vbr requires both a target and a hard ceiling.
    // When --vbr is given, set the ceiling to 2× the target as a
    // reasonable starting point.
    let bmode = match vbr {
        Some(target) => BitrateMode::Vbr {
            target,
            max: target * 2,
        },
        None => BitrateMode::Crf(crf),
    };

    // ── Probe source ──────────────────────────────────────────────────────────

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };

    let width = probe.width();
    let height = probe.height();
    let fps = probe.frame_rate();
    let in_codec = probe.stream_info().codec_name().to_string();
    drop(probe);

    let out_codec = VideoCodec::H264;

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:  {in_name}  {width}×{height}  {fps:.2} fps  codec={in_codec}");

    // ── VideoCodecEncodeExt — codec licence check ─────────────────────────────
    //
    // is_lgpl_compatible() returns true for open codecs (VP9, AV1, MPEG-4, …)
    // and false for codecs with licensing obligations (H.264, H.265).

    println!(
        "Codec:  {}  lgpl_compatible={}",
        out_codec.default_extension(),
        out_codec.is_lgpl_compatible()
    );

    // ── HardwareEncoder::available() — runtime HW detection ──────────────────

    let hw_list: Vec<String> = HardwareEncoder::available()
        .iter()
        .map(|hw| format!("{hw:?}"))
        .collect();
    println!("HW encoders available: {}", hw_list.join(", "));

    let quality_str = match &bmode {
        BitrateMode::Cbr(bps) => format!("cbr={bps}"),
        BitrateMode::Crf(q) => format!("crf={q}"),
        BitrateMode::Vbr { target, max } => format!("vbr target={target} max={max}"),
    };
    println!("Output: {out_name}  {width}×{height}  preset={preset_str}  {quality_str}");
    println!();

    // ── Build encoder directly ────────────────────────────────────────────────
    //
    // VideoEncoder::create() is the low-level entry point.
    // .video()          — set output resolution and frame rate.
    // .video_codec()    — choose the codec.
    // .preset()         — speed/quality tradeoff.
    // .bitrate_mode()   — CRF, CBR, or VBR rate control.

    let mut encoder = match VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(out_codec)
        .preset(preset)
        .bitrate_mode(bmode)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    // actual_video_codec() reflects the codec FFmpeg actually opened,
    // which may differ from the requested one on some platforms.
    // is_hardware_encoding() confirms whether a hardware path is active.
    println!(
        "Encoder opened: actual_codec={}  hardware={}",
        encoder.actual_video_codec(),
        encoder.is_hardware_encoding()
    );
    println!("Encoding...");

    // ── Manual decode → encode loop ───────────────────────────────────────────

    let mut decoder = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    let mut frames: u64 = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = encoder.push_video(&frame) {
            eprintln!("Encode error: {e}");
            process::exit(1);
        }

        frames += 1;
    }

    // ── Flush and finalise ────────────────────────────────────────────────────

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

    println!("Done. {out_name}  {size_str}  {frames} frames");
}
