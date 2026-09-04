//! Bitstream filter tests for stream-copy trimming (#1602).
//!
//! Two things are covered, and they are not the same thing:
//!
//! 1. **What `FFmpeg` does on its own.** libavformat inserts the filter a container
//!    requires without being asked — a muxer's `check_bitstream` callback runs from
//!    `write_packets_common`, which both `av_write_frame` and
//!    `av_interleaved_write_frame` reach, under the default `AVFMT_FLAG_AUTO_BSF`.
//!    Copying H.264 from MP4 into MPEG-TS therefore already yields Annex B. No code
//!    in this crate makes that happen, so the test here is a **regression guard**: it
//!    fails if the packet path ever stops going through `av_interleaved_write_frame`,
//!    or if the flag is cleared. See ADR-0011.
//! 2. **What a caller has to ask for.** `extract_extradata`, `dump_extra` and the
//!    `*_metadata` family are never inserted automatically, and reaching them is what
//!    [`StreamCopyTrimmer::video_bsf`] exists for.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::Duration;

use ff_remux::{RemuxError, StreamCopyTrim, StreamCopyTrimmer};

/// An H.264 + AAC source. Committed, so its presence is not environment-dependent.
fn source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
}

fn out_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-output");
    std::fs::create_dir_all(&dir).ok();
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Is the MPEG-TS muxer in this `FFmpeg` build?
///
/// A capability probe on a *different* thing from what the test asserts (the muxer's
/// existence, not the bitstream form it produces), so a build without it skips
/// without hiding a defect in the code under test (RK-002).
fn has_mpegts_muxer() -> bool {
    let probe = out_path("bsf_probe_mpegts.ts");
    ff_sys::OutputFormatContext::new(Some("mpegts"), &probe).is_ok()
}

/// Is `name` a registered bitstream filter here?
///
/// Opened with empty parameters, so only registration is being asked about:
///
/// * `BSF_NOT_FOUND` — the build lacks the filter. A legitimate skip.
/// * `EINVAL` — the filter **is** registered but declares `codec_ids`, and
///   `av_bsf_init`'s first branch rejects the zeroed `par_in`'s `AV_CODEC_ID_NONE`.
///   `extract_extradata` and `h264_mp4toannexb` both land here, so treating this as a
///   failure would turn "present" into a hard error on the machines that have them.
/// * anything else — the wrapper is broken, and swallowing that is how a gate turns a
///   real defect into a green run (RK-002). Fail loudly.
fn has_bsf(name: &str) -> bool {
    let tb = ff_sys::AVRational { num: 1, den: 1000 };
    match ff_sys::BsfContext::open(name, None, tb) {
        Ok(_) => true,
        Err(e) if e.code() == ff_sys::error_codes::BSF_NOT_FOUND => false,
        Err(e) if e.code() == ff_sys::error_codes::EINVAL => true,
        Err(e) => panic!("probing the bitstream filter {name:?} failed unexpectedly: {e:?}"),
    }
}

#[test]
fn mp4_to_mpegts_stream_copy_should_produce_annex_b_h264() {
    if !has_mpegts_muxer() {
        println!("skipping: this FFmpeg build has no mpegts muxer");
        return;
    }
    let out = out_path("bsf_annexb.ts");
    StreamCopyTrimmer::new(source(), 0.0, 2.0, &out)
        .run()
        .expect("mp4 -> ts stream copy");

    let bytes = std::fs::read(&out).expect("read the ts output");
    assert!(!bytes.is_empty(), "the ts output must not be empty");
    // MPEG-TS is a 188-byte packet stream, each packet starting with the sync byte.
    for offset in [0usize, 188, 376] {
        assert_eq!(
            bytes[offset], 0x47,
            "byte {offset} must be the MPEG-TS sync byte"
        );
    }
    // The load-bearing assertion. In AVCC (what the MP4 source carries) the NAL units
    // are length-prefixed and these start codes do not occur at all; their presence is
    // what shows the conversion happened.
    for (label, marker) in [
        ("SPS", [0x00, 0x00, 0x00, 0x01, 0x67]),
        ("PPS", [0x00, 0x00, 0x00, 0x01, 0x68]),
    ] {
        assert!(
            contains(&bytes, &marker),
            "the ts output must carry an Annex B {label} start code"
        );
    }
}

#[test]
fn trim_should_reject_an_unknown_bitstream_filter() {
    let out = out_path("bsf_unknown.mp4");
    let result = StreamCopyTrimmer::new(source(), 0.0, 1.0, &out)
        .video_bsf("no_such_bitstream_filter")
        .run();
    let Err(RemuxError::InvalidConfig { reason }) = result else {
        panic!("an unregistered filter must fail as InvalidConfig, got {result:?}");
    };
    assert!(
        reason.contains("no_such_bitstream_filter"),
        "the error must name the offending spec, got {reason:?}"
    );
}

#[test]
fn trim_with_an_explicit_video_bsf_should_change_the_output() {
    // `dump_extra` writes the stream's extradata into the packets that lack it, so an
    // applied filter changes the payload and a silently-ignored one does not. Asserting
    // only that the run succeeds would pass either way (RK-015).
    if !has_bsf("dump_extra") {
        println!("skipping: this FFmpeg build has no dump_extra bitstream filter");
        return;
    }
    let plain = out_path("bsf_plain.mp4");
    let filtered = out_path("bsf_dump_extra.mp4");

    StreamCopyTrimmer::new(source(), 0.0, 2.0, &plain)
        .run()
        .expect("unfiltered trim");
    StreamCopyTrimmer::new(source(), 0.0, 2.0, &filtered)
        .video_bsf("dump_extra")
        .run()
        .expect("trim with dump_extra");

    let a = std::fs::read(&plain).expect("read the unfiltered output");
    let b = std::fs::read(&filtered).expect("read the filtered output");
    assert!(
        !a.is_empty() && !b.is_empty(),
        "both outputs must be written"
    );
    println!("unfiltered {} bytes, dump_extra {} bytes", a.len(), b.len());
    assert_ne!(
        a, b,
        "dump_extra must change the output; identical files mean the filter never ran"
    );
}

#[test]
fn bsf_spec_should_dispatch_video_and_audio_to_their_own_streams() {
    // `h264_mp4toannexb` declares `codec_ids = [H264]`, and `av_bsf_init`'s first
    // branch rejects a stream whose `codec_id` is not in that list. That makes it a
    // decisive probe of *where* a spec lands: applied to this file's H.264 stream it
    // initialises, applied to its AAC stream it cannot.
    //
    // Asserting instead that video- and audio-filtered outputs merely *differ* would
    // not pin anything: swapping the two arms of `BsfSpec::for_media_type` swaps which
    // file is which, and both still differ (RK-015).
    if !has_bsf("h264_mp4toannexb") {
        println!("skipping: this FFmpeg build has no h264_mp4toannexb");
        return;
    }
    let (start, end) = (Duration::ZERO, Duration::from_secs(1));

    // Also the only coverage of the `Duration`-based entry point.
    StreamCopyTrim::new(source(), start, end, out_path("bsf_dispatch_v.mp4"))
        .video_bsf("h264_mp4toannexb")
        .run()
        .expect("an H.264-only filter must reach the H.264 stream");

    let result = StreamCopyTrim::new(source(), start, end, out_path("bsf_dispatch_a.mp4"))
        .audio_bsf("h264_mp4toannexb")
        .run();
    let Err(RemuxError::InvalidConfig { reason }) = result else {
        panic!("an H.264-only filter on the AAC stream must fail, got {result:?}");
    };
    assert!(
        reason.contains("h264_mp4toannexb"),
        "the error must name the spec, got {reason:?}"
    );
}

#[test]
fn audio_bsf_should_reach_the_audio_stream() {
    // The dispatch test above proves where a spec is *routed*; this proves an audio
    // filter's output actually reaches the muxer.
    if !has_bsf("dump_extra") {
        println!("skipping: this FFmpeg build has no dump_extra bitstream filter");
        return;
    }
    let plain = out_path("bsf_audio_plain.mp4");
    let filtered = out_path("bsf_audio_filtered.mp4");

    StreamCopyTrimmer::new(source(), 0.0, 2.0, &plain)
        .run()
        .expect("unfiltered trim");
    StreamCopyTrimmer::new(source(), 0.0, 2.0, &filtered)
        .audio_bsf("dump_extra")
        .run()
        .expect("trim with an audio filter");

    let a = std::fs::read(&plain).expect("read plain");
    let b = std::fs::read(&filtered).expect("read audio-filtered");
    println!("plain {} bytes, audio-filtered {} bytes", a.len(), b.len());
    assert_ne!(a, b, "audio_bsf must change the output");
}
