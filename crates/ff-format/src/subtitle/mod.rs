//! Subtitle format parser — SRT, ASS/SSA, `WebVTT`.
//!
//! Provides pure-Rust parsing for the three most common text subtitle formats.
//! Malformed events are skipped with a `log::warn!`; a file with zero valid
//! events returns [`SubtitleError::NoEvents`].
//!
//! # Example
//!
//! ```
//! use ff_format::subtitle::{SubtitleTrack, SubtitleError};
//!
//! let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n";
//! let track = SubtitleTrack::from_srt(srt).unwrap();
//! assert_eq!(track.events.len(), 1);
//! assert_eq!(track.events[0].text, "Hello world");
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

pub use crate::error::SubtitleError;

/// A single subtitle event (cue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleEvent {
    /// Sequential 0-based event index.
    pub index: usize,
    /// Presentation start time.
    pub start: Duration,
    /// Presentation end time.
    pub end: Duration,
    /// Plain text with all style/override tags stripped.
    pub text: String,
    /// Original text including any style or override tags.
    pub raw: String,
    /// Additional metadata fields (e.g. ASS `Actor`, `Style`).
    pub metadata: HashMap<String, String>,
}

/// A parsed subtitle track containing ordered events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleTrack {
    /// Ordered list of subtitle events.
    pub events: Vec<SubtitleEvent>,
    /// BCP-47 language tag when available (e.g. `"en"`, `"ja"`).
    pub language: Option<String>,
}

impl SubtitleTrack {
    /// Parse a `SubRip` (`.srt`) subtitle string.
    ///
    /// Supports multi-line cues and HTML-style tags (`<i>`, `<b>`, `<u>`).
    /// Malformed blocks are skipped with `log::warn!`.
    ///
    /// # Errors
    ///
    /// Returns [`SubtitleError::NoEvents`] when no valid events are found.
    pub fn from_srt(input: &str) -> Result<Self, SubtitleError> {
        parse_srt(input)
    }

    /// Parse an ASS/SSA subtitle string.
    ///
    /// Reads the `[Events]` section only. Override tags (`{...}`) are
    /// preserved in [`SubtitleEvent::raw`] and stripped for
    /// [`SubtitleEvent::text`]. Malformed `Dialogue:` lines are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`SubtitleError::NoEvents`] when no valid events are found.
    pub fn from_ass(input: &str) -> Result<Self, SubtitleError> {
        parse_ass(input)
    }

    /// Parse a `WebVTT` (`.vtt`) subtitle string.
    ///
    /// Cue identifiers are optional. Voice span tags (`<v Speaker>`) and
    /// other HTML tags are stripped for [`SubtitleEvent::text`]. Malformed
    /// cues are skipped with `log::warn!`.
    ///
    /// # Errors
    ///
    /// Returns [`SubtitleError::ParseError`] when the `WEBVTT` header is
    /// missing, or [`SubtitleError::NoEvents`] when no valid cues are found.
    pub fn from_vtt(input: &str) -> Result<Self, SubtitleError> {
        parse_vtt(input)
    }

    /// Serialize this track to a `SubRip` (`.srt`) string.
    ///
    /// Events are numbered sequentially starting at `1`. The `raw` field is
    /// written as the cue body so that style tags round-trip intact.
    /// Events with empty text produce a blank-line body so that the sequential
    /// index is preserved.
    ///
    /// Timestamp format: `HH:MM:SS,mmm --> HH:MM:SS,mmm`.
    #[must_use]
    pub fn to_srt(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for (seq, ev) in self.events.iter().enumerate() {
            let _ = writeln!(out, "{}", seq + 1);
            let _ = writeln!(
                out,
                "{} --> {}",
                duration_to_srt_timestamp(ev.start),
                duration_to_srt_timestamp(ev.end),
            );
            out.push_str(&ev.raw);
            out.push('\n');
            out.push('\n');
        }
        out
    }

    /// Serialize this track to an ASS/SSA string.
    ///
    /// Writes a minimal but valid file containing `[Script Info]`,
    /// `[V4+ Styles]` (one default style), and `[Events]`. The `raw` field
    /// is written as the `Text` column so that override tags round-trip intact.
    /// `Style` and `Name` metadata fields are restored from
    /// [`SubtitleEvent::metadata`] when present.
    ///
    /// Timestamp format: `H:MM:SS.cc` (centiseconds).
    #[must_use]
    pub fn to_ass(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        out.push_str("[Script Info]\n");
        out.push_str("ScriptType: v4.00+\n");
        out.push_str("PlayResX: 384\n");
        out.push_str("PlayResY: 288\n");
        out.push('\n');
        out.push_str("[V4+ Styles]\n");
        out.push_str(
            "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, \
             OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, \
             ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, \
             Alignment, MarginL, MarginR, MarginV, Encoding\n",
        );
        out.push_str(
            "Style: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,\
             &H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n",
        );
        out.push('\n');
        out.push_str("[Events]\n");
        out.push_str(
            "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        );
        for ev in &self.events {
            let style = ev.metadata.get("Style").map_or("Default", String::as_str);
            let name = ev.metadata.get("Name").map_or("", String::as_str);
            let _ = writeln!(
                out,
                "Dialogue: 0,{},{},{},{},0,0,0,,{}",
                duration_to_ass_timestamp(ev.start),
                duration_to_ass_timestamp(ev.end),
                style,
                name,
                ev.raw,
            );
        }
        out
    }

    /// Serialize this track to a `WebVTT` (`.vtt`) string.
    ///
    /// Writes the mandatory `WEBVTT` header followed by one cue per event.
    /// The `raw` field is written as the cue body so that voice span tags
    /// round-trip intact.
    ///
    /// Timestamp format: `HH:MM:SS.mmm --> HH:MM:SS.mmm`.
    #[must_use]
    pub fn to_vtt(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("WEBVTT\n");
        for ev in &self.events {
            out.push('\n');
            let _ = writeln!(
                out,
                "{} --> {}",
                duration_to_vtt_timestamp(ev.start),
                duration_to_vtt_timestamp(ev.end),
            );
            out.push_str(&ev.raw);
            out.push('\n');
        }
        out
    }

    /// Write this track to `path`, choosing the serializer by file extension.
    ///
    /// Supported extensions: `.srt`, `.ass`, `.ssa`, `.vtt`.
    ///
    /// # Errors
    ///
    /// Returns [`SubtitleError::UnsupportedFormat`] for unrecognized extensions,
    /// or [`SubtitleError::Io`] when the file cannot be written.
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<(), SubtitleError> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let content = match ext.as_str() {
            "srt" => self.to_srt(),
            "ass" | "ssa" => self.to_ass(),
            "vtt" => self.to_vtt(),
            _ => return Err(SubtitleError::UnsupportedFormat { extension: ext }),
        };

        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load and parse a subtitle file, auto-detecting the format by extension.
    ///
    /// Supported extensions: `.srt`, `.ass`, `.ssa`, `.vtt`.
    ///
    /// # Errors
    ///
    /// Returns [`SubtitleError::UnsupportedFormat`] for unrecognized extensions,
    /// [`SubtitleError::Io`] on read failure, or a format-specific error when
    /// parsing fails.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, SubtitleError> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // Validate extension before performing I/O.
        match ext.as_str() {
            "srt" | "ass" | "ssa" | "vtt" => {}
            _ => return Err(SubtitleError::UnsupportedFormat { extension: ext }),
        }

        let content = std::fs::read_to_string(path)?;

        match ext.as_str() {
            "srt" => parse_srt(&content),
            "ass" | "ssa" => parse_ass(&content),
            "vtt" => parse_vtt(&content),
            _ => unreachable!("extension validated above"),
        }
    }
}

// SRT parser

fn parse_srt(input: &str) -> Result<SubtitleTrack, SubtitleError> {
    let mut events: Vec<SubtitleEvent> = Vec::new();
    let mut current_block: Vec<String> = Vec::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current_block.is_empty() {
                if let Some(ev) = parse_srt_block(&current_block, events.len()) {
                    events.push(ev);
                }
                current_block.clear();
            }
        } else {
            current_block.push(trimmed.to_string());
        }
    }

    // Handle last block without a trailing blank line.
    if !current_block.is_empty()
        && let Some(ev) = parse_srt_block(&current_block, events.len())
    {
        events.push(ev);
    }

    if events.is_empty() {
        return Err(SubtitleError::NoEvents);
    }

    Ok(SubtitleTrack {
        events,
        language: None,
    })
}

fn parse_srt_block(block: &[String], index: usize) -> Option<SubtitleEvent> {
    // A valid block needs at least an index line and a timestamp line.
    // A missing text line produces an empty-text event (intentional for
    // round-trip preservation of sequential indices).
    if block.len() < 2 {
        log::warn!(
            "srt block has too few lines, skipping count={}",
            block.len()
        );
        return None;
    }

    // First line: 1-based sequence number.
    if block[0].parse::<usize>().is_err() {
        log::warn!(
            "srt block index is not a number, skipping value={}",
            block[0]
        );
        return None;
    }

    let Some((start, end)) = parse_srt_timestamp_line(&block[1]) else {
        log::warn!("srt malformed timestamp line, skipping line={}", block[1]);
        return None;
    };

    let raw = block[2..].join("\n");
    let text = strip_html_tags(&raw);

    Some(SubtitleEvent {
        index,
        start,
        end,
        text,
        raw,
        metadata: HashMap::new(),
    })
}

fn parse_srt_timestamp_line(line: &str) -> Option<(Duration, Duration)> {
    let mut parts = line.splitn(2, " --> ");
    let start = parse_srt_timestamp(parts.next()?.trim())?;
    let end = parse_srt_timestamp(parts.next()?.trim())?;
    Some((start, end))
}

/// Parse `HH:MM:SS,mmm` (comma or period separator) into a [`Duration`].
fn parse_srt_timestamp(s: &str) -> Option<Duration> {
    let s = s.replace(',', ".");
    let (hms_str, ms_str) = match s.split_once('.') {
        Some((h, m)) => (h, m),
        None => (s.as_str(), "0"),
    };
    let ms: u64 = ms_str.parse().ok()?;
    let hms: Vec<u64> = hms_str
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if hms.len() != 3 {
        return None;
    }
    let total_ms = hms[0] * 3_600_000 + hms[1] * 60_000 + hms[2] * 1_000 + ms;
    Some(Duration::from_millis(total_ms))
}

// ASS/SSA parser

fn parse_ass(input: &str) -> Result<SubtitleTrack, SubtitleError> {
    let mut events: Vec<SubtitleEvent> = Vec::new();
    let mut in_events = false;
    let mut format_cols: Vec<String> = Vec::new();

    for (line_no, line) in input.lines().enumerate() {
        let line = line.trim();

        if line.eq_ignore_ascii_case("[Events]") {
            in_events = true;
            continue;
        }

        // New section header ends the [Events] block.
        if line.starts_with('[') && in_events {
            break;
        }

        if !in_events {
            continue;
        }

        if let Some(rest) = line.strip_prefix("Format:") {
            format_cols = rest.split(',').map(|c| c.trim().to_string()).collect();
            continue;
        }

        let Some(rest) = line.strip_prefix("Dialogue:") else {
            continue;
        };

        if format_cols.is_empty() {
            log::warn!(
                "ass dialogue line found before Format line at line={}",
                line_no + 1
            );
            continue;
        }

        let num_cols = format_cols.len();
        let parts: Vec<&str> = rest.splitn(num_cols, ',').collect();
        if parts.len() < num_cols {
            log::warn!(
                "ass dialogue has fewer fields than format at line={}",
                line_no + 1
            );
            continue;
        }

        let col_map: HashMap<&str, &str> = format_cols
            .iter()
            .zip(parts.iter())
            .map(|(k, v)| (k.as_str(), v.trim()))
            .collect();

        let Some(start) = col_map.get("Start").and_then(|s| parse_ass_timestamp(s)) else {
            log::warn!("ass malformed start timestamp at line={}", line_no + 1);
            continue;
        };

        let Some(end) = col_map.get("End").and_then(|s| parse_ass_timestamp(s)) else {
            log::warn!("ass malformed end timestamp at line={}", line_no + 1);
            continue;
        };

        let raw = col_map.get("Text").copied().unwrap_or("").to_string();
        let text = strip_ass_tags(&raw);

        let mut metadata = HashMap::new();
        for key in &["Style", "Name", "Actor", "Layer", "Effect"] {
            if let Some(val) = col_map.get(key)
                && !val.is_empty()
            {
                metadata.insert((*key).to_string(), (*val).to_string());
            }
        }

        events.push(SubtitleEvent {
            index: events.len(),
            start,
            end,
            text,
            raw,
            metadata,
        });
    }

    if events.is_empty() {
        return Err(SubtitleError::NoEvents);
    }

    Ok(SubtitleTrack {
        events,
        language: None,
    })
}

/// Parse `H:MM:SS.cc` (centiseconds) into a [`Duration`].
fn parse_ass_timestamp(s: &str) -> Option<Duration> {
    let (hms_str, cs_str) = match s.split_once('.') {
        Some((h, c)) => (h, c),
        None => (s, "0"),
    };
    let cs: u64 = cs_str.parse().ok()?;
    let hms: Vec<u64> = hms_str
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if hms.len() != 3 {
        return None;
    }
    let total_ms = hms[0] * 3_600_000 + hms[1] * 60_000 + hms[2] * 1_000 + cs * 10;
    Some(Duration::from_millis(total_ms))
}

// WebVTT parser

fn parse_vtt(input: &str) -> Result<SubtitleTrack, SubtitleError> {
    let mut lines_iter = input.lines();

    // The first line must start with "WEBVTT".
    match lines_iter.next() {
        Some(first) if first.trim_start_matches('\u{FEFF}').starts_with("WEBVTT") => {}
        _ => {
            return Err(SubtitleError::ParseError {
                line: 1,
                reason: "WebVTT file must begin with WEBVTT".to_string(),
            });
        }
    }

    let mut events: Vec<SubtitleEvent> = Vec::new();
    let mut current_block: Vec<String> = Vec::new();

    for line in lines_iter {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current_block.is_empty() {
                if let Some(ev) = parse_vtt_block(&current_block, events.len()) {
                    events.push(ev);
                }
                current_block.clear();
            }
        } else {
            current_block.push(trimmed.to_string());
        }
    }

    // Handle last block without a trailing blank line.
    if !current_block.is_empty()
        && let Some(ev) = parse_vtt_block(&current_block, events.len())
    {
        events.push(ev);
    }

    if events.is_empty() {
        return Err(SubtitleError::NoEvents);
    }

    Ok(SubtitleTrack {
        events,
        language: None,
    })
}

fn parse_vtt_block(block: &[String], index: usize) -> Option<SubtitleEvent> {
    // Skip metadata blocks.
    let first = block[0].as_str();
    if first.starts_with("NOTE") || first.starts_with("STYLE") || first.starts_with("REGION") {
        return None;
    }

    // Find the line containing "-->".
    let Some(ts_idx) = block.iter().position(|l| l.contains("-->")) else {
        log::warn!("vtt block has no timestamp line, skipping block_start={first}");
        return None;
    };

    let Some((start, end)) = parse_vtt_timestamp_line(&block[ts_idx]) else {
        log::warn!(
            "vtt malformed timestamp line, skipping line={}",
            block[ts_idx]
        );
        return None;
    };

    if ts_idx + 1 >= block.len() {
        log::warn!("vtt cue has no text start={start:?}");
        return None;
    }

    let raw = block[ts_idx + 1..].join("\n");
    let text = strip_html_tags(&raw);

    Some(SubtitleEvent {
        index,
        start,
        end,
        text,
        raw,
        metadata: HashMap::new(),
    })
}

fn parse_vtt_timestamp_line(line: &str) -> Option<(Duration, Duration)> {
    let mut parts = line.splitn(2, " --> ");
    let start = parse_vtt_timestamp(parts.next()?.trim())?;
    // End timestamp may be followed by cue settings (e.g. `align:center`).
    let end_part = parts.next()?.trim();
    let end_str = end_part.split_whitespace().next().unwrap_or("");
    let end = parse_vtt_timestamp(end_str)?;
    Some((start, end))
}

/// Parse `HH:MM:SS.mmm` or `MM:SS.mmm` into a [`Duration`].
fn parse_vtt_timestamp(s: &str) -> Option<Duration> {
    let (hms_str, ms_str) = match s.split_once('.') {
        Some((h, m)) => (h, m),
        None => (s, "0"),
    };
    // Normalise to exactly 3 digits for milliseconds.
    let ms_padded = format!("{ms_str:0<3}");
    let ms: u64 = ms_padded[..3.min(ms_padded.len())].parse().ok()?;
    let hms: Vec<u64> = hms_str
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    let total_ms = match hms.len() {
        2 => hms[0] * 60_000 + hms[1] * 1_000 + ms,
        3 => hms[0] * 3_600_000 + hms[1] * 60_000 + hms[2] * 1_000 + ms,
        _ => return None,
    };
    Some(Duration::from_millis(total_ms))
}

// Timestamp serialisation helpers

/// Format a [`Duration`] as `HH:MM:SS,mmm` (SRT / `SubRip` style).
#[allow(clippy::cast_possible_truncation)]
fn duration_to_srt_timestamp(d: Duration) -> String {
    let total_ms = d.as_millis() as u64;
    let ms = total_ms % 1_000;
    let secs = total_ms / 1_000;
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = secs / 3_600;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// Format a [`Duration`] as `H:MM:SS.cc` (ASS centisecond style).
#[allow(clippy::cast_possible_truncation)]
fn duration_to_ass_timestamp(d: Duration) -> String {
    let total_ms = d.as_millis() as u64;
    let cs = (total_ms / 10) % 100;
    let secs = total_ms / 1_000;
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = secs / 3_600;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

/// Format a [`Duration`] as `HH:MM:SS.mmm` (`WebVTT` style).
#[allow(clippy::cast_possible_truncation)]
fn duration_to_vtt_timestamp(d: Duration) -> String {
    let total_ms = d.as_millis() as u64;
    let ms = total_ms % 1_000;
    let secs = total_ms / 1_000;
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = secs / 3_600;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

// Tag stripping helpers

/// Strip HTML-style tags (`<tag>`, `</tag>`) from `s`.
fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
}

/// Strip ASS override tags (`{...}`) and convert soft line-breaks (`\N`, `\n`).
fn strip_ass_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                in_tag = true;
                i += 1;
            }
            '}' => {
                in_tag = false;
                i += 1;
            }
            '\\' if !in_tag && i + 1 < chars.len() => match chars[i + 1] {
                'N' | 'n' => {
                    result.push('\n');
                    i += 2;
                }
                _ => {
                    result.push(chars[i]);
                    i += 1;
                }
            },
            c if !in_tag => {
                result.push(c);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // SRT

    #[test]
    fn from_srt_should_parse_single_event() {
        let input = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n";
        let track = SubtitleTrack::from_srt(input).unwrap();
        assert_eq!(track.events.len(), 1);
        let ev = &track.events[0];
        assert_eq!(ev.index, 0);
        assert_eq!(ev.start, Duration::from_millis(1_000));
        assert_eq!(ev.end, Duration::from_millis(4_000));
        assert_eq!(ev.text, "Hello world");
        assert_eq!(ev.raw, "Hello world");
    }

    #[test]
    fn from_srt_should_parse_multiline_text() {
        let input = "1\n00:00:01,000 --> 00:00:04,000\nLine one\nLine two\n\n2\n00:00:05,000 --> 00:00:07,000\nSecond\n";
        let track = SubtitleTrack::from_srt(input).unwrap();
        assert_eq!(track.events.len(), 2);
        assert_eq!(track.events[0].text, "Line one\nLine two");
        assert_eq!(track.events[1].text, "Second");
    }

    #[test]
    fn from_srt_should_strip_html_tags_preserving_raw() {
        let input = "1\n00:00:01,000 --> 00:00:04,000\n<i>Italic</i> and <b>bold</b>\n";
        let track = SubtitleTrack::from_srt(input).unwrap();
        let ev = &track.events[0];
        assert_eq!(ev.text, "Italic and bold");
        assert_eq!(ev.raw, "<i>Italic</i> and <b>bold</b>");
    }

    #[test]
    fn from_srt_should_skip_malformed_event_and_parse_rest() {
        let input = "1\n00:00:01,000 --> 00:00:04,000\nGood\n\nNOT_NUM\nbad ts\ntext\n\n2\n00:00:05,000 --> 00:00:07,000\nAlso good\n";
        let track = SubtitleTrack::from_srt(input).unwrap();
        assert_eq!(track.events.len(), 2);
        assert_eq!(track.events[0].text, "Good");
        assert_eq!(track.events[1].text, "Also good");
    }

    #[test]
    fn from_srt_should_return_no_events_for_empty_input() {
        let result = SubtitleTrack::from_srt("");
        assert!(matches!(result, Err(SubtitleError::NoEvents)));
    }

    #[test]
    fn from_srt_should_return_no_events_when_all_blocks_malformed() {
        let result = SubtitleTrack::from_srt("NOT_NUM\n00:00:01,000 --> 00:00:04,000\ntext\n");
        assert!(matches!(result, Err(SubtitleError::NoEvents)));
    }

    // ASS

    const ASS_SAMPLE: &str = "\
[Script Info]
Title: Test

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:04.00,Default,,0,0,0,,Hello {\\i1}world{\\i0}
Dialogue: 0,0:00:05.00,0:00:07.00,Default,,0,0,0,,Second line
";

    #[test]
    fn from_ass_should_parse_dialogue_events() {
        let track = SubtitleTrack::from_ass(ASS_SAMPLE).unwrap();
        assert_eq!(track.events.len(), 2);
        let ev = &track.events[0];
        assert_eq!(ev.start, Duration::from_millis(1_000));
        assert_eq!(ev.end, Duration::from_millis(4_000));
        assert!(ev.raw.contains("{\\i1}"));
        assert!(!ev.text.contains('{'));
    }

    #[test]
    fn from_ass_should_strip_override_tags_preserving_raw() {
        let input = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:04.00,Default,,0,0,0,,{\\pos(100,200)}Hello\n";
        let track = SubtitleTrack::from_ass(input).unwrap();
        let ev = &track.events[0];
        assert_eq!(ev.text, "Hello");
        assert!(ev.raw.contains("{\\pos"));
    }

    #[test]
    fn from_ass_should_populate_metadata_fields() {
        let input = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:04.00,Signs,Actor1,0,0,0,,text\n";
        let track = SubtitleTrack::from_ass(input).unwrap();
        let ev = &track.events[0];
        assert_eq!(ev.metadata.get("Style"), Some(&"Signs".to_string()));
        assert_eq!(ev.metadata.get("Name"), Some(&"Actor1".to_string()));
    }

    #[test]
    fn from_ass_should_return_no_events_for_empty_events_section() {
        let input = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
        let result = SubtitleTrack::from_ass(input);
        assert!(matches!(result, Err(SubtitleError::NoEvents)));
    }

    // VTT

    const VTT_SAMPLE: &str = "\
WEBVTT

1
00:00:01.000 --> 00:00:04.000
Hello world

00:00:05.000 --> 00:00:07.000 align:center
<v Speaker>Voice tagged text</v>
";

    #[test]
    fn from_vtt_should_parse_cues_with_and_without_identifiers() {
        let track = SubtitleTrack::from_vtt(VTT_SAMPLE).unwrap();
        assert_eq!(track.events.len(), 2);
        let ev = &track.events[0];
        assert_eq!(ev.start, Duration::from_millis(1_000));
        assert_eq!(ev.end, Duration::from_millis(4_000));
        assert_eq!(ev.text, "Hello world");
    }

    #[test]
    fn from_vtt_should_strip_voice_tags_preserving_raw() {
        let track = SubtitleTrack::from_vtt(VTT_SAMPLE).unwrap();
        let ev = &track.events[1];
        assert_eq!(ev.text, "Voice tagged text");
        assert_eq!(ev.raw, "<v Speaker>Voice tagged text</v>");
    }

    #[test]
    fn from_vtt_should_ignore_cue_settings_in_timestamp_line() {
        let track = SubtitleTrack::from_vtt(VTT_SAMPLE).unwrap();
        // Second cue has "align:center" setting — end must still parse correctly.
        assert_eq!(track.events[1].end, Duration::from_millis(7_000));
    }

    #[test]
    fn from_vtt_should_return_parse_error_for_missing_header() {
        let result = SubtitleTrack::from_vtt("not a vtt file\ncontent");
        assert!(matches!(result, Err(SubtitleError::ParseError { .. })));
    }

    #[test]
    fn from_vtt_should_return_no_events_for_empty_content() {
        let result = SubtitleTrack::from_vtt("WEBVTT\n\n");
        assert!(matches!(result, Err(SubtitleError::NoEvents)));
    }

    // from_file

    #[test]
    fn from_file_should_return_unsupported_for_unknown_extension() {
        let result = SubtitleTrack::from_file("subtitle.xyz");
        assert!(matches!(
            result,
            Err(SubtitleError::UnsupportedFormat { .. })
        ));
    }

    // timestamp helpers

    #[test]
    fn parse_srt_timestamp_should_parse_millisecond_precision() {
        let ts = parse_srt_timestamp("01:23:45,678").unwrap();
        let expected_ms = 1 * 3_600_000 + 23 * 60_000 + 45 * 1_000 + 678;
        assert_eq!(ts, Duration::from_millis(expected_ms));
    }

    #[test]
    fn parse_srt_timestamp_should_parse_zero_timestamp() {
        let ts = parse_srt_timestamp("00:00:00,000").unwrap();
        assert_eq!(ts, Duration::from_millis(0));
    }

    #[test]
    fn parse_ass_timestamp_should_parse_centisecond_precision() {
        let ts = parse_ass_timestamp("1:23:45.67").unwrap();
        let expected_ms = 1 * 3_600_000 + 23 * 60_000 + 45 * 1_000 + 670;
        assert_eq!(ts, Duration::from_millis(expected_ms));
    }

    #[test]
    fn parse_vtt_timestamp_should_accept_mm_ss_format() {
        let ts = parse_vtt_timestamp("05:30.500").unwrap();
        assert_eq!(ts, Duration::from_millis(5 * 60_000 + 30 * 1_000 + 500));
    }

    #[test]
    fn parse_vtt_timestamp_should_accept_hh_mm_ss_format() {
        let ts = parse_vtt_timestamp("01:02:03.456").unwrap();
        let expected_ms = 3_600_000 + 2 * 60_000 + 3 * 1_000 + 456;
        assert_eq!(ts, Duration::from_millis(expected_ms));
    }

    // tag stripping helpers

    #[test]
    fn strip_html_tags_should_remove_italic_bold_underline() {
        assert_eq!(strip_html_tags("<i>italic</i>"), "italic");
        assert_eq!(strip_html_tags("<b>bold</b>"), "bold");
        assert_eq!(strip_html_tags("<u>under</u>"), "under");
    }

    #[test]
    fn strip_html_tags_should_remove_voice_span() {
        assert_eq!(strip_html_tags("<v Speaker>text</v>"), "text");
    }

    #[test]
    fn strip_ass_tags_should_remove_curly_brace_overrides() {
        assert_eq!(strip_ass_tags("{\\an8}text"), "text");
        assert_eq!(strip_ass_tags("before{\\pos(100,200)}after"), "beforeafter");
    }

    #[test]
    fn strip_ass_tags_should_convert_soft_line_breaks() {
        assert_eq!(strip_ass_tags("line1\\Nline2"), "line1\nline2");
        assert_eq!(strip_ass_tags("line1\\nline2"), "line1\nline2");
    }

    // timestamp serialisation helpers

    #[test]
    fn duration_to_srt_timestamp_should_format_correctly() {
        let d = Duration::from_millis(1 * 3_600_000 + 23 * 60_000 + 45 * 1_000 + 678);
        assert_eq!(duration_to_srt_timestamp(d), "01:23:45,678");
    }

    #[test]
    fn duration_to_ass_timestamp_should_use_centiseconds() {
        let d = Duration::from_millis(1 * 3_600_000 + 23 * 60_000 + 45 * 1_000 + 670);
        assert_eq!(duration_to_ass_timestamp(d), "1:23:45.67");
    }

    #[test]
    fn duration_to_vtt_timestamp_should_format_correctly() {
        let d = Duration::from_millis(1 * 3_600_000 + 2 * 60_000 + 3 * 1_000 + 456);
        assert_eq!(duration_to_vtt_timestamp(d), "01:02:03.456");
    }

    // to_srt

    #[test]
    fn to_srt_should_produce_1_based_sequential_indices() {
        let track = SubtitleTrack {
            events: vec![
                make_event(0, 1_000, 4_000, "First"),
                make_event(1, 5_000, 7_000, "Second"),
            ],
            language: None,
        };
        let srt = track.to_srt();
        let lines: Vec<&str> = srt.lines().collect();
        assert_eq!(lines[0], "1");
        assert_eq!(lines[4], "2");
    }

    #[test]
    fn to_srt_should_use_comma_separated_timestamps() {
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "Hello")],
            language: None,
        };
        let srt = track.to_srt();
        assert!(srt.contains("00:00:01,000 --> 00:00:04,000"));
    }

    #[test]
    fn to_srt_should_write_empty_text_event_preserving_index_sequence() {
        let empty = SubtitleEvent {
            index: 1,
            start: Duration::from_millis(5_000),
            end: Duration::from_millis(7_000),
            text: String::new(),
            raw: String::new(),
            metadata: HashMap::new(),
        };
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "First"), empty],
            language: None,
        };
        let srt = track.to_srt();
        let reparsed = SubtitleTrack::from_srt(&srt).unwrap();
        // Empty-text event must survive the round-trip and keep the index intact.
        assert_eq!(reparsed.events.len(), 2);
        assert_eq!(reparsed.events[1].start, Duration::from_millis(5_000));
    }

    #[test]
    fn srt_round_trip_should_preserve_start_end_and_text() {
        let srt_in = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n2\n00:00:05,500 --> 00:00:07,250\nSecond\n\n";
        let track = SubtitleTrack::from_srt(srt_in).unwrap();
        let written = track.to_srt();
        let reparsed = SubtitleTrack::from_srt(&written).unwrap();
        assert_eq!(reparsed.events.len(), track.events.len());
        for (a, b) in track.events.iter().zip(reparsed.events.iter()) {
            assert_eq!(a.start, b.start);
            assert_eq!(a.end, b.end);
            assert_eq!(a.text, b.text);
        }
    }

    // to_ass

    #[test]
    fn to_ass_should_contain_required_sections() {
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "Hello")],
            language: None,
        };
        let ass = track.to_ass();
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("[V4+ Styles]"));
        assert!(ass.contains("[Events]"));
        assert!(ass.contains("Format: Layer, Start, End,"));
        assert!(ass.contains("Dialogue:"));
    }

    #[test]
    fn to_ass_should_use_centisecond_timestamps() {
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "Hello")],
            language: None,
        };
        let ass = track.to_ass();
        assert!(ass.contains("0:00:01.00,0:00:04.00"));
    }

    #[test]
    fn ass_round_trip_should_preserve_start_end_and_text() {
        let track = SubtitleTrack::from_ass(ASS_SAMPLE).unwrap();
        let written = track.to_ass();
        let reparsed = SubtitleTrack::from_ass(&written).unwrap();
        assert_eq!(reparsed.events.len(), track.events.len());
        for (a, b) in track.events.iter().zip(reparsed.events.iter()) {
            assert_eq!(a.start, b.start, "start mismatch");
            assert_eq!(a.end, b.end, "end mismatch");
            assert_eq!(a.text, b.text, "text mismatch");
        }
    }

    // to_vtt

    #[test]
    fn to_vtt_should_start_with_webvtt_header() {
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "Hello")],
            language: None,
        };
        let vtt = track.to_vtt();
        assert!(vtt.starts_with("WEBVTT\n"));
    }

    #[test]
    fn to_vtt_should_use_dot_separated_timestamps() {
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "Hello")],
            language: None,
        };
        let vtt = track.to_vtt();
        assert!(vtt.contains("00:00:01.000 --> 00:00:04.000"));
    }

    #[test]
    fn vtt_round_trip_should_preserve_start_end_and_text() {
        let track = SubtitleTrack::from_vtt(VTT_SAMPLE).unwrap();
        let written = track.to_vtt();
        let reparsed = SubtitleTrack::from_vtt(&written).unwrap();
        assert_eq!(reparsed.events.len(), track.events.len());
        for (a, b) in track.events.iter().zip(reparsed.events.iter()) {
            assert_eq!(a.start, b.start, "start mismatch");
            assert_eq!(a.end, b.end, "end mismatch");
            assert_eq!(a.text, b.text, "text mismatch");
        }
    }

    // write_to_file

    #[test]
    fn write_to_file_should_return_unsupported_for_unknown_extension() {
        let track = SubtitleTrack {
            events: vec![make_event(0, 1_000, 4_000, "Hello")],
            language: None,
        };
        let result = track.write_to_file("output.xyz");
        assert!(matches!(
            result,
            Err(SubtitleError::UnsupportedFormat { .. })
        ));
    }

    // helpers

    fn make_event(index: usize, start_ms: u64, end_ms: u64, text: &str) -> SubtitleEvent {
        SubtitleEvent {
            index,
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_string(),
            raw: text.to_string(),
            metadata: HashMap::new(),
        }
    }
}
