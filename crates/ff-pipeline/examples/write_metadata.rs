//! Transcode a video while embedding metadata tags and chapter markers.
//!
//! Demonstrates `PipelineBuilder::metadata()` and `.chapter()` — essential
//! for podcast distribution (ID3-style tags), video platform uploads, and
//! structured long-form content (documentaries, lectures, audiobooks).
//!
//! # Usage
//!
//! ```bash
//! cargo run --example write_metadata -- \
//!   --input    input.mp4            \
//!   --output   tagged.mp4           \
//!   [--title   "My Video"]          \
//!   [--artist  "Author Name"]       \
//!   [--year    2026]                \
//!   [--chapters "00:00:00=Intro,00:01:30=Main,00:05:00=Credits"]
//! ```
//!
//! Verify the output with:
//! ```bash
//! cargo run --example probe_info -- tagged.mp4
//! ```

use std::{path::Path, process, time::Duration};

use ff_decode::VideoDecoder;
use ff_encode::BitrateMode;
use ff_format::{AudioCodec, ChapterInfo, ChapterInfoBuilder, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline};

fn parse_time(s: &str) -> Result<Duration, String> {
    if s.contains(':') {
        let parts: Vec<&str> = s.splitn(3, ':').collect();
        if parts.len() == 3 {
            let h: u64 = parts[0]
                .parse()
                .map_err(|_| format!("invalid hours in '{s}'"))?;
            let m: u64 = parts[1]
                .parse()
                .map_err(|_| format!("invalid minutes in '{s}'"))?;
            let sec: f64 = parts[2]
                .parse()
                .map_err(|_| format!("invalid seconds in '{s}'"))?;
            let total = Duration::from_secs(h * 3600 + m * 60) + Duration::from_secs_f64(sec);
            Ok(total)
        } else {
            Err(format!("invalid time '{s}' (use HH:MM:SS)"))
        }
    } else {
        let secs: f64 = s.parse().map_err(|_| format!("invalid time '{s}'"))?;
        Ok(Duration::from_secs_f64(secs))
    }
}

fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut title = None::<String>;
    let mut artist = None::<String>;
    let mut year = None::<String>;
    let mut chapters_str = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--title" => title = Some(args.next().unwrap_or_default()),
            "--artist" => artist = Some(args.next().unwrap_or_default()),
            "--year" => year = Some(args.next().unwrap_or_default()),
            "--chapters" => chapters_str = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: write_metadata --input <file> --output <file> \
             [--title T] [--artist A] [--year Y] \
             [--chapters \"HH:MM:SS=Title,...\"]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Parse chapter markers ─────────────────────────────────────────────────
    //
    // Format: "HH:MM:SS=Title,HH:MM:SS=Title,..."
    // Each chapter's end time is the next chapter's start (last chapter ends at
    // the file's total duration, probed below).

    let raw_chapters: Vec<(Duration, String)> = if let Some(ref s) = chapters_str {
        s.split(',')
            .filter(|p| !p.is_empty())
            .map(|pair| {
                let mut parts = pair.splitn(2, '=');
                let time_str = parts.next().unwrap_or("").trim();
                let title_str = parts.next().unwrap_or("(untitled)").trim().to_string();
                let t = parse_time(time_str).unwrap_or_else(|e| {
                    eprintln!("Error parsing chapter time: {e}");
                    process::exit(1);
                });
                (t, title_str)
            })
            .collect()
    } else {
        Vec::new()
    };

    // ── Open decoder — probe source dimensions and duration ───────────────────

    let vid_dec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let src_w = vid_dec.width();
    let src_h = vid_dec.height();
    let in_codec = vid_dec.stream_info().codec_name().to_string();
    let total_duration = vid_dec.duration();

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!(
        "Input:   {in_name}  {src_w}×{src_h}  {in_codec}  {}",
        format_duration(total_duration)
    );
    println!("Output:  {out_name}");
    println!();

    // ── Print metadata that will be written ───────────────────────────────────

    let has_meta = title.is_some() || artist.is_some() || year.is_some();
    if has_meta {
        println!("Metadata:");
        if let Some(ref t) = title {
            println!("  title  = {t}");
        }
        if let Some(ref a) = artist {
            println!("  artist = {a}");
        }
        if let Some(ref y) = year {
            println!("  year   = {y}");
        }
        println!();
    }

    // ── Build ChapterInfo list ────────────────────────────────────────────────
    //
    // Chapter end = next chapter's start; last chapter ends at total_duration.

    let chapters: Vec<ChapterInfo> = raw_chapters
        .iter()
        .enumerate()
        .map(|(i, (start, ch_title))| {
            let end = raw_chapters
                .get(i + 1)
                .map_or(total_duration, |(next_start, _)| *next_start);
            #[allow(clippy::cast_possible_wrap)]
            let id = i as i64;
            // ChapterInfo::builder() returns ChapterInfoBuilder — name it
            // explicitly so the type is visible to readers of this example.
            let builder: ChapterInfoBuilder = ChapterInfo::builder()
                .id(id)
                .title(ch_title.clone())
                .start(*start)
                .end(end);
            builder.build()
        })
        .collect();

    if !chapters.is_empty() {
        println!("Chapters ({}):", chapters.len());
        for ch in &chapters {
            let t = ch.title().unwrap_or("(untitled)");
            println!(
                "  {}–{}  {t}",
                format_duration(ch.start()),
                format_duration(ch.end())
            );
        }
        println!();
    }

    // ── Build pipeline with metadata and chapters ─────────────────────────────

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .bitrate_mode(BitrateMode::Crf(23))
        .build();

    let mut builder = Pipeline::builder().input(&input).output(&output, config);

    if let Some(ref t) = title {
        builder = builder.metadata("title", t);
    }
    if let Some(ref a) = artist {
        builder = builder.metadata("artist", a);
    }
    if let Some(ref y) = year {
        builder = builder.metadata("date", y);
    }
    for ch in chapters {
        builder = builder.chapter(ch);
    }

    if let Err(e) = builder
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        })
        .run()
    {
        eprintln!("Error: {e}");
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

    println!("Done. {out_name}  {size_str}");
    println!("Verify with: cargo run --example probe_info -- {out_name}");
}
