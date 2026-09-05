//! Filter-description escape hatch (#1601).
//!
//! `parse_desc` takes a whole libavfilter description — the `ffmpeg -vf` syntax —
//! and splices it into the chain with `avfilter_graph_parse2`. It exists for the
//! two things the linear `FilterStep` list cannot do: carry a chain as a single
//! string, and describe a graph that branches and rejoins. A *single* untyped
//! filter is already served by `raw_filter`, which is why nothing here duplicates
//! that case.
//!
//! # Gating
//!
//! CI's Linux `FFmpeg` is built `--disable-everything`, so `scale`/`hue`/`split`
//! are absent there (RK-002). Each test gates on the filters its description
//! names, probed through `raw_filter` — the *single-filter* escape hatch, a
//! different mechanism from the `parse2` path under test — so the gate cannot
//! mask a defect in what is being asserted.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ff_filter::{FilterError, FilterGraph, ScaleAlgorithm};
use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

/// `YUV420p` frame with explicit luma and chroma values.
fn make_yuv_frame(width: u32, height: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    let y_plane = vec![y; (width * height) as usize];
    let u_plane = vec![u; ((width / 2) * (height / 2)) as usize];
    let v_plane = vec![v; ((width / 2) * (height / 2)) as usize];
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y_plane),
            PooledBuffer::standalone(u_plane),
            PooledBuffer::standalone(v_plane),
        ],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

/// A strongly saturated source: chroma far from the neutral 128, so a `hue=s=0`
/// that did not run is visible in the output rather than indistinguishable.
fn saturated_frame() -> VideoFrame {
    make_yuv_frame(64, 64, 128, 200, 60)
}

/// Does this `FFmpeg` build have the one-in / one-out filter `name`?
///
/// Probed through `raw_filter`, which reaches the filter by name via
/// `avfilter_graph_create_filter` — not through `avfilter_graph_parse2`. The
/// registration check is deferred to the push, so the push is what answers.
fn has_filter(name: &str, args: &str) -> bool {
    let Ok(mut graph) = FilterGraph::builder().raw_filter(name, args).build() else {
        return false;
    };
    graph.push_video(0, &saturated_frame()).is_ok()
}

/// Mean of the `w` x `h` valid region of `plane`, skipping any stride padding.
fn plane_mean(frame: &VideoFrame, plane: usize, w: usize, h: usize) -> f64 {
    let data = frame.plane(plane).expect("plane must exist");
    let stride = frame.stride(plane).expect("stride must exist");
    let mut sum = 0.0_f64;
    for row in 0..h {
        let start = row * stride;
        sum += data[start..start + w]
            .iter()
            .map(|&b| f64::from(b))
            .sum::<f64>();
    }
    #[allow(clippy::cast_precision_loss)]
    {
        sum / (w * h) as f64
    }
}

/// Asserts the frame's chroma is neutral, i.e. `hue=s=0` ran.
fn assert_desaturated(frame: &VideoFrame, w: u32, h: u32) {
    assert_eq!(
        frame.format(),
        PixelFormat::Yuv420p,
        "the plane arithmetic below assumes yuv420p output"
    );
    let (cw, ch) = ((w as usize).div_ceil(2), (h as usize).div_ceil(2));
    for (plane, label) in [(1usize, "U"), (2, "V")] {
        let mean = plane_mean(frame, plane, cw, ch);
        assert!(
            (mean - 128.0).abs() < 1.0,
            "the {label} plane must be neutral after hue=s=0, mean was {mean}"
        );
    }
}

#[test]
fn parse_desc_should_build_a_working_graph_from_a_chain_description() {
    // The whole point of taking a chain as one string: both filters must run.
    // Asserting only that a frame came out would pass if the parser had dropped
    // everything after the first comma, so each filter gets its own evidence —
    // the output size proves `scale`, the neutral chroma proves `hue` (RK-015).
    if !has_filter("scale", "64:48") || !has_filter("hue", "s=0") {
        println!("skipping: this FFmpeg build lacks scale or hue");
        return;
    }
    let mut graph =
        FilterGraph::parse_desc("scale=64:48,hue=s=0").expect("a valid description must build");
    graph
        .push_video(0, &saturated_frame())
        .expect("FFmpeg must accept the parsed chain");
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected a frame out of the parsed chain");

    assert_eq!(out.width(), 64, "scale=64:48 must set the width");
    assert_eq!(out.height(), 48, "scale=64:48 must set the height");
    assert_desaturated(&out, 64, 48);
}

#[test]
fn parse_desc_should_accept_a_branching_description() {
    // The capability that justifies going through `avfilter_graph_parse2` rather
    // than splitting the string into `FilterStep::Raw` steps: a description that
    // branches and rejoins is not a linear chain and cannot be expressed as a
    // step list at all. `overlay` puts the desaturated branch on top at full
    // opacity, so a neutral output also shows the branches were wired the way the
    // labels say.
    //
    // Gated on `hue`, which stands in for "this build has filters at all". A
    // build carrying `hue` but not `split`/`overlay` fails here rather than
    // skipping, which is the right way round for a configuration nobody has seen.
    if !has_filter("hue", "s=0") {
        println!("skipping: this FFmpeg build lacks hue");
        return;
    }
    let mut graph = FilterGraph::parse_desc("split[a][b];[a]hue=s=0[c];[b][c]overlay")
        .expect("a valid branching description must build");
    graph
        .push_video(0, &saturated_frame())
        .expect("FFmpeg must accept the parsed branching graph");
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected a frame out of the branching description");

    assert_eq!(out.width(), 64, "overlay must not change the width");
    assert_eq!(out.height(), 64, "overlay must not change the height");
    assert_desaturated(&out, 64, 64);
}

#[test]
fn parse_desc_should_compose_with_typed_steps_in_one_chain() {
    // #1601's actual objective: reach an untyped filter "without abandoning the
    // typed API". The typed `scale` and the parsed `hue` must both be in the same
    // graph, in the order they were declared.
    if !has_filter("scale", "64:48") || !has_filter("hue", "s=0") {
        println!("skipping: this FFmpeg build lacks scale or hue");
        return;
    }
    let mut graph = FilterGraph::builder()
        .scale(64, 48, ScaleAlgorithm::Fast)
        .parse_desc("hue=s=0")
        .build()
        .expect("a typed step followed by a description must build");
    graph
        .push_video(0, &saturated_frame())
        .expect("FFmpeg must accept the mixed graph");
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected a frame out of the mixed graph");

    assert_eq!(out.width(), 64, "the typed scale step must have run");
    assert_eq!(out.height(), 48, "the typed scale step must have run");
    assert_desaturated(&out, 64, 48);
}

#[test]
fn parse_desc_should_reject_an_unknown_filter_at_build_time() {
    // The description is an unchecked string, so `build()` parses it rather than
    // letting a typo survive to the first frame. This is the guard for that
    // decision: with the eager check removed, `build()` returns Ok and the error
    // only appears on push.
    if !has_filter("null", "") {
        println!("skipping: this FFmpeg build has no filters registered");
        return;
    }
    let result = FilterGraph::parse_desc("no_such_filter_xyz");
    let Err(FilterError::InvalidConfig { reason }) = result else {
        panic!("an unknown filter must fail as InvalidConfig at build time, got {result:?}");
    };
    assert!(
        reason.contains("no_such_filter_xyz"),
        "the error must name the offending description, got {reason:?}"
    );
}

#[test]
fn parse_desc_should_reject_a_bad_option_at_build_time() {
    // `avfilter_graph_parse2` applies options while parsing, so a description is
    // checked further at `build()` than the docs first claimed: option names and
    // any value `av_opt_set` or expression evaluation rejects fail there too.
    //
    // The `raw_filter` half is what makes this non-vacuous. It is the same filter
    // and the same bad option through the *single-filter* path, and it builds
    // fine — so this test pins a real difference between the two escape hatches
    // rather than restating "bad input errors".
    if !has_filter("hue", "s=0") {
        println!("skipping: this FFmpeg build lacks hue");
        return;
    }
    for desc in ["hue=nosuchopt=1", "hue=s=notanumber"] {
        let result = FilterGraph::parse_desc(desc);
        let Err(FilterError::InvalidConfig { reason }) = result else {
            panic!("a bad option in {desc:?} must fail at build time, got {result:?}");
        };
        assert!(
            reason.contains(desc),
            "the error must name the offending description, got {reason:?}"
        );
    }

    assert!(
        FilterGraph::builder()
            .raw_filter("hue", "nosuchopt=1")
            .build()
            .is_ok(),
        "raw_filter must still defer its option check to the first push; if this \
         starts failing, the two hatches no longer differ and the docs saying so are wrong"
    );
}

#[test]
fn parse_desc_should_reject_a_description_that_is_not_one_in_one_out() {
    // A description links into the chain like any single step, so it must leave
    // exactly one open input and one open output. `split` (two outputs by
    // default) is the same code path a source such as `color=c=red` (no input)
    // takes, and unlike a source it can be probed for with `raw_filter`.
    if !has_filter("split", "1") {
        println!("skipping: this FFmpeg build has no split filter");
        return;
    }
    let result = FilterGraph::parse_desc("split");
    let Err(FilterError::InvalidConfig { reason }) = result else {
        panic!("a two-output description must fail as InvalidConfig, got {result:?}");
    };
    assert!(
        reason.contains("split") && reason.contains("but has 1 and 2"),
        "the error must name the description and report the pad counts, got {reason:?}"
    );
}
