//! Display media file metadata.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example probe_info -- <input_file>
//! ```
//!
//! # Example output
//!
//! ```text
//! File:     video.mp4
//! Duration: 00:02:34.560
//! Bitrate:  3 842 kbps
//! Chapters: 3
//!
//! Video streams (1):
//!   #0  H.264  1920×1080  29.97 fps  yuv420p
//!
//! Audio streams (2):
//!   #0  AAC  stereo  48000 Hz  192 kbps
//!   #1  AAC  stereo  48000 Hz  128 kbps  [jpn]
//!
//! Subtitle streams (1):
//!   #0  ASS  [eng]
//! ```

use std::{fmt::Write as _, process, time::Duration};

use ff_format::{ColorRange, Rational};

fn format_duration(d: Duration) -> String {
    let total_ms = d.as_millis();
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

fn format_bitrate(bps: u64) -> String {
    let kbps = bps / 1000;
    // Format with spaces as thousands separator (e.g. "3 842")
    let s = kbps.to_string();
    let mut chars: Vec<char> = s.chars().collect();
    let mut i = chars.len().saturating_sub(3);
    while i > 0 {
        chars.insert(i, '\u{a0}'); // narrow no-break space → readable space
        i = i.saturating_sub(3);
    }
    chars.into_iter().collect::<String>() + " kbps"
}

fn main() {
    let mut args = std::env::args();
    let _program = args.next();

    let Some(input_path) = args.next() else {
        eprintln!("Usage: probe_info <input_file>");
        process::exit(1);
    };

    let info = match ff_probe::open(&input_path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    // ── File header ──────────────────────────────────────────────────────────

    let filename = std::path::Path::new(&input_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input_path);

    println!("File:     {filename}");
    println!("Duration: {}", format_duration(info.duration()));
    if let Some(br) = info.bitrate() {
        println!("Bitrate:  {}", format_bitrate(br));
    }
    let chapter_count = info.chapter_count();
    if chapter_count > 0 {
        println!("Chapters: {chapter_count}");
    }
    if let Some(title) = info.title() {
        println!("Title:    {title}");
    }

    // ── Video streams ────────────────────────────────────────────────────────

    let video_streams = info.video_streams();
    println!("\nVideo streams ({}):", video_streams.len());
    if video_streams.is_empty() {
        println!("  (none)");
    } else {
        for v in video_streams {
            let idx = v.index();
            let codec = v.codec_name();
            let w = v.width();
            let h = v.height();
            let fps = v.fps();
            let pix_fmt = v.pixel_format();

            // frame_rate() returns Rational (numerator/denominator fraction).
            // This is more precise than fps() for rates like 30000/1001 (≈29.97).
            let frame_rate: Rational = v.frame_rate();

            // color_range() reports whether the signal is Full, Limited, or Unknown.
            let color_range: ColorRange = v.color_range();

            let mut line = format!("  #{idx}  {codec}  {w}\u{d7}{h}  {fps:.2} fps  {pix_fmt}");
            let _ = write!(
                line,
                "  fps={}/{} color_range={}",
                frame_rate.num(),
                frame_rate.den(),
                color_range.name(),
            );

            if let Some(br) = v.bitrate() {
                let _ = write!(line, "  {}", format_bitrate(br));
            }
            if v.is_hdr() {
                line.push_str("  [HDR]");
            }
            if v.is_4k() {
                line.push_str("  [4K]");
            } else if v.is_full_hd() {
                line.push_str("  [FHD]");
            } else if v.is_hd() {
                line.push_str("  [HD]");
            }

            println!("{line}");
        }
    }

    // ── Audio streams ────────────────────────────────────────────────────────

    let audio_streams = info.audio_streams();
    println!("\nAudio streams ({}):", audio_streams.len());
    if audio_streams.is_empty() {
        println!("  (none)");
    } else {
        for a in audio_streams {
            let idx = a.index();
            let codec = a.codec_name();
            let layout = a.channel_layout();
            let rate = a.sample_rate();

            let mut line = format!("  #{idx}  {codec}  {layout}  {rate} Hz");

            if let Some(br) = a.bitrate() {
                let _ = write!(line, "  {}", format_bitrate(br));
            }
            if let Some(lang) = a.language() {
                let _ = write!(line, "  [{lang}]");
            }

            println!("{line}");
        }
    }

    // ── Subtitle streams ─────────────────────────────────────────────────────

    let subtitle_streams = info.subtitle_streams();
    println!("\nSubtitle streams ({}):", subtitle_streams.len());
    if subtitle_streams.is_empty() {
        println!("  (none)");
    } else {
        for s in subtitle_streams {
            let idx = s.index();
            let codec = s.codec_name();

            let mut line = format!("  #{idx}  {codec}");

            if let Some(lang) = s.language() {
                let _ = write!(line, "  [{lang}]");
            }
            if let Some(title) = s.title()
                && !title.is_empty()
            {
                let _ = write!(line, "  \"{title}\"");
            }
            if s.is_forced() {
                line.push_str("  [forced]");
            }

            println!("{line}");
        }
    }

    // ── Chapters ─────────────────────────────────────────────────────────────

    if info.has_chapters() {
        println!("\nChapters:");
        for chapter in info.chapters() {
            let start = format_duration(chapter.start());
            let end = format_duration(chapter.end());
            let title = chapter.title().unwrap_or("(untitled)");
            println!("  {start}\u{2013}{end}  {title}");
        }
    }
}
