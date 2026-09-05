//! `ff_format::Color::parse_ffmpeg`'s name table, checked against the linked
//! `FFmpeg` (#1630).
//!
//! The table mirrors `libavutil/parseutils.c`'s `color_table`, and a hand-written
//! copy of 140 rows is exactly where a typo survives forever. `ff-format` has no
//! `FFmpeg` dependency and so cannot check itself; this test lives here, in the
//! lowest crate that has one, and asks `FFmpeg` for every row (RK-005: verify
//! tokens against the real thing, not against documentation).
//!
//! Each name is rendered with the `color` filter and the pixel read back. Both
//! branches are `format=rgba` and the overlay is given `:format=auto` — the
//! spelling the compositor uses everywhere else for the same reason. Without it
//! `overlay` takes its `yuv420` default, the chain round-trips through YUV, and
//! `0x123456` reads back as 17,50,85 instead of 18,52,86; every assertion here
//! would then be approximate for no reason.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ff_filter::FilterGraph;
use ff_format::{Color, PixelFormat, PooledBuffer, Timestamp, VideoFrame};

/// The value the gate renders. Any exact RGB triple would do; this one is not in
/// the name table, so the gate cannot pass by accident on a table entry.
const GATE_HEX: &str = "0x123456";
const GATE_RGB: [u8; 3] = [0x12, 0x34, 0x56];

fn base_frame() -> VideoFrame {
    let (w, h) = (8u32, 8u32);
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(vec![128u8; (w * h) as usize]),
            PooledBuffer::standalone(vec![128u8; ((w / 2) * (h / 2)) as usize]),
            PooledBuffer::standalone(vec![128u8; ((w / 2) * (h / 2)) as usize]),
        ],
        vec![w as usize, (w / 2) as usize, (w / 2) as usize],
        w,
        h,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

/// Renders `color=c=<spec>` over an 8x8 base and returns the top-left RGBA pixel.
///
/// `None` means this `FFmpeg` build could not run the graph at all — which is
/// what the gate below distinguishes from a wrong value.
fn render_color(spec: &str) -> Option<[u8; 4]> {
    let desc = format!(
        "format=rgba[a];color=c={spec}:s=8x8,format=rgba[b];[a][b]overlay=0:0:format=auto,format=rgba"
    );
    let mut graph = FilterGraph::parse_desc(&desc).ok()?;
    graph.push_video(0, &base_frame()).ok()?;
    let frame = graph.pull_video().ok()??;
    let plane = frame.plane(0)?;
    Some([plane[0], plane[1], plane[2], plane[3]])
}

/// Can this build render an exact colour at all?
///
/// Probes with a **hex** spec, which `parse_ffmpeg` handles without consulting the
/// name table — so the gate exercises a different thing from what is asserted and
/// cannot mask a wrong row (RK-002). Requiring the exact value also rules out a
/// build whose format conversions are lossy, where every assertion below would be
/// noise.
fn exact_color_rendering_available() -> bool {
    match render_color(GATE_HEX) {
        Some(px) => {
            let exact = [px[0], px[1], px[2]] == GATE_RGB;
            if !exact {
                println!("skipping: {GATE_HEX} rendered as {px:?}, not exact");
            }
            exact
        }
        None => {
            println!("skipping: this FFmpeg build cannot run the color/overlay graph");
            false
        }
    }
}

#[test]
fn every_color_name_should_match_what_ffmpeg_renders() {
    if !exact_color_rendering_available() {
        return;
    }
    let mut checked = 0usize;
    let mut mismatches = Vec::new();
    for (name, expected) in Color::ffmpeg_color_names() {
        let Some(px) = render_color(name) else {
            mismatches.push(format!("{name}: FFmpeg rejected the name"));
            continue;
        };
        if [px[0], px[1], px[2]] != [expected.r, expected.g, expected.b] {
            mismatches.push(format!(
                "{name}: table says {:?}, FFmpeg renders {:?}",
                [expected.r, expected.g, expected.b],
                [px[0], px[1], px[2]]
            ));
        }
        checked += 1;
    }
    assert!(
        mismatches.is_empty(),
        "the colour table disagrees with this FFmpeg:\n{}",
        mismatches.join("\n")
    );
    assert_eq!(
        checked,
        Color::ffmpeg_color_names().len(),
        "every row must have been rendered"
    );
}

#[test]
fn the_table_should_agree_with_ffmpeg_on_the_values_memory_gets_wrong() {
    // A focused guard for the two rows that would be wrong had the table been
    // written from the CSS list rather than read out of FFmpeg. It fails loudly
    // and by name, where the sweep above would bury them in a list.
    if !exact_color_rendering_available() {
        return;
    }
    let green = render_color("green").expect("green must render");
    assert_eq!(
        [green[0], green[1], green[2]],
        [0x00, 0x80, 0x00],
        "FFmpeg's `green` is the HTML value, not X11's 0x00FF00"
    );
    assert_eq!(
        Color::parse_ffmpeg("green").map(|c| [c.r, c.g, c.b]),
        Some([0x00, 0x80, 0x00])
    );
    assert!(
        render_color("lightgray").is_none(),
        "FFmpeg has `lightgrey` but no `lightgray`; if that changed, add the row"
    );
    assert!(render_color("lightgrey").is_some());
}

#[test]
fn the_parser_should_not_accept_a_form_ffmpeg_rejects() {
    // The parser's contract is "accept what FFmpeg accepts", and being *more*
    // permissive is the harmful direction: the GPU would map a colour the CPU
    // filter path cannot build, and ADR-0007 makes the CPU the correctness
    // reference. `0X` is the one form the two disagreed on when the boundary was
    // measured, so it is checked here against FFmpeg rather than against a belief
    // about FFmpeg.
    if !exact_color_rendering_available() {
        return;
    }
    for spec in ["0x123456", "#123456", "GREEN", "green@0.5", "0x12345678"] {
        assert!(
            render_color(spec).is_some() && Color::parse_ffmpeg(spec).is_some(),
            "{spec} must be accepted by both"
        );
    }
    assert!(
        render_color("0X123456").is_none(),
        "FFmpeg compares the `0x` prefix case-sensitively; if that changed, the          parser may accept `0X` again"
    );
    assert_eq!(Color::parse_ffmpeg("0X123456"), None);
}
