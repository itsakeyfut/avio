//! Apply advanced video effects using `FilterGraph` + manual encode loop.
//!
//! Available effects:
//!   `motion-blur`  — simulate motion blur via frame blending (`tblend`)
//!   `film-grain`   — add random per-frame film grain (`noise`)
//!   `glow`         — bloom / glow around bright areas
//!   `lens-correct` — correct radial lens distortion (`lenscorrection`)
//!   `chroma-fix`   — fix lateral chromatic aberration (`rgbashift`)
//!
//! These effects are added to a `FilterGraph` after construction.  The example
//! uses a manual decode → filter → encode loop identical to `filter_direct`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example advanced_video_effects -- \
//!   --input   input.mp4  \
//!   --output  out.mp4    \
//!   --effect  film-grain
//! ```

use std::{path::Path, process};

use ff_decode::{AudioDecoder, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_filter::FilterGraph;
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
            "Usage: advanced_video_effects --input <file> --output <file> \
             --effect motion-blur|film-grain|glow|lens-correct|chroma-fix"
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

    // Probe source dimensions

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

    // Build FilterGraph and apply the chosen effect
    //
    // Effects are added to a FilterGraph after builder().build().  They queue
    // FFmpeg filter steps that are applied lazily on the first push_video call.

    let mut filter = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    match effect.as_str() {
        "motion-blur" => {
            println!("Effect:  motion-blur  (shutter=180°, sub_frames=2)");
            if let Err(e) = filter.motion_blur(180.0, 2) {
                eprintln!("motion_blur setup error: {e}");
                process::exit(1);
            }
        }
        "film-grain" => {
            println!("Effect:  film-grain  (luma=50, chroma=20)");
            filter.film_grain(50.0, 20.0);
        }
        "glow" => {
            println!("Effect:  glow  (threshold=0.8, radius=5.0, intensity=0.5)");
            filter.glow(0.8, 5.0, 0.5);
        }
        "lens-correct" => {
            println!("Effect:  lens-correct  (k1=-0.1, k2=0.0)");
            if let Err(e) = filter.lens_correction(-0.1, 0.0) {
                eprintln!("lens_correction setup error: {e}");
                process::exit(1);
            }
        }
        "chroma-fix" => {
            println!("Effect:  chroma-fix  (red_scale=1.002, blue_scale=0.998)");
            if let Err(e) = filter.fix_chromatic_aberration(1.002, 0.998) {
                eprintln!("fix_chromatic_aberration setup error: {e}");
                process::exit(1);
            }
        }
        other => {
            eprintln!(
                "Unknown effect '{other}' (try motion-blur, film-grain, glow, lens-correct, chroma-fix)"
            );
            process::exit(1);
        }
    }

    println!("Input:   {in_name}  {src_w}×{src_h}  {fps:.2} fps");
    println!("Output:  {out_name}");
    println!();

    // Build encoder

    let audio_probe = AudioDecoder::open(&input).build().ok();
    let (sample_rate, channels) = audio_probe
        .as_ref()
        .map_or((48_000, 2), |d| (d.sample_rate(), d.channels()));
    drop(audio_probe);

    let mut enc_builder = VideoEncoder::create(&output)
        .video(src_w, src_h, fps)
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

    // Video loop: decode → filter → encode

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

        match filter.push_video(0, &raw) {
            Ok(()) => {}
            Err(ff_filter::FilterError::BuildFailed) => {
                println!("Note: filter not available in this FFmpeg build — copying through.");
                if let Err(e) = encoder.push_video(&raw) {
                    eprintln!("Encode push_video error: {e}");
                    process::exit(1);
                }
                video_frames += 1;
                continue;
            }
            Err(e) => {
                eprintln!("Filter push_video error: {e}");
                process::exit(1);
            }
        }

        loop {
            match filter.pull_video() {
                Ok(Some(filtered)) => {
                    if let Err(e) = encoder.push_video(&filtered) {
                        eprintln!("Encode push_video error: {e}");
                        process::exit(1);
                    }
                    video_frames += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Filter pull_video error: {e}");
                    process::exit(1);
                }
            }
        }
    }

    // Audio pass-through

    let mut audio_frames: u64 = 0;

    if let Ok(mut adec) = AudioDecoder::open(&input).build() {
        loop {
            let raw = match adec.decode_one() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(_) => break,
            };
            if let Ok(()) = encoder.push_audio(&raw) {
                audio_frames += 1;
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
        "Done. {out_name}  {size_str}  \
         video_frames={video_frames}  audio_frames={audio_frames}"
    );
}
