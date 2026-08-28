//! Add an audio track to a silent video file.
//!
//! Uses [`AudioAdder`] to mux a video stream with an audio stream from a
//! separate file using stream-copy (no re-encode).  When the audio source is
//! shorter than the video, pass `--loop` to loop the audio automatically.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_addition -- \
//!     --video  silent.mp4    \
//!     --audio  music.mp3     \
//!     --output with_audio.mp4
//!
//! # Loop short audio to cover the full video duration:
//! cargo run --example audio_addition -- \
//!     --video  silent.mp4    \
//!     --audio  short.mp3     \
//!     --output with_audio.mp4 \
//!     --loop
//! ```

use std::process;

use ff_remux::AudioAdder;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut video = None::<String>;
    let mut audio = None::<String>;
    let mut output = None::<String>;
    let mut loop_audio = false;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--video" | "-v" => video = Some(args.next().unwrap_or_default()),
            "--audio" | "-a" => audio = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--loop" | "-l" => loop_audio = true,
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let video = video.unwrap_or_else(|| {
        eprintln!("Usage: audio_addition --video <file> --audio <file> --output <file> [--loop]");
        process::exit(1);
    });
    let audio = audio.unwrap_or_else(|| {
        eprintln!("--audio is required");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    println!("Video source: {video}");
    println!("Audio source: {audio}");
    println!("Loop audio:   {loop_audio}");
    println!("Output:       {output}");
    println!();
    println!("Adding audio track (stream-copy, no re-encode)…");

    let mut adder = AudioAdder::new(&video, &audio, &output);
    if loop_audio {
        adder = adder.loop_audio();
    }

    adder.run().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    let size = match std::fs::metadata(&output) {
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

    println!("Done. {output}  {size}");
}
