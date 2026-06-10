//! Internal filter step representation.

use std::iter;
use std::time::Duration;

use super::builder::FilterGraphBuilder;
use super::types::{
    DrawTextOptions, EqBand, Rgb, ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode,
};
use crate::animation::AnimatedValue;
use crate::blend::BlendMode;
use ff_format::{AlphaMode, ColorRange, ColorSpace, PixelFormat};

/// Escapes a filesystem path for use as a value inside an `FFmpeg` filter
/// argument string (e.g. `lut3d`'s `file=` option).
///
/// `FFmpeg`'s filter-argument parser treats `:` as an option separator and `\`
/// as an escape character, so Windows paths like `D:\dir\file.cube` break the
/// parser. Normalising backslashes to forward slashes (accepted on Windows) and
/// escaping the drive colon as `\:` yields a value the parser accepts.
pub(crate) fn escape_filter_path(path: &str) -> String {
    path.replace('\\', "/").replace(':', "\\:")
}

// ── FilterStep ────────────────────────────────────────────────────────────────

/// A single step in a filter chain.
///
/// Used by [`crate::FilterGraphBuilder`] to build pipeline filter graphs, and by
/// [`crate::AudioTrack::effects`] to attach per-track effects in a multi-track mix.
#[derive(Debug, Clone)]
pub enum FilterStep {
    /// Convert video to a suitable pixel format from the given options.
    Format {
        pix_fmts: Vec<PixelFormat>,
        color_spaces: Vec<ColorSpace>,
        color_ranges: Vec<ColorRange>,
        alpha_modes: Vec<AlphaMode>,
    },

    /// Trim: keep only frames in `[start, end)` seconds.
    Trim { start: f64, end: f64 },
    /// Scale to a new resolution using the given resampling algorithm.
    Scale {
        width: u32,
        height: u32,
        algorithm: ScaleAlgorithm,
    },
    /// Crop a rectangular region.
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    /// Overlay a second stream at position `(x, y)`.
    Overlay { x: i32, y: i32 },
    /// Fade-in from black starting at `start` seconds, over `duration` seconds.
    FadeIn { start: f64, duration: f64 },
    /// Fade-out to black starting at `start` seconds, over `duration` seconds.
    FadeOut { start: f64, duration: f64 },
    /// Audio fade-in from silence starting at `start` seconds, over `duration` seconds.
    AFadeIn { start: f64, duration: f64 },
    /// Audio fade-out to silence starting at `start` seconds, over `duration` seconds.
    AFadeOut { start: f64, duration: f64 },
    /// Fade-in from white starting at `start` seconds, over `duration` seconds.
    FadeInWhite { start: f64, duration: f64 },
    /// Fade-out to white starting at `start` seconds, over `duration` seconds.
    FadeOutWhite { start: f64, duration: f64 },
    /// Rotate clockwise by `angle_degrees`, filling exposed areas with `fill_color`.
    Rotate {
        angle_degrees: f64,
        fill_color: String,
    },
    /// HDR-to-SDR tone mapping.
    ToneMap(ToneMap),
    /// Adjust audio volume (in dB; negative = quieter).
    Volume(f64),
    /// Mix `n` audio inputs together.
    Amix(usize),
    /// Multi-band parametric equalizer (low-shelf, high-shelf, or peak bands).
    ///
    /// Each band maps to its own `FFmpeg` filter node chained in sequence.
    /// The `bands` vec must not be empty.
    ParametricEq { bands: Vec<EqBand> },
    /// Apply a 3D LUT from a `.cube` or `.3dl` file.
    Lut3d { path: String },
    /// Brightness/contrast/saturation adjustment via `FFmpeg` `eq` filter.
    Eq {
        brightness: f32,
        contrast: f32,
        saturation: f32,
    },
    /// Brightness / contrast / saturation / gamma via `FFmpeg` `eq` filter (optionally animated).
    ///
    /// Arguments are evaluated at [`Duration::ZERO`] for the initial graph build.
    /// Per-frame updates are applied via `avfilter_graph_send_command` in #363.
    EqAnimated {
        /// Brightness offset. Range: −1.0 – 1.0 (neutral: 0.0).
        brightness: AnimatedValue<f64>,
        /// Contrast multiplier. Range: 0.0 – 3.0 (neutral: 1.0).
        contrast: AnimatedValue<f64>,
        /// Saturation multiplier. Range: 0.0 – 3.0 (neutral: 1.0; 0.0 = grayscale).
        saturation: AnimatedValue<f64>,
        /// Global gamma correction. Range: 0.1 – 10.0 (neutral: 1.0).
        gamma: AnimatedValue<f64>,
    },
    /// Three-way color balance (shadows / midtones / highlights) via `FFmpeg` `colorbalance` filter
    /// (optionally animated).
    ///
    /// Each tuple is `(R, G, B)`. Valid range per component: −1.0 – 1.0 (neutral: 0.0).
    ///
    /// Arguments are evaluated at [`Duration::ZERO`] for the initial graph build.
    /// Per-frame updates are applied via `avfilter_graph_send_command` in #363.
    ColorBalanceAnimated {
        /// Shadows (lift) correction per channel. `FFmpeg` params: `"rs"`, `"gs"`, `"bs"`.
        lift: AnimatedValue<(f64, f64, f64)>,
        /// Midtones (gamma) correction per channel. `FFmpeg` params: `"rm"`, `"gm"`, `"bm"`.
        gamma: AnimatedValue<(f64, f64, f64)>,
        /// Highlights (gain) correction per channel. `FFmpeg` params: `"rh"`, `"gh"`, `"bh"`.
        gain: AnimatedValue<(f64, f64, f64)>,
    },
    /// Per-channel RGB color curves adjustment.
    Curves {
        master: Vec<(f32, f32)>,
        r: Vec<(f32, f32)>,
        g: Vec<(f32, f32)>,
        b: Vec<(f32, f32)>,
    },
    /// White balance correction via `colorchannelmixer`.
    WhiteBalance { temperature_k: u32, tint: f32 },
    /// Hue rotation by an arbitrary angle.
    Hue { degrees: f32 },
    /// Per-channel gamma correction via `FFmpeg` `eq` filter.
    Gamma { r: f32, g: f32, b: f32 },
    /// Three-way colour corrector (lift / gamma / gain) via `FFmpeg` `curves` filter.
    ThreeWayCC {
        /// Affects shadows (blacks). Neutral: `Rgb::NEUTRAL`.
        lift: Rgb,
        /// Affects midtones. Neutral: `Rgb::NEUTRAL`. All components must be > 0.0.
        gamma: Rgb,
        /// Affects highlights (whites). Neutral: `Rgb::NEUTRAL`.
        gain: Rgb,
    },
    /// Vignette effect via `FFmpeg` `vignette` filter.
    Vignette {
        /// Radius angle in radians (valid range: 0.0 – π/2 ≈ 1.5708). Default: π/5 ≈ 0.628.
        angle: f32,
        /// Horizontal centre of the vignette. `0.0` maps to `w/2`.
        x0: f32,
        /// Vertical centre of the vignette. `0.0` maps to `h/2`.
        y0: f32,
    },
    /// Horizontal flip (mirror left-right).
    HFlip,
    /// Vertical flip (mirror top-bottom).
    VFlip,
    /// Reverse video playback (buffers entire clip in memory — use only on short clips).
    Reverse,
    /// Reverse audio playback (buffers entire clip in memory — use only on short clips).
    AReverse,
    /// Pad to a target resolution with a fill color (letterbox / pillarbox).
    Pad {
        /// Target canvas width in pixels.
        width: u32,
        /// Target canvas height in pixels.
        height: u32,
        /// Horizontal offset of the source frame within the canvas.
        /// Negative values are replaced with `(ow-iw)/2` (centred).
        x: i32,
        /// Vertical offset of the source frame within the canvas.
        /// Negative values are replaced with `(oh-ih)/2` (centred).
        y: i32,
        /// Fill color (any `FFmpeg` color string, e.g. `"black"`, `"0x000000"`).
        color: String,
    },
    /// Scale (preserving aspect ratio) then centre-pad to fill target dimensions
    /// (letterbox or pillarbox as required).
    ///
    /// Implemented as a `scale` filter with `force_original_aspect_ratio=decrease`
    /// followed by a `pad` filter that centres the scaled frame on the canvas.
    FitToAspect {
        /// Target canvas width in pixels.
        width: u32,
        /// Target canvas height in pixels.
        height: u32,
        /// Fill color for the bars (any `FFmpeg` color string, e.g. `"black"`).
        color: String,
    },
    /// Gaussian blur with configurable radius.
    ///
    /// `sigma` is the blur radius. Valid range: 0.0 – 10.0 (values near 0.0 are
    /// nearly a no-op; higher values produce a stronger blur).
    GBlur {
        /// Blur radius (standard deviation). Must be ≥ 0.0.
        sigma: f32,
    },
    /// Crop with optionally animated boundaries (pixels, `f64` for sub-pixel precision).
    ///
    /// Arguments are evaluated at [`Duration::ZERO`] for the initial graph build.
    /// Per-frame updates are applied via `avfilter_graph_send_command` in #363.
    CropAnimated {
        /// X offset of the top-left corner, in pixels.
        x: AnimatedValue<f64>,
        /// Y offset of the top-left corner, in pixels.
        y: AnimatedValue<f64>,
        /// Width of the cropped region. Must evaluate to > 0 at `Duration::ZERO`.
        width: AnimatedValue<f64>,
        /// Height of the cropped region. Must evaluate to > 0 at `Duration::ZERO`.
        height: AnimatedValue<f64>,
    },
    /// Gaussian blur with an optionally animated sigma (blur radius).
    ///
    /// Arguments are evaluated at [`Duration::ZERO`] for the initial graph build.
    /// Per-frame updates are applied via `avfilter_graph_send_command` in #363.
    GBlurAnimated {
        /// Blur radius (standard deviation). Must evaluate to ≥ 0.0 at `Duration::ZERO`.
        sigma: AnimatedValue<f64>,
    },
    /// Sharpen or blur via unsharp mask (luma + chroma strength).
    ///
    /// Positive values sharpen; negative values blur. Valid range for each
    /// component: −1.5 – 1.5.
    Unsharp {
        /// Luma (brightness) sharpening/blurring amount. Range: −1.5 – 1.5.
        luma_strength: f32,
        /// Chroma (colour) sharpening/blurring amount. Range: −1.5 – 1.5.
        chroma_strength: f32,
    },
    /// High Quality 3D noise reduction (`hqdn3d`).
    ///
    /// Typical values: `luma_spatial=4.0`, `chroma_spatial=3.0`,
    /// `luma_tmp=6.0`, `chroma_tmp=4.5`. All values must be ≥ 0.0.
    Hqdn3d {
        /// Spatial luma noise reduction strength. Must be ≥ 0.0.
        luma_spatial: f32,
        /// Spatial chroma noise reduction strength. Must be ≥ 0.0.
        chroma_spatial: f32,
        /// Temporal luma noise reduction strength. Must be ≥ 0.0.
        luma_tmp: f32,
        /// Temporal chroma noise reduction strength. Must be ≥ 0.0.
        chroma_tmp: f32,
    },
    /// Non-local means noise reduction (`nlmeans`).
    ///
    /// `strength` controls the denoising intensity; range 1.0–30.0.
    /// Higher values remove more noise but are significantly more CPU-intensive.
    ///
    /// NOTE: nlmeans is CPU-intensive; avoid for real-time pipelines.
    Nlmeans {
        /// Denoising strength. Must be in the range [1.0, 30.0].
        strength: f32,
    },
    /// Deinterlace using the `yadif` filter.
    Yadif {
        /// Deinterlacing mode controlling output frame rate and spatial checks.
        mode: YadifMode,
    },
    /// Cross-dissolve transition between two video streams (`xfade`).
    ///
    /// Requires two input slots: slot 0 is clip A, slot 1 is clip B.
    /// `duration` is the overlap length in seconds; `offset` is the PTS
    /// offset (in seconds) at which clip B begins.
    XFade {
        /// Transition style.
        transition: XfadeTransition,
        /// Overlap duration in seconds. Must be > 0.0.
        duration: f64,
        /// PTS offset (seconds) where clip B starts.
        offset: f64,
    },
    /// Draw text onto the video using the `drawtext` filter.
    DrawText {
        /// Full set of drawtext parameters.
        opts: DrawTextOptions,
    },
    /// Burn-in SRT subtitles (hard subtitles) using the `subtitles` filter.
    SubtitlesSrt {
        /// Absolute or relative path to the `.srt` file.
        path: String,
    },
    /// Burn-in ASS/SSA styled subtitles using the `ass` filter.
    SubtitlesAss {
        /// Absolute or relative path to the `.ass` or `.ssa` file.
        path: String,
    },
    /// Playback speed change using `setpts` (video) and chained `atempo` (audio).
    ///
    /// `factor > 1.0` = fast motion; `factor < 1.0` = slow motion.
    /// Valid range: 0.1–100.0.
    ///
    /// Video path: `setpts=PTS/{factor}`.
    /// Audio path: the `atempo` filter only accepts [0.5, 2.0] per instance;
    /// `filter_inner` chains multiple instances to cover the full range.
    Speed {
        /// Speed multiplier. Must be in [0.1, 100.0].
        factor: f64,
    },
    /// EBU R128 two-pass loudness normalization.
    ///
    /// Pass 1 measures integrated loudness with `ebur128=peak=true:metadata=1`.
    /// Pass 2 applies a linear volume correction so the output reaches `target_lufs`.
    /// All audio frames are buffered in memory between the two passes — use only
    /// for clips that fit comfortably in RAM.
    LoudnessNormalize {
        /// Target integrated loudness in LUFS (e.g. −23.0). Must be < 0.0.
        target_lufs: f32,
        /// True-peak ceiling in dBTP (e.g. −1.0). Must be ≤ 0.0.
        true_peak_db: f32,
        /// Target loudness range in LU (e.g. 7.0). Must be > 0.0.
        lra: f32,
    },
    /// Peak-level two-pass normalization using `astats`.
    ///
    /// Pass 1 measures the true peak with `astats=metadata=1`.
    /// Pass 2 applies `volume={gain}dB` so the output peak reaches `target_db`.
    /// All audio frames are buffered in memory between passes — use only
    /// for clips that fit comfortably in RAM.
    NormalizePeak {
        /// Target peak level in dBFS (e.g. −1.0). Must be ≤ 0.0.
        target_db: f32,
    },
    /// Noise gate via `FFmpeg`'s `agate` filter.
    ///
    /// Audio below `threshold_db` is attenuated; audio above passes through.
    /// The threshold is converted from dBFS to the linear scale expected by
    /// `agate`'s `threshold` parameter (`linear = 10^(dB/20)`).
    ANoiseGate {
        /// Gate open/close threshold in dBFS (e.g. −40.0).
        threshold_db: f32,
        /// Attack time in milliseconds — how quickly the gate opens. Must be > 0.0.
        attack_ms: f32,
        /// Release time in milliseconds — how quickly the gate closes. Must be > 0.0.
        release_ms: f32,
    },
    /// Dynamic range compressor via `FFmpeg`'s `acompressor` filter.
    ///
    /// Reduces the dynamic range of the audio signal: peaks above
    /// `threshold_db` are attenuated by `ratio`:1.  `makeup_db` applies
    /// additional gain after compression to restore perceived loudness.
    ACompressor {
        /// Compression threshold in dBFS (e.g. −20.0).
        threshold_db: f32,
        /// Compression ratio (e.g. 4.0 = 4:1). Must be ≥ 1.0.
        ratio: f32,
        /// Attack time in milliseconds. Must be > 0.0.
        attack_ms: f32,
        /// Release time in milliseconds. Must be > 0.0.
        release_ms: f32,
        /// Make-up gain in dB applied after compression (e.g. 6.0).
        makeup_db: f32,
    },
    /// Downmix stereo to mono via `FFmpeg`'s `pan` filter.
    ///
    /// Both channels are mixed with equal weight:
    /// `mono|c0=0.5*c0+0.5*c1`.  The output has a single channel.
    StereoToMono,
    /// Remap audio channels using `FFmpeg`'s `channelmap` filter.
    ///
    /// `mapping` is a `|`-separated list of output channel names taken
    /// from input channels, e.g. `"FR|FL"` swaps left and right.
    /// Must not be empty.
    ChannelMap {
        /// `FFmpeg` channelmap mapping expression (e.g. `"FR|FL"`).
        mapping: String,
    },
    /// A/V sync correction via audio delay or advance.
    ///
    /// Positive `ms`: uses `FFmpeg`'s `adelay` filter to shift audio later.
    /// Negative `ms`: uses `FFmpeg`'s `atrim` filter to trim the audio start,
    /// effectively advancing audio by `|ms|` milliseconds.
    /// Zero `ms`: uses `adelay` with zero delay (no-op).
    AudioDelay {
        /// Delay in milliseconds. Positive = delay; negative = advance.
        ms: f64,
    },
    /// Concatenate `n` sequential video input segments via `FFmpeg`'s `concat` filter.
    ///
    /// Requires `n` video input slots (0 through `n-1`). `n` must be ≥ 2.
    ConcatVideo {
        /// Number of video input segments to concatenate. Must be ≥ 2.
        n: u32,
    },
    /// Concatenate `n` sequential audio input segments via `FFmpeg`'s `concat` filter.
    ///
    /// Requires `n` audio input slots (0 through `n-1`). `n` must be ≥ 2.
    ConcatAudio {
        /// Number of audio input segments to concatenate. Must be ≥ 2.
        n: u32,
    },
    /// Freeze a single frame for a configurable duration using `FFmpeg`'s `loop` filter.
    ///
    /// The frame nearest to `pts` seconds is held for `duration` seconds, then
    /// playback resumes. Frame numbers are approximated using a 25 fps assumption;
    /// accuracy depends on the source stream's actual frame rate.
    FreezeFrame {
        /// Timestamp of the frame to freeze, in seconds. Must be >= 0.0.
        pts: f64,
        /// Duration to hold the frozen frame, in seconds. Must be > 0.0.
        duration: f64,
    },
    /// Scrolling text ticker (right-to-left) using the `drawtext` filter.
    ///
    /// The text starts off-screen to the right and scrolls left at
    /// `speed_px_per_sec` pixels per second using the expression
    /// `x = w - t * speed`.
    Ticker {
        /// Text to display. Special characters (`\`, `:`, `'`) are escaped.
        text: String,
        /// Y position as an `FFmpeg` expression, e.g. `"h-50"` or `"10"`.
        y: String,
        /// Horizontal scroll speed in pixels per second (must be > 0.0).
        speed_px_per_sec: f32,
        /// Font size in points.
        font_size: u32,
        /// Font color as an `FFmpeg` color string, e.g. `"white"` or `"0xFFFFFF"`.
        font_color: String,
    },
    /// Join two video clips with a cross-dissolve transition.
    ///
    /// Compound step — expands in `filter_inner` to:
    /// ```text
    /// in0 → trim(end=clip_a_end+dissolve_dur) → setpts → xfade[0]
    /// in1 → trim(start=max(0, clip_b_start−dissolve_dur)) → setpts → xfade[1]
    /// ```
    ///
    /// Requires two video input slots: slot 0 = clip A, slot 1 = clip B.
    /// `clip_a_end` and `dissolve_dur` must be > 0.0.
    JoinWithDissolve {
        /// Timestamp (seconds) where clip A ends. Must be > 0.0.
        clip_a_end: f64,
        /// Timestamp (seconds) where clip B content starts (before the overlap).
        clip_b_start: f64,
        /// Cross-dissolve overlap duration in seconds. Must be > 0.0.
        dissolve_dur: f64,
    },
    /// Composite a PNG image (watermark / logo) over video with optional opacity.
    ///
    /// This is a compound step: internally it creates a `movie` source,
    /// a `lut` alpha-scaling filter, and an `overlay` compositing filter.
    /// The image file is loaded once at graph construction time.
    OverlayImage {
        /// Absolute or relative path to the `.png` file.
        path: String,
        /// Horizontal position as an `FFmpeg` expression, e.g. `"10"` or `"W-w-10"`.
        x: String,
        /// Vertical position as an `FFmpeg` expression, e.g. `"10"` or `"H-h-10"`.
        y: String,
        /// Opacity 0.0 (fully transparent) to 1.0 (fully opaque).
        opacity: f32,
    },

    /// Blend a `top` layer over the current stream (bottom) using the given mode.
    ///
    /// This is a compound step:
    /// - **Normal** mode: `[top]colorchannelmixer=aa=<opacity>[top_faded];
    ///   [bottom][top_faded]overlay=format=auto:shortest=1[out]`
    ///   (the `colorchannelmixer` step is omitted when `opacity == 1.0`).
    /// - All other modes return [`crate::FilterError::InvalidConfig`] from
    ///   [`crate::FilterGraphBuilder::build`] until implemented.
    ///
    /// The `top` builder's steps are applied to the second input slot (`in1`).
    /// `opacity` is clamped to `[0.0, 1.0]` by the builder method.
    ///
    /// `Box<FilterGraphBuilder>` is used to break the otherwise-recursive type:
    /// `FilterStep` → `FilterGraphBuilder` → `Vec<FilterStep>`.
    Blend {
        /// Filter pipeline for the top (foreground) layer.
        top: Box<FilterGraphBuilder>,
        /// How the two layers are combined.
        mode: BlendMode,
        /// Opacity of the top layer in `[0.0, 1.0]`; 1.0 = fully opaque.
        opacity: f32,
    },

    /// Remove pixels matching `color` using `FFmpeg`'s `chromakey` filter,
    /// producing a `yuva420p` output with transparent areas where the key
    /// color was detected.
    ///
    /// Use this for YCbCr-encoded sources (most video).  For RGB sources
    /// use `colorkey` instead.
    ChromaKey {
        /// `FFmpeg` color string, e.g. `"green"`, `"0x00FF00"`, `"#00FF00"`.
        color: String,
        /// Match radius in `[0.0, 1.0]`; higher = more pixels removed.
        similarity: f32,
        /// Edge softness in `[0.0, 1.0]`; `0.0` = hard edge.
        blend: f32,
    },

    /// Remove pixels matching `color` in RGB space using `FFmpeg`'s `colorkey`
    /// filter, producing an `rgba` output with transparent areas where the key
    /// color was detected.
    ///
    /// Use this for RGB-encoded sources.  For YCbCr-encoded video (most video)
    /// use `chromakey` instead.
    ColorKey {
        /// `FFmpeg` color string, e.g. `"green"`, `"0x00FF00"`, `"#00FF00"`.
        color: String,
        /// Match radius in `[0.0, 1.0]`; higher = more pixels removed.
        similarity: f32,
        /// Edge softness in `[0.0, 1.0]`; `0.0` = hard edge.
        blend: f32,
    },

    /// Reduce color spill from the key color on subject edges using `FFmpeg`'s
    /// `hue` filter to desaturate the spill hue region.
    ///
    /// Applies `hue=h=0:s=(1.0 - strength)`.  `strength=0.0` leaves the image
    /// unchanged; `strength=1.0` fully desaturates.
    ///
    /// `key_color` is stored for future use by a more targeted per-hue
    /// implementation.
    SpillSuppress {
        /// `FFmpeg` color string identifying the spill color, e.g. `"green"`.
        key_color: String,
        /// Suppression intensity in `[0.0, 1.0]`; `0.0` = no effect, `1.0` = full suppression.
        strength: f32,
    },

    /// Merge a grayscale `matte` as the alpha channel of the input video using
    /// `FFmpeg`'s `alphamerge` filter.
    ///
    /// White (luma=255) in the matte produces fully opaque output; black (luma=0)
    /// produces fully transparent output.
    ///
    /// This is a compound step: the `matte` builder's pipeline is applied to the
    /// second input slot (`in1`) before the `alphamerge` filter is linked.
    ///
    /// `Box<FilterGraphBuilder>` breaks the otherwise-recursive type, following
    /// the same pattern as [`FilterStep::Blend`].
    AlphaMatte {
        /// Pipeline for the grayscale matte stream (slot 1).
        matte: Box<FilterGraphBuilder>,
    },

    /// Key out pixels by luminance value using `FFmpeg`'s `lumakey` filter.
    ///
    /// Pixels whose normalized luma is within `tolerance` of `threshold` are
    /// made transparent.  When `invert` is `true`, a `geq` filter is appended
    /// to negate the alpha channel, effectively swapping transparent and opaque
    /// regions.
    ///
    /// - `threshold`: luma cutoff in `[0.0, 1.0]`; `0.0` = black, `1.0` = white.
    /// - `tolerance`: match radius around the threshold in `[0.0, 1.0]`.
    /// - `softness`: edge feather width in `[0.0, 1.0]`; `0.0` = hard edge.
    /// - `invert`: when `false`, keys out bright regions (pixels matching the
    ///   threshold); when `true`, the alpha is negated after keying, making
    ///   the complementary region transparent instead.
    ///
    /// Output carries an alpha channel (`yuva420p`).
    LumaKey {
        /// Luma cutoff in `[0.0, 1.0]`.
        threshold: f32,
        /// Match radius around the threshold in `[0.0, 1.0]`.
        tolerance: f32,
        /// Edge feather width in `[0.0, 1.0]`; `0.0` = hard edge.
        softness: f32,
        /// When `true`, the alpha channel is negated after keying.
        invert: bool,
    },

    /// Apply a rectangular alpha mask using `FFmpeg`'s `geq` filter.
    ///
    /// Pixels inside the rectangle defined by (`x`, `y`, `width`, `height`)
    /// are made fully opaque (`alpha=255`); pixels outside are made fully
    /// transparent (`alpha=0`).  When `invert` is `true` the roles are swapped:
    /// inside becomes transparent and outside becomes opaque.
    ///
    /// - `x`, `y`: top-left corner of the rectangle (in pixels).
    /// - `width`, `height`: rectangle dimensions (must be > 0).
    /// - `invert`: when `false`, keeps the interior; when `true`, keeps the
    ///   exterior.
    ///
    /// `width` and `height` are validated in [`build`](FilterGraphBuilder::build);
    /// zero values return [`crate::FilterError::InvalidConfig`].
    ///
    /// The output carries an alpha channel (`rgba`).
    RectMask {
        /// Left edge of the rectangle (pixels from the left).
        x: u32,
        /// Top edge of the rectangle (pixels from the top).
        y: u32,
        /// Width of the rectangle in pixels (must be > 0).
        width: u32,
        /// Height of the rectangle in pixels (must be > 0).
        height: u32,
        /// When `true`, the mask is inverted: outside is opaque, inside is transparent.
        invert: bool,
    },

    /// Feather (soften) the alpha channel edges using a Gaussian blur.
    ///
    /// Splits the stream into a color copy and an alpha copy, blurs the alpha
    /// plane with `gblur=sigma=<radius>`, then re-merges:
    ///
    /// ```text
    /// [in]split=2[color][with_alpha];
    /// [with_alpha]alphaextract[alpha_only];
    /// [alpha_only]gblur=sigma=<radius>[alpha_blurred];
    /// [color][alpha_blurred]alphamerge[out]
    /// ```
    ///
    /// `radius` is the blur kernel half-size in pixels and must be > 0.
    /// Validated in [`build`](FilterGraphBuilder::build); `radius == 0` returns
    /// [`crate::FilterError::InvalidConfig`].
    ///
    /// Typically chained after a keying or masking step
    /// (e.g. [`FilterStep::ChromaKey`], [`FilterStep::RectMask`],
    /// [`FilterStep::PolygonMatte`]).  Applying this step to a fully-opaque
    /// video (no prior alpha) is a no-op because a uniform alpha of 255 blurs
    /// to 255 everywhere.
    FeatherMask {
        /// Gaussian blur kernel half-size in pixels (must be > 0).
        radius: u32,
    },

    /// Simulate motion blur by blending consecutive frames via `FFmpeg`'s `tblend` filter.
    ///
    /// `shutter_angle_degrees` controls the blend ratio; 360° equals a full
    /// frame-period exposure (maximum blur). `sub_frames` is the number of
    /// frames blended and must be in [2, 16]; it is validated by
    /// [`FilterGraph::motion_blur`](crate::FilterGraph::motion_blur).
    MotionBlur {
        /// Shutter angle in degrees (0° = no blur, 360° = full-period blur).
        shutter_angle_degrees: f32,
        /// Number of frames blended. Must be in [2, 16].
        sub_frames: u8,
    },

    /// Correct radial lens distortion using two polynomial coefficients via
    /// `FFmpeg`'s `lenscorrection` filter.
    ///
    /// Negative values correct barrel distortion; positive values correct
    /// pincushion distortion. Both `k1` and `k2` must be in [−1.0, 1.0];
    /// validated by [`FilterGraph::lens_correction`](crate::FilterGraph::lens_correction).
    LensCorrection {
        /// First-order radial distortion coefficient. Range: [−1.0, 1.0].
        k1: f32,
        /// Second-order radial distortion coefficient. Range: [−1.0, 1.0].
        k2: f32,
    },

    /// Add synthetic per-frame random film grain to luma and chroma channels
    /// via `FFmpeg`'s `noise` filter.
    ///
    /// `luma_strength` and `chroma_strength` are clamped to [0.0, 100.0].
    /// The `allf=t` flag varies the seed each frame to simulate real film grain.
    FilmGrain {
        /// Grain strength applied to the luma (Y) plane. Clamped to [0.0, 100.0].
        luma_strength: f32,
        /// Grain strength applied to the Cb and Cr planes. Clamped to [0.0, 100.0].
        chroma_strength: f32,
    },

    /// Uniform scale by a fractional multiplier via `FFmpeg`'s `scale` filter.
    ///
    /// Both width and height are multiplied by `factor`. Used to hide warped
    /// border pixels left after lens distortion correction.
    ScaleMultiplier {
        /// Scale factor applied to both dimensions (e.g. `1.05` = 5 % zoom-in).
        factor: f32,
    },

    /// Reduce lateral chromatic aberration by independently shifting the R and B
    /// channels via `FFmpeg`'s `rgbashift` filter.
    ///
    /// `rh` and `bh` are the horizontal pixel shifts for the red and blue
    /// channels respectively. Derived from scale deviations by
    /// [`FilterGraph::fix_chromatic_aberration`](crate::FilterGraph::fix_chromatic_aberration).
    ChromaticAberration {
        /// Horizontal shift for the red channel in pixels (positive = right).
        rh: i32,
        /// Horizontal shift for the blue channel in pixels (positive = right).
        bh: i32,
    },

    /// Glow / bloom effect: blends blurred highlights back over the image via
    /// `split`, `curves`, `gblur`, and `blend` filters.
    ///
    /// This is a compound step — see
    /// [`FilterGraph::glow`](crate::FilterGraph::glow) for parameter semantics.
    Glow {
        /// Luminance threshold that triggers the glow (clamped to [0.0, 1.0]).
        threshold: f32,
        /// Gaussian blur radius in pixels (clamped to [0.5, 50.0]).
        radius: f32,
        /// Additive blend strength (clamped to [0.0, 2.0]).
        intensity: f32,
    },

    /// Convolution reverb using an impulse response (IR) audio file.
    ///
    /// The IR is loaded via `FFmpeg`'s `amovie` filter, optionally delayed by
    /// `pre_delay_ms` via `adelay`, then convolved with the main audio stream
    /// via `FFmpeg`'s `afir` filter.
    ///
    /// This is a compound step — see
    /// [`FilterGraph::reverb_ir`](crate::FilterGraph::reverb_ir) for parameter
    /// semantics.
    ReverbIr {
        /// Absolute or relative path to the `.wav` or `.flac` IR file.
        ir_path: String,
        /// Wet (reverb) mix level in [0.0, 1.0].
        wet: f32,
        /// Dry (original) mix level in [0.0, 1.0].
        dry: f32,
        /// Pre-delay before the reverb tail in milliseconds (clamped to 0–500).
        pre_delay_ms: u32,
    },

    /// Algorithmic multi-tap echo/reverb via `FFmpeg`'s `aecho` filter.
    ///
    /// `in_gain` and `out_gain` are amplitude multipliers clamped to [0.0, 1.0].
    /// `delays` contains delay times in milliseconds (one per tap); `decays`
    /// contains the corresponding decay factors in [0.0, 1.0].  Both vecs must
    /// have equal length in the range 1–8; validated by
    /// [`FilterGraph::reverb_echo`](crate::FilterGraph::reverb_echo).
    ReverbEcho {
        /// Input gain (amplitude multiplier). Clamped to [0.0, 1.0].
        in_gain: f32,
        /// Output gain (amplitude multiplier). Clamped to [0.0, 1.0].
        out_gain: f32,
        /// Delay times in milliseconds (one per tap).
        delays: Vec<f32>,
        /// Decay factors per tap. Clamped to [0.0, 1.0].
        decays: Vec<f32>,
    },

    /// Pitch shift without tempo change.
    ///
    /// Shifts audio pitch by `semitones` semitones without altering playback
    /// duration.  Implemented as `asetrate` (changes the declared sample rate
    /// to shift pitch) followed by `atempo` (restores the original duration).
    ///
    /// Range: [−12.0, 12.0]; validated by
    /// [`FilterGraph::pitch_shift`](crate::FilterGraph::pitch_shift).
    ///
    /// This is a compound step — `filter_name()` returns `"asetrate"` for
    /// `validate_filter_steps`; the actual graph construction is handled by
    /// `filter_inner::build::build_audio_graph`.
    PitchShift {
        /// Pitch shift in semitones. Range: [−12.0, 12.0].
        semitones: f32,
    },

    /// Time-stretch audio without changing pitch via `FFmpeg`'s `atempo` filter.
    ///
    /// `factor < 1.0` = slower (longer duration); `factor > 1.0` = faster
    /// (shorter duration).  Range: [0.1, 10.0].  Values outside [0.5, 2.0]
    /// are realised by chaining multiple `atempo` instances (each in [0.5, 2.0]).
    ///
    /// Validated by [`FilterGraph::time_stretch`](crate::FilterGraph::time_stretch).
    TimeStretch {
        /// Speed / duration factor. 0.5 = 2× longer; 2.0 = 2× shorter. Range: [0.1, 10.0].
        factor: f32,
    },

    /// Simultaneously change audio speed and pitch by the same factor.
    ///
    /// Equivalent to playing a tape at a different speed: `factor > 1.0` makes
    /// audio faster and higher; `factor < 1.0` makes it slower and lower.
    ///
    /// Uses `FFmpeg`'s `asetrate` to multiply the declared sample rate by
    /// `factor` without resampling.  Range: [0.1, 10.0]; validated by
    /// [`FilterGraph::speed_change`](crate::FilterGraph::speed_change).
    SpeedChange {
        /// Speed/pitch multiplier. Range: [0.1, 10.0].
        factor: f64,
    },

    /// Spectral noise reduction using a statistical noise-type model.
    ///
    /// Uses `FFmpeg`'s `afftdn` filter.  `noise_type_flag` is the single-letter
    /// `nt` parameter (`"w"` = white, `"p"` = pink, `"b"` = brown).
    /// `nr_level` is the reduction amount in dB, clamped to [0.0, 97.0].
    ///
    /// Created by [`FilterGraph::noise_reduce`](crate::FilterGraph::noise_reduce).
    NoiseReduce {
        /// `afftdn` `nt` flag: `"w"`, `"p"`, or `"b"`.
        noise_type_flag: String,
        /// Noise reduction amount in dB. Clamped to [0.0, 97.0].
        nr_level: f32,
    },

    /// Spectral noise reduction using a captured noise profile.
    ///
    /// Uses `FFmpeg`'s `afftdn` with the `pl` (profile length) option: the
    /// filter learns the noise profile from the first `profile_duration_secs`
    /// seconds, then subtracts it from the rest of the stream.
    /// `nr_level` is the reduction amount in dB, clamped to [0.0, 97.0].
    ///
    /// Created by
    /// [`FilterGraph::noise_reduce_profile`](crate::FilterGraph::noise_reduce_profile).
    NoiseReduceProfile {
        /// Duration in seconds from which to capture the noise profile. Minimum 0.1.
        profile_duration_secs: f32,
        /// Noise reduction amount in dB. Clamped to [0.0, 97.0].
        nr_level: f32,
    },

    /// Sidechain compression for audio ducking via `FFmpeg`'s `sidechaincompress` filter.
    ///
    /// Reduces the background audio level when the foreground (sidechain) signal
    /// exceeds the threshold.  Push background audio to slot 0 and foreground
    /// audio to slot 1.
    ///
    /// `threshold_linear` is the trigger level as a linear amplitude (pre-converted
    /// from dBFS by [`FilterGraph::duck`](crate::FilterGraph::duck)).
    /// `ratio`, `attack_ms`, and `release_ms` are validated by
    /// [`FilterGraph::duck`](crate::FilterGraph::duck).
    Duck {
        /// Compression threshold as a linear amplitude ratio in (0.0, 1.0].
        threshold_linear: f32,
        /// Compression ratio (e.g. 20.0 for near hard-limiting). Must be >= 1.0.
        ratio: f32,
        /// Attack time in milliseconds. Must be >= 0.0.
        attack_ms: f32,
        /// Release time in milliseconds. Must be >= 0.0.
        release_ms: f32,
    },

    /// Apply a polygon alpha mask using `FFmpeg`'s `geq` filter with a
    /// crossing-number point-in-polygon test.
    ///
    /// Pixels inside the polygon are fully opaque (`alpha=255`); pixels outside
    /// are fully transparent (`alpha=0`).  When `invert` is `true` the roles
    /// are swapped.
    ///
    /// - `vertices`: polygon corners as `(x, y)` in `[0.0, 1.0]` (normalised
    ///   to frame size).  Minimum 3, maximum 16.
    /// - `invert`: when `false`, inside = opaque; when `true`, outside = opaque.
    ///
    /// Vertex count and coordinates are validated in
    /// [`build`](FilterGraphBuilder::build); out-of-range values return
    /// [`crate::FilterError::InvalidConfig`].
    ///
    /// The `geq` expression is constructed from the vertex list at graph
    /// build time.  Degenerate polygons (zero area) produce a fully-transparent
    /// mask.  The output carries an alpha channel (`rgba`).
    PolygonMatte {
        /// Polygon corners in normalised `[0.0, 1.0]` frame coordinates.
        vertices: Vec<(f32, f32)>,
        /// When `true`, the mask is inverted: outside is opaque, inside is transparent.
        invert: bool,
    },
}

/// Convert a color temperature in Kelvin to linear RGB multipliers using
/// Tanner Helland's algorithm.
///
/// Returns `(r, g, b)` each in `[0.0, 1.0]`.
fn kelvin_to_rgb(temp_k: u32) -> (f64, f64, f64) {
    let t = (f64::from(temp_k) / 100.0).clamp(10.0, 400.0);
    let r = if t <= 66.0 {
        1.0
    } else {
        (329.698_727_446_4 * (t - 60.0).powf(-0.133_204_759_2) / 255.0).clamp(0.0, 1.0)
    };
    let g = if t <= 66.0 {
        ((99.470_802_586_1 * t.ln() - 161.119_568_166_1) / 255.0).clamp(0.0, 1.0)
    } else {
        ((288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)) / 255.0).clamp(0.0, 1.0)
    };
    let b = if t >= 66.0 {
        1.0
    } else if t <= 19.0 {
        0.0
    } else {
        ((138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7) / 255.0).clamp(0.0, 1.0)
    };
    (r, g, b)
}

impl FilterStep {
    /// Returns the libavfilter filter name for this step.
    pub(crate) fn filter_name(&self) -> &'static str {
        match self {
            Self::Format { .. } => "format",
            Self::Trim { .. } => "trim",
            Self::Scale { .. } => "scale",
            Self::Crop { .. } => "crop",
            Self::Overlay { .. } => "overlay",
            Self::FadeIn { .. }
            | Self::FadeOut { .. }
            | Self::FadeInWhite { .. }
            | Self::FadeOutWhite { .. } => "fade",
            Self::AFadeIn { .. } | Self::AFadeOut { .. } => "afade",
            Self::Rotate { .. } => "rotate",
            Self::ToneMap(_) => "tonemap",
            Self::Volume(_) => "volume",
            Self::Amix(_) => "amix",
            // ParametricEq is a compound step; "equalizer" is used only by
            // validate_filter_steps as a best-effort existence check.  The
            // actual nodes are built by `filter_inner::add_parametric_eq_chain`.
            Self::ParametricEq { .. } => "equalizer",
            Self::Lut3d { .. } => "lut3d",
            Self::Eq { .. } => "eq",
            Self::EqAnimated { .. } => "eq",
            Self::ColorBalanceAnimated { .. } => "colorbalance",
            Self::Curves { .. } => "curves",
            Self::WhiteBalance { .. } => "colorchannelmixer",
            Self::Hue { .. } => "hue",
            Self::Gamma { .. } => "eq",
            Self::ThreeWayCC { .. } => "curves",
            Self::Vignette { .. } => "vignette",
            Self::HFlip => "hflip",
            Self::VFlip => "vflip",
            Self::Reverse => "reverse",
            Self::AReverse => "areverse",
            Self::Pad { .. } => "pad",
            // FitToAspect is implemented as scale + pad; "scale" is validated at
            // build time.  The pad filter is inserted by filter_inner at graph
            // construction time.
            Self::FitToAspect { .. } => "scale",
            Self::GBlur { .. } => "gblur",
            Self::Unsharp { .. } => "unsharp",
            Self::Hqdn3d { .. } => "hqdn3d",
            Self::Nlmeans { .. } => "nlmeans",
            Self::Yadif { .. } => "yadif",
            Self::XFade { .. } => "xfade",
            Self::DrawText { .. } | Self::Ticker { .. } => "drawtext",
            // "setpts" is checked at build-time; the audio path uses "atempo"
            // which is verified at graph-construction time in filter_inner.
            Self::Speed { .. } => "setpts",
            Self::FreezeFrame { .. } => "loop",
            Self::LoudnessNormalize { .. } => "ebur128",
            Self::NormalizePeak { .. } => "astats",
            Self::ANoiseGate { .. } => "agate",
            Self::ACompressor { .. } => "acompressor",
            Self::StereoToMono => "pan",
            Self::ChannelMap { .. } => "channelmap",
            // AudioDelay dispatches to adelay (positive) or atrim (negative) at
            // build time; "adelay" is returned here for validate_filter_steps only.
            Self::AudioDelay { .. } => "adelay",
            Self::ConcatVideo { .. } | Self::ConcatAudio { .. } => "concat",
            // JoinWithDissolve is a compound step (trim+setpts → xfade ← setpts+trim);
            // "xfade" is used by validate_filter_steps as the primary filter check.
            Self::JoinWithDissolve { .. } => "xfade",
            Self::SubtitlesSrt { .. } => "subtitles",
            Self::SubtitlesAss { .. } => "ass",
            // OverlayImage is a compound step (movie → lut → overlay); "overlay"
            // is used only by validate_filter_steps as a best-effort existence
            // check.  The actual graph construction is handled by
            // `filter_inner::build::add_overlay_image_step`.
            Self::OverlayImage { .. } => "overlay",
            // Blend is a compound step; "overlay" is used as the primary filter
            // for validate_filter_steps.  Unimplemented modes are caught by
            // build() before validate_filter_steps is reached.
            Self::Blend { .. } => "overlay",
            Self::ChromaKey { .. } => "chromakey",
            Self::ColorKey { .. } => "colorkey",
            Self::SpillSuppress { .. } => "hue",
            // AlphaMatte is a compound step (matte pipeline → alphamerge);
            // "alphamerge" is used by validate_filter_steps as the primary check.
            Self::AlphaMatte { .. } => "alphamerge",
            // LumaKey is a compound step when invert=true (lumakey + geq);
            // "lumakey" is used here for validate_filter_steps.
            Self::LumaKey { .. } => "lumakey",
            // RectMask uses geq to set alpha per-pixel based on rectangle bounds.
            Self::RectMask { .. } => "geq",
            // FeatherMask is a compound step (split → alphaextract → gblur → alphamerge);
            // "alphaextract" is used by validate_filter_steps as the primary check.
            Self::FeatherMask { .. } => "alphaextract",
            // PolygonMatte uses geq with a crossing-number point-in-polygon expression.
            Self::PolygonMatte { .. } => "geq",
            Self::CropAnimated { .. } => "crop",
            Self::GBlurAnimated { .. } => "gblur",
            Self::MotionBlur { .. } => "tblend",
            Self::LensCorrection { .. } => "lenscorrection",
            Self::FilmGrain { .. } => "noise",
            Self::ScaleMultiplier { .. } => "scale",
            Self::ChromaticAberration { .. } => "rgbashift",
            // Glow is a compound step (split → curves → gblur → blend);
            // "split" is used by validate_filter_steps as the primary check.
            Self::Glow { .. } => "split",
            // ReverbIr is a compound step (amovie[+adelay] → afir);
            // "afir" is used by validate_filter_steps as the primary check.
            Self::ReverbIr { .. } => "afir",
            Self::ReverbEcho { .. } => "aecho",
            // PitchShift is a compound step (asetrate → atempo);
            // "asetrate" is used by validate_filter_steps as the primary check.
            Self::PitchShift { .. } => "asetrate",
            // TimeStretch uses one or more chained atempo filters.
            Self::TimeStretch { .. } => "atempo",
            // SpeedChange uses asetrate to shift speed and pitch together.
            Self::SpeedChange { .. } => "asetrate",
            Self::NoiseReduce { .. } | Self::NoiseReduceProfile { .. } => "afftdn",
            // Duck is a two-input compound step; "sidechaincompress" is checked at
            // build time by validate_filter_steps.
            Self::Duck { .. } => "sidechaincompress",
        }
    }

    /// Returns the `args` string passed to `avfilter_graph_create_filter`.
    pub(crate) fn args(&self) -> String {
        match self {
            Self::Format {
                pix_fmts,
                color_spaces,
                color_ranges,
                alpha_modes,
            } => {
                let mut parts = vec![];
                if !pix_fmts.is_empty() {
                    parts.push(
                        iter::once("pix_fmts=")
                            .chain(
                                pix_fmts
                                    .iter()
                                    .flat_map(|pix_fmt| ["|", pix_fmt.name()])
                                    .skip(1),
                            )
                            .collect::<String>(),
                    );
                }
                if !color_spaces.is_empty() {
                    parts.push(
                        iter::once("color_spaces=")
                            .chain(
                                color_spaces
                                    .iter()
                                    .flat_map(|color_space| ["|", color_space.name()])
                                    .skip(1),
                            )
                            .collect::<String>(),
                    );
                }
                if !color_ranges.is_empty() {
                    parts.push(
                        iter::once("color_ranges=")
                            .chain(
                                color_ranges
                                    .iter()
                                    .flat_map(|color_range| ["|", color_range.name()])
                                    .skip(1),
                            )
                            .collect::<String>(),
                    );
                }
                if !alpha_modes.is_empty() {
                    parts.push(
                        iter::once("alpha_modes=")
                            .chain(
                                alpha_modes
                                    .iter()
                                    .flat_map(|alpha_mode| ["|", alpha_mode.name()])
                                    .skip(1),
                            )
                            .collect::<String>(),
                    );
                }
                parts.join(":")
            }
            Self::Trim { start, end } => format!("start={start}:end={end}"),
            Self::Scale {
                width,
                height,
                algorithm,
            } => format!("w={width}:h={height}:flags={}", algorithm.as_flags_str()),
            Self::Crop {
                x,
                y,
                width,
                height,
            } => {
                format!("x={x}:y={y}:w={width}:h={height}")
            }
            Self::Overlay { x, y } => format!("x={x}:y={y}"),
            Self::FadeIn { start, duration } => {
                format!("type=in:start_time={start}:duration={duration}")
            }
            Self::FadeOut { start, duration } => {
                format!("type=out:start_time={start}:duration={duration}")
            }
            Self::FadeInWhite { start, duration } => {
                format!("type=in:start_time={start}:duration={duration}:color=white")
            }
            Self::FadeOutWhite { start, duration } => {
                format!("type=out:start_time={start}:duration={duration}:color=white")
            }
            Self::AFadeIn { start, duration } => {
                format!("type=in:start_time={start}:duration={duration}")
            }
            Self::AFadeOut { start, duration } => {
                format!("type=out:start_time={start}:duration={duration}")
            }
            Self::Rotate {
                angle_degrees,
                fill_color,
            } => {
                format!(
                    "angle={}:fillcolor={fill_color}",
                    angle_degrees.to_radians()
                )
            }
            Self::ToneMap(algorithm) => format!("tonemap={}", algorithm.as_str()),
            Self::Volume(db) => format!("volume={db}dB"),
            Self::Amix(inputs) => format!("inputs={inputs}"),
            // args() for ParametricEq is not used by the build loop (which is
            // bypassed in favour of add_parametric_eq_chain); provided here for
            // completeness using the first band's args.
            Self::ParametricEq { bands } => bands.first().map(EqBand::args).unwrap_or_default(),
            Self::Lut3d { path } => {
                format!("file={}:interp=trilinear", escape_filter_path(path))
            }
            Self::Eq {
                brightness,
                contrast,
                saturation,
            } => format!("brightness={brightness}:contrast={contrast}:saturation={saturation}"),
            Self::EqAnimated {
                brightness,
                contrast,
                saturation,
                gamma,
            } => {
                let b = brightness.value_at(Duration::ZERO);
                let c = contrast.value_at(Duration::ZERO);
                let s = saturation.value_at(Duration::ZERO);
                let g = gamma.value_at(Duration::ZERO);
                format!("brightness={b}:contrast={c}:saturation={s}:gamma={g}")
            }
            Self::ColorBalanceAnimated { lift, gamma, gain } => {
                let (rl, gl, bl) = lift.value_at(Duration::ZERO);
                let (rm, gm, bm) = gamma.value_at(Duration::ZERO);
                let (rh, gh, bh) = gain.value_at(Duration::ZERO);
                format!("rs={rl}:gs={gl}:bs={bl}:rm={rm}:gm={gm}:bm={bm}:rh={rh}:gh={gh}:bh={bh}")
            }
            Self::Curves { master, r, g, b } => {
                let fmt = |pts: &[(f32, f32)]| -> String {
                    pts.iter()
                        .map(|(x, y)| format!("{x}/{y}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                [("master", master.as_slice()), ("r", r), ("g", g), ("b", b)]
                    .iter()
                    .filter(|(_, pts)| !pts.is_empty())
                    .map(|(name, pts)| format!("{name}='{}'", fmt(pts)))
                    .collect::<Vec<_>>()
                    .join(":")
            }
            Self::WhiteBalance {
                temperature_k,
                tint,
            } => {
                let (r, g, b) = kelvin_to_rgb(*temperature_k);
                let g_adj = (g + f64::from(*tint)).clamp(0.0, 2.0);
                format!("rr={r}:gg={g_adj}:bb={b}")
            }
            Self::Hue { degrees } => format!("h={degrees}"),
            Self::Gamma { r, g, b } => format!("gamma_r={r}:gamma_g={g}:gamma_b={b}"),
            Self::Vignette { angle, x0, y0 } => {
                let cx = if *x0 == 0.0 {
                    "w/2".to_string()
                } else {
                    x0.to_string()
                };
                let cy = if *y0 == 0.0 {
                    "h/2".to_string()
                } else {
                    y0.to_string()
                };
                format!("angle={angle}:x0={cx}:y0={cy}")
            }
            Self::ThreeWayCC { lift, gamma, gain } => {
                // Convert lift/gamma/gain to a 3-point per-channel curves representation.
                // The formula maps:
                //   input 0.0 → (lift - 1.0) * gain  (black point)
                //   input 0.5 → (0.5 * lift)^(1/gamma) * gain  (midtone)
                //   input 1.0 → gain  (white point)
                // All neutral (1.0) produces the identity curve 0/0 0.5/0.5 1/1.
                let curve = |l: f32, gm: f32, gn: f32| -> String {
                    let l = f64::from(l);
                    let gm = f64::from(gm);
                    let gn = f64::from(gn);
                    let black = ((l - 1.0) * gn).clamp(0.0, 1.0);
                    let mid = ((0.5 * l).powf(1.0 / gm) * gn).clamp(0.0, 1.0);
                    let white = gn.clamp(0.0, 1.0);
                    format!("0/{black} 0.5/{mid} 1/{white}")
                };
                format!(
                    "r='{}':g='{}':b='{}'",
                    curve(lift.r, gamma.r, gain.r),
                    curve(lift.g, gamma.g, gain.g),
                    curve(lift.b, gamma.b, gain.b),
                )
            }
            Self::HFlip | Self::VFlip | Self::Reverse | Self::AReverse => String::new(),
            Self::GBlur { sigma } => format!("sigma={sigma}"),
            Self::Unsharp {
                luma_strength,
                chroma_strength,
            } => format!(
                "luma_msize_x=5:luma_msize_y=5:luma_amount={luma_strength}:\
                 chroma_msize_x=5:chroma_msize_y=5:chroma_amount={chroma_strength}"
            ),
            Self::Hqdn3d {
                luma_spatial,
                chroma_spatial,
                luma_tmp,
                chroma_tmp,
            } => format!("{luma_spatial}:{chroma_spatial}:{luma_tmp}:{chroma_tmp}"),
            Self::Nlmeans { strength } => format!("s={strength}"),
            Self::Yadif { mode } => format!("mode={}", *mode as i32),
            Self::XFade {
                transition,
                duration,
                offset,
            } => {
                let t = transition.as_str();
                format!("transition={t}:duration={duration}:offset={offset}")
            }
            Self::DrawText { opts } => {
                // Escape special characters recognised by the drawtext filter.
                let escaped = opts
                    .text
                    .replace('\\', "\\\\")
                    .replace(':', "\\:")
                    .replace('\'', "\\'");
                let mut parts = vec![
                    format!("text='{escaped}'"),
                    format!("x={}", opts.x),
                    format!("y={}", opts.y),
                    format!("fontsize={}", opts.font_size),
                    format!("fontcolor={}@{:.2}", opts.font_color, opts.opacity),
                ];
                if let Some(ref ff) = opts.font_file {
                    parts.push(format!("fontfile={ff}"));
                }
                if let Some(ref bc) = opts.box_color {
                    parts.push("box=1".to_string());
                    parts.push(format!("boxcolor={bc}"));
                    parts.push(format!("boxborderw={}", opts.box_border_width));
                }
                parts.join(":")
            }
            Self::Ticker {
                text,
                y,
                speed_px_per_sec,
                font_size,
                font_color,
            } => {
                // Use the same escaping as DrawText.
                let escaped = text
                    .replace('\\', "\\\\")
                    .replace(':', "\\:")
                    .replace('\'', "\\'");
                // x = w - t * speed: at t=0 the text starts fully off the right
                // edge (x = w) and scrolls left by `speed` pixels per second.
                format!(
                    "text='{escaped}':x=w-t*{speed_px_per_sec}:y={y}:\
                     fontsize={font_size}:fontcolor={font_color}"
                )
            }
            // Video path: divide PTS by factor to change playback speed.
            // Audio path args are built by filter_inner (chained atempo).
            Self::Speed { factor } => format!("PTS/{factor}"),
            // args() is not used by the build loop for LoudnessNormalize (two-pass
            // is handled entirely in filter_inner); provided here for completeness.
            Self::LoudnessNormalize { .. } => "peak=true:metadata=1".to_string(),
            // args() is not used by the build loop for NormalizePeak (two-pass
            // is handled entirely in filter_inner); provided here for completeness.
            Self::NormalizePeak { .. } => "metadata=1".to_string(),
            Self::FreezeFrame { pts, duration } => {
                // The `loop` filter needs a frame index and a loop count, not PTS or
                // wall-clock duration.  We approximate both using 25 fps; accuracy
                // depends on the source stream's actual frame rate.
                #[allow(clippy::cast_possible_truncation)]
                let start = (*pts * 25.0) as i64;
                #[allow(clippy::cast_possible_truncation)]
                let loop_count = (*duration * 25.0) as i64;
                format!("loop={loop_count}:size=1:start={start}")
            }
            Self::SubtitlesSrt { path } | Self::SubtitlesAss { path } => {
                format!("filename={path}")
            }
            // args() for OverlayImage returns the overlay positional args (x:y).
            // These are not consumed by add_and_link_step (which is bypassed for
            // this compound step); they exist here only for completeness.
            Self::OverlayImage { x, y, .. } => format!("{x}:{y}"),
            // args() for Blend is not consumed by add_and_link_step (which is
            // bypassed in favour of add_blend_normal_step).  Provided for
            // completeness using the Normal-mode overlay args.
            Self::Blend { .. } => "format=auto:shortest=1".to_string(),
            Self::ChromaKey {
                color,
                similarity,
                blend,
            } => format!("color={color}:similarity={similarity}:blend={blend}"),
            Self::ColorKey {
                color,
                similarity,
                blend,
            } => format!("color={color}:similarity={similarity}:blend={blend}"),
            Self::SpillSuppress { strength, .. } => format!("h=0:s={}", 1.0 - strength),
            // args() is not consumed by add_and_link_step (which is bypassed for
            // this compound step); provided here for completeness.
            Self::AlphaMatte { .. } => String::new(),
            Self::LumaKey {
                threshold,
                tolerance,
                softness,
                ..
            } => format!("threshold={threshold}:tolerance={tolerance}:softness={softness}"),
            // args() is not consumed by add_and_link_step (which is bypassed for
            // this compound step); provided here for completeness.
            Self::FeatherMask { .. } => String::new(),
            Self::RectMask {
                x,
                y,
                width,
                height,
                invert,
            } => {
                let xw = x + width - 1;
                let yh = y + height - 1;
                let (inside, outside) = if *invert { (0, 255) } else { (255, 0) };
                format!(
                    "r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':\
                     a='if(between(X,{x},{xw})*between(Y,{y},{yh}),{inside},{outside})'"
                )
            }
            Self::PolygonMatte { vertices, invert } => {
                // Build a crossing-number point-in-polygon expression.
                // For each edge (ax,ay)→(bx,by), a horizontal ray from (X,Y) going
                // right crosses the edge when Y is in [min(ay,by), max(ay,by)) and
                // the intersection x > X.  Exact horizontal edges (dy==0) are skipped.
                let n = vertices.len();
                let mut edge_exprs = Vec::new();
                for i in 0..n {
                    let (ax, ay) = vertices[i];
                    let (bx, by) = vertices[(i + 1) % n];
                    let dy = by - ay;
                    if dy == 0.0 {
                        // Horizontal edge — never crosses a horizontal ray; skip.
                        continue;
                    }
                    let min_y = ay.min(by);
                    let max_y = ay.max(by);
                    let dx = bx - ax;
                    // x_intersect = ax*iw + (Y - ay*ih) * dx*iw / (dy*ih)
                    edge_exprs.push(format!(
                        "if(gte(Y,{min_y}*ih)*lt(Y,{max_y}*ih)*gt({ax}*iw+(Y-{ay}*ih)*{dx}*iw/({dy}*ih),X),1,0)"
                    ));
                }
                let sum = if edge_exprs.is_empty() {
                    "0".to_string()
                } else {
                    edge_exprs.join("+")
                };
                let (inside, outside) = if *invert { (0, 255) } else { (255, 0) };
                format!(
                    "r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':\
                     a='if(gt(mod({sum},2),0),{inside},{outside})'"
                )
            }
            Self::FitToAspect { width, height, .. } => {
                // Scale to fit within the target dimensions, preserving the source
                // aspect ratio.  The accompanying pad filter (inserted by
                // filter_inner after this scale filter) centres the result on the
                // target canvas.
                format!("w={width}:h={height}:force_original_aspect_ratio=decrease")
            }
            Self::Pad {
                width,
                height,
                x,
                y,
                color,
            } => {
                let px = if *x < 0 {
                    "(ow-iw)/2".to_string()
                } else {
                    x.to_string()
                };
                let py = if *y < 0 {
                    "(oh-ih)/2".to_string()
                } else {
                    y.to_string()
                };
                format!("width={width}:height={height}:x={px}:y={py}:color={color}")
            }
            Self::ANoiseGate {
                threshold_db,
                attack_ms,
                release_ms,
            } => {
                // `agate` expects threshold as a linear amplitude ratio (0.0–1.0).
                let threshold_linear = 10f32.powf(threshold_db / 20.0);
                format!("threshold={threshold_linear:.6}:attack={attack_ms}:release={release_ms}")
            }
            Self::ACompressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => {
                format!(
                    "threshold={threshold_db}dB:ratio={ratio}:attack={attack_ms}:\
                     release={release_ms}:makeup={makeup_db}dB"
                )
            }
            Self::StereoToMono => "mono|c0=0.5*c0+0.5*c1".to_string(),
            Self::ChannelMap { mapping } => format!("map={mapping}"),
            // args() is not used directly for AudioDelay — the audio build loop
            // dispatches to add_raw_filter_step with the correct filter name and
            // args based on the sign of ms.  These are provided for completeness.
            Self::AudioDelay { ms } => {
                if *ms >= 0.0 {
                    format!("delays={ms}:all=1")
                } else {
                    format!("start={}", -ms / 1000.0)
                }
            }
            Self::ConcatVideo { n } => format!("n={n}:v=1:a=0"),
            Self::ConcatAudio { n } => format!("n={n}:v=0:a=1"),
            // args() for JoinWithDissolve is not used by the build loop (which is
            // bypassed in favour of add_join_with_dissolve_step); provided here for
            // completeness using the xfade args.
            Self::JoinWithDissolve {
                clip_a_end,
                dissolve_dur,
                ..
            } => format!("transition=dissolve:duration={dissolve_dur}:offset={clip_a_end}"),
            Self::CropAnimated {
                x,
                y,
                width,
                height,
            } => {
                let x0 = x.value_at(Duration::ZERO);
                let y0 = y.value_at(Duration::ZERO);
                let w0 = width.value_at(Duration::ZERO);
                let h0 = height.value_at(Duration::ZERO);
                format!("x={x0}:y={y0}:w={w0}:h={h0}")
            }
            Self::GBlurAnimated { sigma } => {
                let s0 = sigma.value_at(Duration::ZERO);
                format!("sigma={s0}")
            }
            Self::MotionBlur {
                shutter_angle_degrees,
                ..
            } => {
                let alpha = f64::from(*shutter_angle_degrees / 360.0).clamp(0.0, 1.0);
                let keep = 1.0 - alpha;
                let blend = alpha;
                format!("all_expr='A*{keep}+B*{blend}'")
            }
            Self::LensCorrection { k1, k2 } => format!("k1={k1}:k2={k2}"),
            Self::FilmGrain {
                luma_strength,
                chroma_strength,
            } => {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let ls = luma_strength.clamp(0.0, 100.0) as u32;
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let cs = chroma_strength.clamp(0.0, 100.0) as u32;
                format!("alls={ls}:c0s={cs}:c1s={cs}:allf=t")
            }
            Self::ScaleMultiplier { factor } => {
                format!("w=iw*{factor}:h=ih*{factor}")
            }
            Self::ChromaticAberration { rh, bh } => {
                format!("rh={rh}:bh={bh}:edge=smear")
            }
            // args() is not consumed by add_and_link_step (which is bypassed for
            // this compound step); provided here for completeness.
            Self::Glow {
                threshold,
                radius,
                intensity,
            } => {
                let t = threshold.clamp(0.0, 1.0);
                let r = radius.clamp(0.5, 50.0);
                let iv = intensity.clamp(0.0, 2.0);
                let hi_lo = format!("0/0 {t}/0 1/1");
                format!(
                    "split=2[base][hl];[hl]curves=all='{hi_lo}'[glow_src];\
                     [glow_src]gblur=sigma={r}[glow];\
                     [base][glow]blend=all_mode=addition:all_opacity={iv}"
                )
            }
            Self::ReverbEcho {
                in_gain,
                out_gain,
                delays,
                decays,
            } => {
                let delay_str = delays
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("|");
                let decay_str = decays
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("|");
                format!(
                    "in_gain={ig}:out_gain={og}:delays={ds}:decays={dec}",
                    ig = in_gain,
                    og = out_gain,
                    ds = delay_str,
                    dec = decay_str,
                )
            }
            // args() is not consumed by add_and_link_step (which is bypassed for
            // this compound step); provided here for completeness.
            Self::ReverbIr {
                ir_path,
                wet,
                dry,
                pre_delay_ms,
            } => {
                let delay = pre_delay_ms.min(&500);
                let delay_part = if *delay > 0 {
                    format!(",adelay={delay}:all=1")
                } else {
                    String::new()
                };
                format!("amovie={ir_path}{delay_part}[ir];[0:a][ir]afir=dry={dry}:wet={wet}")
            }
            // args() is not consumed by add_and_link_step (which is bypassed for
            // this compound step); provided here for completeness.
            Self::PitchShift { semitones } => {
                let rate = 2f64.powf(f64::from(*semitones) / 12.0);
                let atempo = 1.0 / rate;
                format!("asetrate=sr*{rate:.6},atempo={atempo:.6}")
            }
            // args() is not consumed by add_and_link_step (bypassed in favour of
            // add_atempo_chain); provided here for single-instance completeness.
            Self::TimeStretch { factor } => format!("{factor:.6}"),
            // args() is not consumed by add_and_link_step (bypassed; sample rate
            // is resolved from buffersrc_args at build time); provided for completeness.
            Self::SpeedChange { factor } => format!("asetrate=sr*{factor:.6}"),
            Self::NoiseReduce {
                noise_type_flag,
                nr_level,
            } => format!("nt={noise_type_flag}:nr={nr_level}"),
            Self::NoiseReduceProfile {
                profile_duration_secs,
                nr_level,
            } => format!("nr={nr_level}:nf=-25:nt=w:pl={profile_duration_secs}"),
            // args() is not consumed by add_and_link_step (bypassed for this
            // compound two-input step); provided for completeness.
            Self::Duck {
                threshold_linear,
                ratio,
                attack_ms,
                release_ms,
            } => format!(
                "threshold={threshold_linear}:ratio={ratio}:attack={attack_ms}:release={release_ms}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_filter_path_should_escape_windows_drive_path() {
        // Backslashes → forward slashes; the drive colon is escaped as `\:` so
        // FFmpeg's filter-arg parser does not treat it as an option separator.
        assert_eq!(
            escape_filter_path(r"D:\dir\look.cube"),
            "D\\:/dir/look.cube"
        );
    }

    #[test]
    fn escape_filter_path_should_leave_unix_path_unchanged() {
        assert_eq!(escape_filter_path("/home/u/look.cube"), "/home/u/look.cube");
    }

    #[test]
    fn lut3d_args_should_escape_path() {
        let step = FilterStep::Lut3d {
            path: r"D:\luts\look.cube".to_string(),
        };
        let args = step.args();
        assert!(
            !args.contains(r"D:\"),
            "raw Windows path must not appear unescaped in args: {args}"
        );
        assert!(args.contains("D\\:/luts/look.cube"), "got: {args}");
        assert!(args.ends_with(":interp=trilinear"));
    }
}
