//! Replace the audio track of a video file without re-encoding video.
//!
//! Uses [`AudioReplacement`] to mux the video stream from one file with the
//! first audio stream from another file using stream-copy (no decode/encode
//! cycle).  The video quality is preserved exactly and the operation is fast.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_replacement --features encode -- \
//!     --video  original.mp4  \
//!     --audio  new_audio.mp3 \
//!     --output replaced.mp4
//! ```

use std::process;

use ff_remux::AudioReplacement;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut video = None::<String>;
    let mut audio = None::<String>;
    let mut output = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--video" | "-v" => video = Some(args.next().unwrap_or_default()),
            "--audio" | "-a" => audio = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let video = video.unwrap_or_else(|| {
        eprintln!("Usage: audio_replacement --video <file> --audio <file> --output <file>");
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
    println!("Output:       {output}");
    println!();
    println!("Replacing audio track (stream-copy, no re-encode)…");

    AudioReplacement::new(&video, &audio, &output)
        .run()
        .unwrap_or_else(|e| {
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
