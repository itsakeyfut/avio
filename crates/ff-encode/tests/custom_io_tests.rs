//! Muxing into a caller-supplied byte sink (#1600).
//!
//! The assertions are a round trip rather than "the buffer is non-empty": MP4
//! rewrites its header once the sizes are known, and `avio_context_free` discards
//! whatever is still buffered, so a sink that is never seeked or never flushed
//! still collects plausible-looking bytes that no demuxer will open.
//!
//! Probe-gated: the suite skips when the H.264 encoder or the MP4 muxer is
//! missing, which is the state of CI's `--disable-everything` `FFmpeg` build.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Cursor, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use ff_encode::{BitrateMode, VideoCodec, VideoEncoder};
use ff_format::VideoFrame;

const W: u32 = 64;
const H: u32 = 64;
const FRAMES: usize = 10;

/// An in-memory sink whose bytes stay reachable after the encoder has taken it.
///
/// The sink itself is moved into the encoder, so a bare `Cursor` would be
/// unreadable afterwards. Sharing the cursor is also the shape a caller wanting
/// the output in memory would actually write.
#[derive(Clone)]
struct SharedSink(Arc<Mutex<Cursor<Vec<u8>>>>);

impl SharedSink {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Cursor::new(Vec::new()))))
    }

    fn bytes(&self) -> Vec<u8> {
        self.0
            .lock()
            .map_or_else(|_| Vec::new(), |c| c.get_ref().clone())
    }
}

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| std::io::Error::other("sink poisoned"))?;
        guard.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for SharedSink {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| std::io::Error::other("sink poisoned"))?;
        guard.seek(pos)
    }
}

/// A solid mid-grey frame; the content does not matter, only that it encodes.
fn frame() -> Option<VideoFrame> {
    VideoFrame::from_rgba(W, H, vec![128u8; (W * H * 4) as usize]).ok()
}

/// Encodes `FRAMES` frames into `sink`, or `None` when this environment cannot
/// encode at all (a skip, not a failure).
fn encode_into(sink: SharedSink) -> Option<()> {
    let mut encoder = VideoEncoder::create("out.mp4")
        .video(W, H, 30.0)
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(400_000))
        .output_sink(sink)
        .build()
        .ok()?;
    let f = frame()?;
    for _ in 0..FRAMES {
        encoder.push_video(&f).ok()?;
    }
    encoder.finish().ok()
}

/// The control: the same encode written to a file the ordinary way.
///
/// The comparison is against this rather than against a fixed frame count,
/// because the encoder's own flush behaviour decides how many frames come back
/// (measured: 9 of 10 pushed, on both routes). A hard-coded expectation would
/// either bake that in as if it were the sink's doing, or hide a real loss.
fn encode_to_file() -> Option<(u64, usize)> {
    let dir = std::env::temp_dir().join("avio-custom-io-tests");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("control.mp4");
    let _ = std::fs::remove_file(&path);
    let mut encoder = VideoEncoder::create(&path)
        .video(W, H, 30.0)
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(400_000))
        .build()
        .ok()?;
    let f = frame()?;
    for _ in 0..FRAMES {
        encoder.push_video(&f).ok()?;
    }
    encoder.finish().ok()?;
    let len = std::fs::metadata(&path).ok()?.len();
    let mut decoder = ff_decode::VideoDecoder::open(&path).build().ok()?;
    let mut frames = 0usize;
    while let Ok(Some(_)) = decoder.decode_one() {
        frames += 1;
    }
    let _ = std::fs::remove_file(&path);
    Some((len, frames))
}

#[test]
fn encoding_into_an_in_memory_sink_should_produce_a_decodable_stream() {
    let sink = SharedSink::new();
    if encode_into(sink.clone()).is_none() {
        return; // no H.264 encoder / no MP4 muxer here
    }

    let Some((file_len, file_frames)) = encode_to_file() else {
        return;
    };

    let bytes = sink.bytes();
    // The real assertion: those bytes are a container a demuxer opens and
    // decodes, and they are the same output the file route produces. That only
    // holds if the seek callback let the muxer rewrite its header and the tail
    // was flushed before teardown -- a truncated or never-seeked output fails
    // here, where a "non-empty" check would not.
    let mut decoder = ff_decode::VideoDecoder::from_reader(Cursor::new(bytes.clone()))
        .build()
        .expect("the sink's bytes must be a decodable stream");
    let mut decoded = 0usize;
    while let Ok(Some(_)) = decoder.decode_one() {
        decoded += 1;
    }
    println!(
        "sink: {} bytes / {decoded} frames | file: {file_len} bytes / {file_frames} frames",
        bytes.len()
    );
    assert!(file_frames > 0, "the control must decode something");
    assert_eq!(
        bytes.len() as u64,
        file_len,
        "the sink must receive the same output the file route writes"
    );
    assert_eq!(
        decoded, file_frames,
        "the sink's stream must decode to the same frames as the file's"
    );
}

#[test]
fn a_sink_should_receive_the_whole_stream_not_just_its_head() {
    // Guards the flush specifically. `avio_context_free` discards what is still
    // buffered, and the buffer is 4 KiB, so an unflushed teardown loses the tail
    // of a stream this size -- which the header rewrite alone would not reveal.
    let sink = SharedSink::new();
    if encode_into(sink.clone()).is_none() {
        return;
    }
    let bytes = sink.bytes();
    assert!(
        bytes.len() > 1024,
        "a 10-frame stream should be more than a header, got {} bytes",
        bytes.len()
    );
    // `mdat` is the payload box; its presence means more than the header reached
    // the sink.
    assert!(
        bytes.windows(4).any(|w| w == b"mdat"),
        "the muxed payload must have reached the sink"
    );
}

#[test]
fn a_sink_and_faststart_should_be_rejected_at_build_time() {
    // `movflags=+faststart` finalises by reopening the output for reading
    // (`mov_write_trailer` -> `shift_data` -> `ff_format_shift_data`, which does
    // `io_open(s, &read_pb, s->url, AVIO_FLAG_READ, ...)`). With a sink the bytes
    // never reached `s->url`. Measured before this was rejected: with no file at
    // that path `finish()` failed and left a moov-less stream in the sink; with an
    // unrelated file of that name `finish()` returned **Ok** and that file's bytes
    // were copied into the caller's sink. A silent-Ok corruption is exactly what a
    // build-time rejection has to prevent.
    let built = VideoEncoder::create("out.mp4")
        .video(W, H, 30.0)
        .video_codec(VideoCodec::H264)
        .faststart()
        .output_sink(SharedSink::new())
        .build();
    assert!(
        built.is_err(),
        "faststart with a caller-supplied sink must be rejected"
    );
}

#[test]
fn a_sink_and_a_self_managing_muxer_should_be_rejected() {
    // An `AVFMT_NOFILE` muxer (an image2 sequence, selected by the '%' in the path)
    // opens its own outputs per frame, so there is nowhere to attach a sink. The
    // alternative to rejecting is accepting the sink and never writing to it.
    //
    // The control matters here: a `%03d.png` sequence fails to build in this
    // configuration whether or not a sink is attached, so a test using it would pass
    // without ever reaching this branch (it did, until mutation injection showed the
    // branch could be deleted with everything still green).
    let dir = std::env::temp_dir().join("avio-custom-io-tests");
    let _ = std::fs::create_dir_all(&dir);
    let pattern = dir.join("frame_%03d.mp4");

    let control = VideoEncoder::create(&pattern)
        .video(W, H, 30.0)
        .video_codec(VideoCodec::H264)
        .build();
    if control.is_err() {
        return; // this environment cannot build the sequence encoder at all
    }
    drop(control);

    let built = VideoEncoder::create(&pattern)
        .video(W, H, 30.0)
        .video_codec(VideoCodec::H264)
        .output_sink(SharedSink::new())
        .build();
    assert!(
        built.is_err(),
        "a self-managing muxer with a caller-supplied sink must be rejected"
    );
}

#[test]
fn a_sink_and_two_pass_should_be_rejected_at_build_time() {
    // Pass 2 reopens the output, which a moved-in sink cannot serve. The failure
    // has to be at build time; silently writing only the second pass would be a
    // truncated file with no error.
    let built = VideoEncoder::create("out.mp4")
        .video(W, H, 30.0)
        .video_codec(VideoCodec::H264)
        .two_pass()
        .output_sink(SharedSink::new())
        .build();
    assert!(
        built.is_err(),
        "two-pass with a caller-supplied sink must be rejected"
    );
}
