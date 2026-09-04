//! Reference-image regression suite for the 40 blend modes and the 6 Porter-Duff
//! operators (#1671).
//!
//! # What this pins that the existing suites do not
//!
//! #1669 and #1670 tie the shader to a Rust mirror, and the Rust mirror to a
//! *transcription* of `libavfilter/blend_modes.c` and the W3C compositing tables.
//! Every link below the top one is verified by a test; the top one was a reading of
//! the source. Here the reference is produced by **running** libavfilter, so a
//! mistranscription that both transcriptions share still fails.
//!
//! # What the blend-mode comparison measures
//!
//! The reference is generated the way the CPU **preview** composites: rgba layers
//! into `blend`, which negotiates an 8-bit format and runs
//! `blend_modes.c`'s `DEPTH == 8` branch. The **export** path is deliberately not
//! covered: it normalises both blend inputs to `yuv420p` first
//! (`composition_inner.rs`), and `all_mode` evaluates per plane, so the same mode
//! yields different numbers there (RK-012). ADR-0010 makes the `DEPTH == 32` float
//! branch the *formulas'* reference, and the GPU evaluates them in float, so the two
//! sides agree on the formula but not on the arithmetic. The assertion is therefore
//! not equality: it is that the gap stays rounding-sized.
//!
//! Measured on this machine, 35 of the 40 modes deviate by at most 1 level at every
//! sample and `overlay` / `hardlight` by at most 2, so 37 sit inside [`TOL_CHANNEL`]
//! outright. Exactly five carry a non-default bound, and each traces to a named line
//! of `blend_modes.c` rather than to a formula disagreement: `softlight`,
//! `multiply128` and `vividlight` to the 8-bit branch's integer arithmetic, `bleach`
//! and `stain` to the `fn` macro's unclipped store. Those last two agree **exactly**
//! (mean 0.0000) on every sample the GPU did not clamp, which is what identifies the
//! wrap as the whole of the gap. See [`BLEND_CASES`] for the per-mode numbers.
//!
//! Generating from the float branch instead (a `format=gbrpf32le` before the blend)
//! is not an improvement: `and`, `or` and `xor` are bitwise on IEEE-754 bit patterns
//! there, which is why ADR-0010 takes their 8-bit definition, and the float path's
//! singular modes (`divide`, `freeze`, `glow`, `reflect`) then pass infinities
//! through swscale's conversion. Measured here, the float reference put 23 of 40
//! modes outside tolerance against the 8-bit reference's 5.
//!
//! # Why the operators are GPU goldens
//!
//! `FFmpeg` has no Porter-Duff filter, and `blend`'s `all_expr` cannot reference the
//! other input's alpha, so avio's CPU In/Out/Atop/Xor are per-channel arithmetic on a
//! format with no alpha plane (#1753). There is no `FFmpeg` reference to generate from,
//! so those six fixtures are the GPU's own output, committed: a regression net, not
//! an independent check. The independent check for them is `blend_math.rs`'s W3C
//! `Fa`/`Fb` table.
//!
//! # Running
//!
//! Verification is a normal adapter-gated test (an `#[ignore]`d one would run neither
//! locally, since `.claude/scripts/test.sh` passes no `--include-ignored`, nor on CI).
//! Only regeneration is ignored:
//!
//! ```text
//! GPU_REFERENCE_GENERATE=1 cargo test -p avio --all-features gpu_reference -- --include-ignored
//! ```
//!
//! Regeneration needs both an `FFmpeg` built with filters and a GPU adapter, and it
//! fails loudly rather than skipping when either is missing: a silent skip would write
//! no fixtures and still report success (RK-024).

#![cfg(feature = "gpu")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use image::{RgbImage, RgbaImage};

use avio::{
    AnimatedValue, BlendMode, CompositeOp, GpuCompositor, PixelFormat, RealtimeLayer, VideoFrame,
};

use ff_filter::RealtimeComposer;

/// Fixture edge. 64 steps of `x * 4` per axis sweep the input domain densely enough
/// that a per-mode error anywhere in it shows up, and the alpha pair's two axes cover
/// all 64x64 `(sa, da)` combinations in a single image.
const SIZE: u32 = 64;

/// Per-channel tolerance, in 8-bit levels. The GPU renders in float and quantises
/// once; `FFmpeg` computes in 8-bit throughout. Two levels covers that plus driver
/// rounding, and is the same bound the CPU reference suite uses.
const TOL_CHANNEL: u8 = 2;

/// Default bound on the mean absolute per-channel deviation. A threshold-flipped
/// boundary is a thin line in a 64x64 frame, so it barely moves the mean; a
/// mistranscribed formula moves it everywhere. Measured maxima on this machine sit
/// at 0.75 (`grainextract`); the rest are below 0.41.
const TOL_MEAN: f64 = 1.0;

/// A saturation-excluded comparison must still cover most of the frame, or the
/// exclusion, not the shader, would be what makes it pass.
const MIN_COMPARED_PCT: f64 = 25.0;

/// The operator goldens are the GPU's own prior output, so only driver rounding
/// separates a passing run from the fixture.
const TOL_COMPOSITE_MEAN: f64 = 0.5;

/// One blend mode's fixture and the bounds its 8-bit-vs-float gap justifies.
///
/// Every number here is measured, and each non-default one names the line of
/// `blend_modes.c` that produces it. The filename is `FFmpeg`'s `all_mode` token, so
/// a fixture always names the mode that produced it.
struct BlendCase {
    token: &'static str,
    mode: BlendMode,
    /// Bound on the mean absolute per-channel deviation.
    mean: f64,
    /// Bound on the share of channel samples allowed past [`TOL_CHANNEL`].
    outlier_pct: f64,
    /// Drop the channel samples the GPU saturated (0 or 255) before measuring.
    ///
    /// The `fn(NAME, EXPR)` macro stores through `PIXEL` with no clip, so an
    /// expression that leaves `[0, MAX]` **wraps** in the 8-bit branch where the GPU's
    /// `Rgba8Unorm` target saturates. The wrapped samples are exactly the saturated
    /// ones, so excluding them compares the two paths where they are comparable and
    /// keeps the bound tight instead of widening it to cover a wrap.
    /// [`MIN_COMPARED_PCT`] stops the exclusion from swallowing the frame.
    skip_saturated: bool,
}

/// A mode whose float and 8-bit evaluations agree to within rounding everywhere.
const fn case(token: &'static str, mode: BlendMode) -> BlendCase {
    BlendCase {
        token,
        mode,
        mean: TOL_MEAN,
        outlier_pct: 0.0,
        skip_saturated: false,
    }
}

/// A mode whose 8-bit branch differs structurally, with the bounds that difference
/// justifies.
const fn tuned(
    token: &'static str,
    mode: BlendMode,
    mean: f64,
    outlier_pct: f64,
    skip_saturated: bool,
) -> BlendCase {
    BlendCase {
        token,
        mode,
        mean,
        outlier_pct,
        skip_saturated,
    }
}

/// All 40 `ff_filter::BlendMode` variants. `ff-render`'s `ALL_BLEND_MODES` and
/// `avio::gpu`'s `map_scene_should_map_every_blend_mode` already pin that the set is
/// complete; this table pins that each member has a fixture.
const BLEND_CASES: &[BlendCase] = &[
    case("normal", BlendMode::Normal),
    case("multiply", BlendMode::Multiply),
    case("screen", BlendMode::Screen),
    case("overlay", BlendMode::Overlay),
    // `CLIP(A * A / MAX + 2 * (B * ((A * (MAX - A)) / MAX) / MAX))`: three integer
    // divisions by MAX, each truncating, bias the 8-bit result down by up to 4 levels
    // across most of the frame. Measured mean 1.87, max 4, 17.2% past tolerance.
    tuned("softlight", BlendMode::SoftLight, 2.5, 20.0, false),
    case("hardlight", BlendMode::HardLight),
    case("dodge", BlendMode::ColorDodge),
    case("burn", BlendMode::ColorBurn),
    case("darken", BlendMode::Darken),
    case("lighten", BlendMode::Lighten),
    case("difference", BlendMode::Difference),
    case("exclusion", BlendMode::Exclusion),
    case("addition", BlendMode::Add),
    case("subtract", BlendMode::Subtract),
    case("and", BlendMode::And),
    case("average", BlendMode::Average),
    // `(MAX - B) + (MAX - A) - MAX` goes negative wherever `a + b > 1`, and the
    // unclipped 8-bit store wraps it (see `skip_saturated`).
    tuned("bleach", BlendMode::Bleach, TOL_MEAN, 0.0, true),
    case("divide", BlendMode::Divide),
    case("extremity", BlendMode::Extremity),
    case("freeze", BlendMode::Freeze),
    case("geometric", BlendMode::Geometric),
    case("glow", BlendMode::Glow),
    case("grainextract", BlendMode::GrainExtract),
    case("grainmerge", BlendMode::GrainMerge),
    case("hardmix", BlendMode::HardMix),
    case("hardoverlay", BlendMode::HardOverlay),
    case("harmonic", BlendMode::Harmonic),
    case("heat", BlendMode::Heat),
    case("interpolate", BlendMode::Interpolate),
    case("linearlight", BlendMode::LinearLight),
    // `CLIP((A - HALF) * B / MDIV + HALF)` with `MDIV = 0.125f * (1 << DEPTH)`: the
    // 8-bit branch divides by 256/8 and offsets by 128, the float branch by 255/8 and
    // 127.5. Measured mean 0.20, max 4, 1.34% past tolerance.
    tuned("multiply128", BlendMode::Multiply128, TOL_MEAN, 2.0, false),
    case("negation", BlendMode::Negation),
    case("or", BlendMode::Or),
    case("phoenix", BlendMode::Phoenix),
    case("pinlight", BlendMode::PinLight),
    case("reflect", BlendMode::Reflect),
    case("softdifference", BlendMode::SoftDifference),
    // `2 * MAX - A - B` exceeds MAX wherever `a + b < 1`; same unclipped store as
    // `bleach`, wrapping down instead of up.
    tuned("stain", BlendMode::Stain, TOL_MEAN, 0.0, true),
    // `(A < HALF) ? BURN(2 * A, B) : DODGE(2 * (A - HALF), B)`, where the 8-bit
    // BURN/DODGE scale by `1 << DEPTH` and divide as integers. The quotient is
    // steepest next to the branch, so the disagreement is a thin band along it:
    // measured mean 0.31, max 24, 0.85% past tolerance.
    tuned("vividlight", BlendMode::VividLight, TOL_MEAN, 1.5, false),
    case("xor", BlendMode::Xor),
];

/// All 6 `ff_filter::CompositeOp` variants, named by their lowercase variant name.
const COMPOSITE_CASES: &[(&str, CompositeOp)] = &[
    ("over", CompositeOp::Over),
    ("under", CompositeOp::Under),
    ("in", CompositeOp::In),
    ("out", CompositeOp::Out),
    ("atop", CompositeOp::Atop),
    ("xor", CompositeOp::Xor),
];

// Fixtures

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gpu_reference")
}

/// Bottom of the opaque pair: R ramps in x, G in y, B flat.
///
/// Paired with [`opaque_top`] the **green** channel alone sweeps all 64x64 `(A, B)`
/// pairs, so every mode is exercised over its whole 8-bit domain rather than at a
/// handful of colours; red and blue add a second and third sample of each mode with
/// one operand pinned at 64. Each channel also takes a different role between the two
/// inputs, so a mode that swapped its operands shows up.
fn opaque_bottom() -> Vec<u8> {
    let mut v = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            v.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 64, 255]);
        }
    }
    v
}

/// Top of the opaque pair: R flat, G ramps in x, B ramps in y.
fn opaque_top() -> Vec<u8> {
    let mut v = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            v.extend_from_slice(&[64, (x * 4) as u8, (y * 4) as u8, 255]);
        }
    }
    v
}

/// Bottom of the alpha pair: a flat warm colour whose alpha (`da`) ramps in x.
fn alpha_bottom() -> Vec<u8> {
    let mut v = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _y in 0..SIZE {
        for x in 0..SIZE {
            v.extend_from_slice(&[200, 120, 60, (x * 4) as u8]);
        }
    }
    v
}

/// Top of the alpha pair: a flat cool colour whose alpha (`sa`) ramps in y. With
/// [`alpha_bottom`] one 64x64 frame covers every `(sa, da)` pair on the grid, which
/// is the whole reason the operators are checked against an image rather than a table.
fn alpha_top() -> Vec<u8> {
    let mut v = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for _x in 0..SIZE {
            v.extend_from_slice(&[60, 180, 240, (y * 4) as u8]);
        }
    }
    v
}

/// A canvas-sized, identity layer carrying `blend_mode` / `composite_op`: the v1
/// supported shape the GPU renders without falling back.
fn layer(blend_mode: BlendMode, composite_op: CompositeOp) -> RealtimeLayer {
    RealtimeLayer {
        width: SIZE,
        height: SIZE,
        pixel_format: PixelFormat::Rgba,
        effects: Vec::new(),
        opacity: AnimatedValue::Static(1.0),
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        blend_mode,
        composite_op,
    }
}

// Rendering

/// Composites `bottom` under `top` on the CPU (`RealtimeComposer` = real
/// libavfilter), returning rgba, or `None` when `FFmpeg` has no filters (RK-002).
fn cpu_render(
    bottom: &[u8],
    top: &[u8],
    blend_mode: BlendMode,
    composite_op: CompositeOp,
) -> Option<Vec<u8>> {
    let bottom_frame = VideoFrame::from_rgba(SIZE, SIZE, bottom.to_vec()).unwrap();
    let top_frame = VideoFrame::from_rgba(SIZE, SIZE, top.to_vec()).unwrap();
    let layers = [
        layer(BlendMode::Normal, CompositeOp::Over),
        layer(blend_mode, composite_op),
    ];
    let mut composer = RealtimeComposer::with_canvas(&layers, Some((SIZE, SIZE))).ok()?;
    composer.push_layer(0, &bottom_frame).ok()?;
    composer.push_layer(1, &top_frame).ok()?;
    composer.pull().ok()??.to_rgba()
}

/// The same two layers on the GPU. `None` means the compositor fell back, which for
/// these inputs would itself be the regression.
fn gpu_render(
    gpu: &mut GpuCompositor,
    bottom: &[u8],
    top: &[u8],
    blend_mode: BlendMode,
    composite_op: CompositeOp,
) -> Option<Vec<u8>> {
    let bottom_frame = VideoFrame::from_rgba(SIZE, SIZE, bottom.to_vec()).unwrap();
    let top_frame = VideoFrame::from_rgba(SIZE, SIZE, top.to_vec()).unwrap();
    let bottom_layer = layer(BlendMode::Normal, CompositeOp::Over);
    let top_layer = layer(blend_mode, composite_op);
    let (rgba, _, _) = gpu.composite(
        &[(&bottom_layer, &bottom_frame), (&top_layer, &top_frame)],
        (SIZE, SIZE),
        Duration::ZERO,
    )?;
    Some(rgba)
}

// PNG I/O

fn save_rgb(path: &Path, rgba: &[u8]) {
    let mut img = RgbImage::new(SIZE, SIZE);
    for (px, out) in rgba.as_chunks::<4>().0.iter().zip(img.pixels_mut()) {
        *out = image::Rgb([px[0], px[1], px[2]]);
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save(path).unwrap();
}

fn save_rgba(path: &Path, rgba: &[u8]) {
    let img = RgbaImage::from_raw(SIZE, SIZE, rgba.to_vec()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save(path).unwrap();
}

/// Loads a fixture as a flat buffer of `channels` bytes per pixel. Panics rather
/// than skipping: the fixtures are committed, so a missing one is a broken checkout
/// or a mode without a reference, not an environment the suite should tolerate.
fn load(path: &Path, channels: usize) -> Vec<u8> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("missing or unreadable fixture {}: {e}", path.display()));
    assert_eq!(
        (img.width(), img.height()),
        (SIZE, SIZE),
        "fixture {} has the wrong size",
        path.display()
    );
    match channels {
        3 => img.to_rgb8().into_raw(),
        4 => img.to_rgba8().into_raw(),
        n => panic!("unsupported channel count {n}"),
    }
}

// Comparison

struct Deviation {
    mean: f64,
    max: u8,
    outlier_pct: f64,
    /// Share of the frame's channel samples the comparison actually looked at. Below
    /// 100% only when `skip_saturated` dropped the samples the GPU clamped.
    compared_pct: f64,
}

/// Compares the first `channels` channels of an rgba render against a flat fixture
/// of the same channel count.
fn deviation(rgba: &[u8], expected: &[u8], channels: usize, skip_saturated: bool) -> Deviation {
    assert_eq!(
        rgba.len() / 4 * channels,
        expected.len(),
        "render and fixture cover a different number of pixels"
    );
    let mut sum = 0u64;
    let mut max = 0u8;
    let mut outliers = 0u64;
    let mut n = 0u64;
    let mut total = 0u64;
    for (px, exp) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.chunks_exact(channels))
    {
        for c in 0..channels {
            total += 1;
            if skip_saturated && (px[c] == 0 || px[c] == u8::MAX) {
                continue;
            }
            let d = px[c].abs_diff(exp[c]);
            sum += u64::from(d);
            max = max.max(d);
            if d > TOL_CHANNEL {
                outliers += 1;
            }
            n += 1;
        }
    }
    let compared = n.max(1) as f64;
    Deviation {
        mean: sum as f64 / compared,
        max,
        outlier_pct: outliers as f64 * 100.0 / compared,
        compared_pct: n as f64 * 100.0 / total.max(1) as f64,
    }
}

// Generation

/// Regenerates every fixture. Ignored (it writes into the repo) and loud on a missing
/// adapter or a filterless `FFmpeg`, so it can never report success without having
/// written the references.
#[test]
#[ignore = "regenerates committed fixtures; run with GPU_REFERENCE_GENERATE=1 -- --include-ignored"]
fn gpu_reference_fixtures_should_regenerate() {
    if std::env::var("GPU_REFERENCE_GENERATE").is_err() {
        println!("set GPU_REFERENCE_GENERATE=1 to regenerate");
        return;
    }
    let dir = fixture_dir();
    let (ob, ot) = (opaque_bottom(), opaque_top());
    let (ab, at) = (alpha_bottom(), alpha_top());
    save_rgba(&dir.join("opaque_bottom.png"), &ob);
    save_rgba(&dir.join("opaque_top.png"), &ot);
    save_rgba(&dir.join("alpha_bottom.png"), &ab);
    save_rgba(&dir.join("alpha_top.png"), &at);

    for c in BLEND_CASES {
        let cpu = cpu_render(&ob, &ot, c.mode, CompositeOp::Over)
            .unwrap_or_else(|| panic!("{}: FFmpeg produced no reference frame", c.token));
        // `vf_blend.c`'s `config_params` assigns `all_mode` to all four planes with
        // no alpha special-case, so the 8-bit reference's alpha is a blend of the two
        // 255s rather than a coverage value. (`normal` is the exception: it is built
        // from `overlay`, not `blend`.) Either way the alpha is not comparable to the
        // GPU's, so only RGB is stored.
        save_rgb(&dir.join(format!("blend/{}.png", c.token)), &cpu);
    }

    let mut gpu = GpuCompositor::new().expect("regeneration needs a GPU adapter");
    for (name, op) in COMPOSITE_CASES {
        let out = gpu_render(&mut gpu, &ab, &at, BlendMode::Normal, *op)
            .unwrap_or_else(|| panic!("{name}: the GPU compositor fell back"));
        save_rgba(&dir.join(format!("composite/{name}.png")), &out);
    }
    println!(
        "regenerated {} blend + {} composite fixtures under {}",
        BLEND_CASES.len(),
        COMPOSITE_CASES.len(),
        dir.display()
    );
}

// Verification

#[test]
fn blend_mode_gpu_render_should_match_the_ffmpeg_reference_images() {
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let dir = fixture_dir().join("blend");
    let (ob, ot) = (opaque_bottom(), opaque_top());

    let mut failures = Vec::new();
    for c in BLEND_CASES {
        let expected = load(&dir.join(format!("{}.png", c.token)), 3);
        let Some(out) = gpu_render(&mut gpu, &ob, &ot, c.mode, CompositeOp::Over) else {
            panic!(
                "{}: the GPU compositor fell back on a supported layer",
                c.token
            );
        };
        let d = deviation(&out, &expected, 3, c.skip_saturated);
        println!(
            "{:<15} mean={:.4} max={:>3} past-tol={:.3}% compared={:.1}%",
            c.token, d.mean, d.max, d.outlier_pct, d.compared_pct
        );
        if d.mean > c.mean || d.outlier_pct > c.outlier_pct || d.compared_pct < MIN_COMPARED_PCT {
            failures.push(format!(
                "{}: mean={:.4} (<= {}) past-tol={:.3}% (<= {:.3}%) max={} compared={:.1}%",
                c.token, d.mean, c.mean, d.outlier_pct, c.outlier_pct, d.max, d.compared_pct
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} blend mode(s) diverged from the FFmpeg reference:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn composite_op_gpu_render_should_match_the_reference_images() {
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let dir = fixture_dir().join("composite");
    let (ab, at) = (alpha_bottom(), alpha_top());

    let mut failures = Vec::new();
    for (name, op) in COMPOSITE_CASES {
        let expected = load(&dir.join(format!("{name}.png")), 4);
        let Some(out) = gpu_render(&mut gpu, &ab, &at, BlendMode::Normal, *op) else {
            panic!("{name}: the GPU compositor fell back on a supported layer");
        };
        let d = deviation(&out, &expected, 4, false);
        println!(
            "{name:<8} mean={:.4} max={:>3} past-tol={:.3}%",
            d.mean, d.max, d.outlier_pct
        );
        if d.mean > TOL_COMPOSITE_MEAN || d.outlier_pct > 0.0 {
            failures.push(format!(
                "{name}: mean={:.4} past-tol={:.3}% max={}",
                d.mean, d.outlier_pct, d.max
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} composite operator(s) diverged from their golden:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// Non-vacuity

/// A reference suite passes trivially if the fixtures are all the same image, or all
/// equal to an input. Needs no GPU, so it guards the fixtures on every machine.
#[test]
fn blend_mode_fixtures_should_be_distinct_and_not_the_inputs() {
    let dir = fixture_dir();
    let bottom_rgb: Vec<u8> = opaque_bottom()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let top_rgb: Vec<u8> = opaque_top()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();

    let mut loaded = Vec::with_capacity(BLEND_CASES.len());
    for c in BLEND_CASES {
        let img = load(&dir.join(format!("blend/{}.png", c.token)), 3);
        // `normal` is the top layer by definition, so it is the one fixture that may
        // equal an input; every other mode must transform it.
        if c.mode != BlendMode::Normal {
            assert_ne!(img, top_rgb, "{}: fixture is just the top input", c.token);
        }
        assert_ne!(
            img, bottom_rgb,
            "{}: fixture is just the bottom input",
            c.token
        );
        loaded.push((c.token, img));
    }
    for (i, (a_name, a)) in loaded.iter().enumerate() {
        for (b_name, b) in &loaded[i + 1..] {
            assert_ne!(a, b, "fixtures for {a_name} and {b_name} are identical");
        }
    }
}

/// One of the four input generators, named so the fixture guard can table them.
type InputGenerator = fn() -> Vec<u8>;

/// The committed inputs must still be what the generators produce.
///
/// Nothing else reads these four PNGs: verification rebuilds the inputs in Rust. So if
/// a generator is edited without regenerating, all 46 references were produced from a
/// different image than the one being rendered, and the failures would point at the
/// modes instead of at the cause. Needs no GPU.
#[test]
fn input_fixtures_should_match_the_generators() {
    let dir = fixture_dir();
    let cases: [(&str, InputGenerator); 4] = [
        ("opaque_bottom", opaque_bottom),
        ("opaque_top", opaque_top),
        ("alpha_bottom", alpha_bottom),
        ("alpha_top", alpha_top),
    ];
    for (name, generate) in cases {
        assert_eq!(
            load(&dir.join(format!("{name}.png")), 4),
            generate(),
            "{name}.png no longer matches its generator; regenerate the fixtures"
        );
    }
}

/// The operator goldens' **alpha** must satisfy the W3C `ao = as*Fa + ab*Fb` closed
/// forms across the whole grid, independently of the GPU that produced them.
///
/// This is what keeps the six self-generated fixtures from being vacuous. The alpha
/// output needs no premultiplication reasoning and no colour algebra: with `sa` and
/// `da` read straight off the input ramps, each operator's `ao` is one expression,
/// evaluated here for all 4096 combinations. A shader that mixed up `In` and `Out`, or
/// dropped `da` from `Atop`, fails here even though it generated the fixtures.
///
/// It does **not** separate `Over` from `Under`, whose `ao` coincide; that pair is
/// covered by [`composite_op_fixtures_should_be_distinct`], which compares colour.
#[test]
fn composite_fixtures_should_satisfy_the_w3c_alpha_closed_forms() {
    let dir = fixture_dir().join("composite");
    for (name, op) in COMPOSITE_CASES {
        let img = load(&dir.join(format!("{name}.png")), 4);
        let mut worst = 0u8;
        for y in 0..SIZE {
            for x in 0..SIZE {
                // The bottom layer's alpha ramps in x and reaches the canvas as the
                // backdrop coverage; the top layer's ramps in y and is the source.
                let da = f32::from((x * 4) as u8) / 255.0;
                let sa = f32::from((y * 4) as u8) / 255.0;
                let (fa, fb) = match op {
                    CompositeOp::Over => (1.0, 1.0 - sa),
                    CompositeOp::Under => (1.0 - da, 1.0),
                    CompositeOp::In => (da, 0.0),
                    CompositeOp::Out => (1.0 - da, 0.0),
                    CompositeOp::Atop => (da, 1.0 - sa),
                    CompositeOp::Xor => (1.0 - da, 1.0 - sa),
                    // `CompositeOp` is `#[non_exhaustive]` from ff-filter (RK-003).
                    _ => panic!("{name}: operator has no closed form here"),
                };
                let expected = ((sa * fa + da * fb) * 255.0 + 0.5) as u8;
                let actual = img[((y * SIZE + x) * 4 + 3) as usize];
                worst = worst.max(actual.abs_diff(expected));
            }
        }
        assert!(
            worst <= TOL_CHANNEL,
            "{name}: golden alpha departs from the W3C closed form by {worst} levels"
        );
    }
}

/// The six operators must produce six different images: a shader that ignored
/// `u.composite` and always composited `Over` would otherwise pass against goldens it
/// had generated itself.
#[test]
fn composite_op_fixtures_should_be_distinct() {
    let dir = fixture_dir().join("composite");
    let loaded: Vec<(&str, Vec<u8>)> = COMPOSITE_CASES
        .iter()
        .map(|(name, _)| (*name, load(&dir.join(format!("{name}.png")), 4)))
        .collect();
    for (i, (a_name, a)) in loaded.iter().enumerate() {
        for (b_name, b) in &loaded[i + 1..] {
            assert_ne!(a, b, "fixtures for {a_name} and {b_name} are identical");
        }
    }
}

#[test]
fn every_blend_mode_and_operator_should_have_a_fixture_row() {
    assert_eq!(
        BLEND_CASES.len(),
        40,
        "ff_filter::BlendMode has 40 variants"
    );
    assert_eq!(
        COMPOSITE_CASES.len(),
        6,
        "ff_filter::CompositeOp has 6 variants"
    );
    let mut tokens: Vec<&str> = BLEND_CASES.iter().map(|c| c.token).collect();
    tokens.sort_unstable();
    let before = tokens.len();
    tokens.dedup();
    assert_eq!(before, tokens.len(), "two modes share a fixture filename");
}
