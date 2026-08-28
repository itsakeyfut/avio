//! Apply a filter graph directly, managing the push/pull loop without Pipeline.
//!
//! Demonstrates the low-level filter graph API:
//! - `FilterGraph::push_video()` / `pull_video()` — feed and drain video frames
//! - `FilterGraph::push_audio()` / `pull_audio()` — feed and drain audio frames
//!
//! The `Pipeline` type manages these calls internally; this example shows the
//! underlying loop for callers who need frame-level control (e.g. custom
//! schedulers, real-time processing, or mixing multiple sources manually).
//!
//! # Usage
//!
//! ```bash
//! cargo run --example filter_direct -- \
//!   --input  input.mp4  \
//!   --output output.mp4
//! ```

use std::{path::Path, process};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_filter::FilterGraphBuilder;
use ff_format::{AudioCodec, VideoCodec};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: filter_direct --input <file> --output <file>");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
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

    // Probe source

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening video: {e}");
            process::exit(1);
        }
    };
    let src_w = probe.width();
    let src_h = probe.height();
    let fps = probe.frame_rate();
    drop(probe);

    let audio_probe = AudioDecoder::open(&input).build().ok();
    let (sample_rate, channels) = audio_probe
        .as_ref()
        .map_or((48_000, 2), |d| (d.sample_rate(), d.channels()));
    drop(audio_probe);

    // Scale to 1280×720 (or source size if smaller)
    let out_w = src_w.min(1280);
    let out_h = src_h.min(720);

    println!("Input:   {in_name}  {src_w}×{src_h}  {fps:.2} fps");
    println!("Output:  {out_name}  {out_w}×{out_h}  scale + volume(0.8)");
    println!();

    // Build filter graph
    //
    // FilterGraphBuilder chains filter operations. The resulting FilterGraph
    // exposes push_video/pull_video for the video path and push_audio/pull_audio
    // for the audio path on the same graph object.

    let mut filter = match FilterGraphBuilder::new()
        .scale(out_w, out_h, ff_filter::ScaleAlgorithm::Fast)
        .volume(0.8)
        .build()
    {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    // Build encoder

    let mut enc_builder = VideoEncoder::create(&output)
        .video(out_w, out_h, fps)
        .video_codec(VideoCodec::H264);

    if sample_rate > 0 {
        enc_builder = enc_builder
            .audio(sample_rate, channels)
            .audio_codec(AudioCodec::Aac);
    }

    let mut encoder = match enc_builder.build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    println!("Encoding...");

    // Video loop: decode → push_video → pull_video → encode
    //
    // push_video(slot, frame) feeds a raw frame into filter slot 0.
    // pull_video() drains one filtered frame; returns None when the filter
    // is buffering (e.g. between inputs) — keep feeding until Some is returned.

    let mut vdec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening video decoder: {e}");
            process::exit(1);
        }
    };

    let mut video_frames: u64 = 0;

    loop {
        let raw = match vdec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Video decode error: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = filter.push_video(0, &raw) {
            eprintln!("Filter push_video error: {e}");
            process::exit(1);
        }

        // pull_video may return None if the filter needs more input before
        // producing output — loop until drained.
        loop {
            match filter.pull_video() {
                Ok(Some(filtered)) => {
                    if let Err(e) = encoder.push_video(&filtered) {
                        eprintln!("Encode push_video error: {e}");
                        process::exit(1);
                    }
                    video_frames += 1;
                }
                Ok(None) => break, // filter needs more input
                Err(e) => {
                    eprintln!("Filter pull_video error: {e}");
                    process::exit(1);
                }
            }
        }
    }

    // Audio loop: decode → push_audio → pull_audio → encode
    //
    // push_audio(slot, frame) feeds a raw audio frame into the filter graph.
    // pull_audio() drains one filtered audio frame.
    // The same filter graph object handles both video and audio paths.

    let mut audio_frames: u64 = 0;

    if let Ok(mut adec) = AudioDecoder::open(&input).build() {
        loop {
            let raw = match adec.decode_one() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Audio decode error: {e}");
                    break;
                }
            };

            if let Err(e) = filter.push_audio(0, &raw) {
                eprintln!("Filter push_audio error: {e}");
                break;
            }

            loop {
                match filter.pull_audio() {
                    Ok(Some(filtered)) => {
                        if let Err(e) = encoder.push_audio(&filtered) {
                            eprintln!("Encode push_audio error: {e}");
                            break;
                        }
                        audio_frames += 1;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("Filter pull_audio error: {e}");
                        break;
                    }
                }
            }
        }
    }

    // Finalise

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
        "Done. {out_name}  {size_str}  video_frames={video_frames}  audio_frames={audio_frames}"
    );
}
