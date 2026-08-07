//! Unsafe `FFmpeg` filter-graph builders for multi-track composition.

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]

use std::path::PathBuf;
use std::ptr::NonNull;
use std::time::Duration;

use ff_format::ChannelLayout;

use crate::animation::{AnimatedValue, AnimationEntry};
use crate::blend::BlendMode;
use crate::composite::CompositeOp;
use crate::error::FilterError;
use crate::filter_inner::FilterGraphInner;
use crate::graph::FfmpegToken;
use crate::graph::filter_step::FilterStep;
use crate::graph::graph::FilterGraph;
use crate::graph::types::{Rgb, ScaleAlgorithm};

use super::multi_track_composer::VideoLayer;
use super::multi_track_mixer::AudioTrack;

/// Maps a `BlendMode` to the `FFmpeg` `blend` filter `all_mode` name.
///
/// Delegates to [`FfmpegToken`] (the single source of the 40-mode mapping).
/// `BlendMode::Normal` is not normally routed here (it uses the `overlay` filter);
/// the `"normal"` fallback is a defensive default.
fn blend_mode_to_ffmpeg(mode: BlendMode) -> &'static str {
    mode.ffmpeg_token().unwrap_or("normal")
}

// ── Video composition graph builder ──────────────────────────────────────────

pub(super) unsafe fn build_video_composition(
    canvas_width: u32,
    canvas_height: u32,
    background: Rgb,
    frame_rate: f64,
    layers: &[VideoLayer],
) -> Result<FilterGraph, FilterError> {
    use std::ffi::CString;

    // The canvas and every per-layer `fps` conform are generated at this rate.
    // It must equal the rate the caller encodes at, or the composited video is
    // stretched/compressed relative to the audio. `{}` keeps full f64 precision
    // (FFmpeg parses it via av_d2q, matching the encoder's rational).
    let fps_str = format!("{frame_rate}");

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::CompositionFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::CompositionFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // ── Base canvas ───────────────────────────────────────────────────────────
    let r = (background.r.clamp(0.0, 1.0) * 255.0) as u8;
    let g_ch = (background.g.clamp(0.0, 1.0) * 255.0) as u8;
    let b = (background.b.clamp(0.0, 1.0) * 255.0) as u8;
    let color_args_str =
        format!("c=#{r:02x}{g_ch:02x}{b:02x}:s={canvas_width}x{canvas_height}:r={fps_str}");
    let Ok(color_args) = CString::new(color_args_str.as_str()) else {
        bail!(graph, "CString::new failed for color filter args");
    };
    let color_filter = ff_sys::avfilter_get_by_name(c"color".as_ptr());
    if color_filter.is_null() {
        bail!(graph, "filter not found: color");
    }
    let mut base_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut base_ctx,
        color_filter,
        c"base".as_ptr(),
        color_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create color filter code={ret}"));
    }
    log::debug!(
        "video composition color source canvas={canvas_width}x{canvas_height} \
         color=#{r:02x}{g_ch:02x}{b:02x}"
    );

    let mut prev_ctx = base_ctx;
    let layer_count = layers.len();
    let mut animations: Vec<AnimationEntry> = Vec::new();

    // Pre-compute which layers should skip their overlay because the next
    // layer will cross-fade from them.  skip_overlay[i] is true when
    // layers[i+1] has an in_transition on the same z_order as layers[i].
    let skip_overlay: Vec<bool> = (0..layer_count)
        .map(|i| {
            layers.get(i + 1).is_some_and(|next| {
                next.in_transition.is_some() && next.z_order == layers[i].z_order
            })
        })
        .collect();
    // Saved chain_end from the preceding layer when skip_overlay was true.
    let mut saved_chain: *mut ff_sys::AVFilterContext = std::ptr::null_mut();

    for (idx, layer) in layers.iter().enumerate() {
        let is_last = idx == layer_count - 1;

        // ── movie= source ─────────────────────────────────────────────────────
        let movie_filter = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
        if movie_filter.is_null() {
            bail!(graph, "filter not found: movie");
        }
        let Ok(movie_name) = CString::new(format!("movie{idx}")) else {
            bail!(graph, "CString::new failed for movie name");
        };
        // When input_format is "lavfi", source is a filtergraph string opened via
        // FFmpeg's lavfi virtual demuxer; escape "\" and ":" for the option parser.
        // For regular files, normalise Windows backslashes and escape the drive colon.
        let movie_args_str = if layer.input_format.as_deref() == Some("lavfi") {
            let lavfi_str = layer.source.to_string_lossy();
            let escaped = lavfi_str.replace('\\', "\\\\").replace(':', "\\:");
            format!("filename={escaped}:format_name=lavfi")
        } else {
            // Decode from the proxy file when one is set; otherwise the original source.
            let decode_path = match &layer.proxy {
                Some(p) => p.path.as_path(),
                None => layer.source.as_path(),
            };
            let path = decode_path
                .to_string_lossy()
                .replace('\\', "/")
                .replace(':', "\\:");
            format!("filename={path}")
        };
        let Ok(movie_args) = CString::new(movie_args_str) else {
            bail!(graph, "CString::new failed for movie args");
        };
        let mut movie_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut movie_ctx,
            movie_filter,
            movie_name.as_ptr(),
            movie_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create movie filter layer={idx} code={ret}")
            );
        }
        log::debug!("video composition layer={idx} movie source");
        let mut chain_end = movie_ctx;

        // ── Proxy upscale ─────────────────────────────────────────────────────
        // When decoding from a proxy, scale the low-resolution frames up to the
        // original source resolution immediately, so trim/effects/compositing
        // all operate as if the original were used.
        if let Some(proxy) = &layer.proxy {
            let scale_filter = ff_sys::avfilter_get_by_name(c"scale".as_ptr());
            if scale_filter.is_null() {
                bail!(graph, "filter not found: scale");
            }
            let Ok(ps_name) = CString::new(format!("proxy_scale{idx}")) else {
                bail!(graph, "CString::new failed for proxy scale name");
            };
            let Ok(ps_args) = CString::new(format!("{}:{}", proxy.width, proxy.height)) else {
                bail!(graph, "CString::new failed for proxy scale args");
            };
            let mut ps_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut ps_ctx,
                scale_filter,
                ps_name.as_ptr(),
                ps_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create proxy scale filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, ps_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: movie→proxy_scale layer={idx}"));
            }
            chain_end = ps_ctx;
        }

        // ── Optional trim + setpts ────────────────────────────────────────────
        let trim_spec: Option<String> = match (layer.in_point, layer.out_point) {
            (Some(a), Some(b)) => {
                Some(format!("start={}:end={}", a.as_secs_f64(), b.as_secs_f64()))
            }
            (Some(a), None) => Some(format!("start={}", a.as_secs_f64())),
            (None, Some(b)) => Some(format!("end={}", b.as_secs_f64())),
            (None, None) => None,
        };
        if let Some(trim_args_str) = trim_spec {
            let trim_filter = ff_sys::avfilter_get_by_name(c"trim".as_ptr());
            if trim_filter.is_null() {
                bail!(graph, "filter not found: trim");
            }
            let Ok(trim_name) = CString::new(format!("trim{idx}")) else {
                bail!(graph, "CString::new failed for trim name");
            };
            let Ok(trim_args) = CString::new(trim_args_str) else {
                bail!(graph, "CString::new failed for trim args");
            };
            let mut trim_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut trim_ctx,
                trim_filter,
                trim_name.as_ptr(),
                trim_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create trim filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, trim_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: movie→trim layer={idx}"));
            }
            chain_end = trim_ctx;

            let setpts_filter = ff_sys::avfilter_get_by_name(c"setpts".as_ptr());
            if setpts_filter.is_null() {
                bail!(graph, "filter not found: setpts");
            }
            let Ok(sp_name) = CString::new(format!("setpts_trim{idx}")) else {
                bail!(graph, "CString::new failed for setpts name");
            };
            let mut sp_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut sp_ctx,
                setpts_filter,
                sp_name.as_ptr(),
                c"PTS-STARTPTS".as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create setpts filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, sp_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: trim→setpts layer={idx}"));
            }
            chain_end = sp_ctx;
        }

        // ── Optional timeline offset ──────────────────────────────────────────
        if layer.time_offset > Duration::ZERO {
            let offset = layer.time_offset.as_secs_f64();
            let setpts_filter = ff_sys::avfilter_get_by_name(c"setpts".as_ptr());
            if setpts_filter.is_null() {
                bail!(graph, "filter not found: setpts");
            }
            let Ok(sp_name) = CString::new(format!("setpts_offset{idx}")) else {
                bail!(graph, "CString::new failed for setpts offset name");
            };
            let Ok(sp_args) = CString::new(format!("PTS+{offset}/TB")) else {
                bail!(graph, "CString::new failed for setpts offset args");
            };
            let mut sp_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut sp_ctx,
                setpts_filter,
                sp_name.as_ptr(),
                sp_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create setpts offset filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, sp_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →setpts_offset layer={idx}"));
            }
            chain_end = sp_ctx;
        }

        // ── Optional scale ────────────────────────────────────────────────────
        let sx = layer.scale_x.value_at(Duration::ZERO);
        let sy = layer.scale_y.value_at(Duration::ZERO);
        if (sx - 1.0).abs() > f64::EPSILON || (sy - 1.0).abs() > f64::EPSILON {
            let sw = (f64::from(canvas_width) * sx).round() as u32;
            let sh = (f64::from(canvas_height) * sy).round() as u32;
            let scale_filter = ff_sys::avfilter_get_by_name(c"scale".as_ptr());
            if scale_filter.is_null() {
                bail!(graph, "filter not found: scale");
            }
            let Ok(sc_name) = CString::new(format!("scale{idx}")) else {
                bail!(graph, "CString::new failed for scale name");
            };
            let Ok(sc_args) = CString::new(format!("{sw}:{sh}")) else {
                bail!(graph, "CString::new failed for scale args");
            };
            let mut sc_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut sc_ctx,
                scale_filter,
                sc_name.as_ptr(),
                sc_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create scale filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, sc_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →scale layer={idx}"));
            }
            chain_end = sc_ctx;
        }

        // ── Optional rotation ─────────────────────────────────────────────────
        let rotation_deg = layer.rotation.value_at(Duration::ZERO);
        if rotation_deg.abs() > f64::EPSILON {
            let angle_rad = rotation_deg.to_radians();
            let rotate_filter = ff_sys::avfilter_get_by_name(c"rotate".as_ptr());
            if rotate_filter.is_null() {
                bail!(graph, "filter not found: rotate");
            }
            let Ok(rot_name) = CString::new(format!("layer_{idx}_rotate")) else {
                bail!(graph, "CString::new failed for rotate name");
            };
            let Ok(rot_args) = CString::new(format!("{angle_rad}")) else {
                bail!(graph, "CString::new failed for rotate args");
            };
            let mut rot_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut rot_ctx,
                rotate_filter,
                rot_name.as_ptr(),
                rot_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create rotate filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, rot_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →rotate layer={idx}"));
            }
            chain_end = rot_ctx;
            log::debug!("video composition layer={idx} rotate angle_deg={rotation_deg}");
        }

        // ── Optional opacity ──────────────────────────────────────────────────
        //
        // For BlendMode::Normal the opacity is applied via colorchannelmixer so
        // the overlay filter can blend the alpha channel.
        //
        // For photographic blend modes (Multiply, Screen, etc.) opacity is
        // forwarded to the blend filter's `all_opacity` parameter instead; the
        // colorchannelmixer step is skipped entirely.
        let use_blend_filter = !matches!(layer.blend_mode, BlendMode::Normal);
        let is_animated_opacity =
            !use_blend_filter && matches!(layer.opacity, AnimatedValue::Track(_));
        let opacity_initial = layer.opacity.value_at(Duration::ZERO).clamp(0.0, 1.0);
        // True for any Normal-mode layer whose opacity may be < 1.0 (static or animated).
        // Both cases require yuva420p + colorchannelmixer so the overlay can use alpha blending.
        // Only the default overlay path (composite_op == Over, Normal blend) needs the
        // colorchannelmixer alpha pre-step; a non-Over Porter-Duff op handles its own
        // opacity (via `all_opacity` for the expression operators) in its own branch.
        let needs_alpha = !use_blend_filter
            && matches!(layer.composite_op, CompositeOp::Over)
            && (is_animated_opacity || opacity_initial < 1.0);

        if needs_alpha {
            // ── format=yuva420p: add alpha plane before colorchannelmixer ─────
            let fmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
            if fmt_filter.is_null() {
                bail!(graph, "filter not found: format");
            }
            let Ok(fmt_name) = CString::new(format!("opacity_fmt{idx}")) else {
                bail!(graph, "CString::new failed for format name");
            };
            let mut fmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut fmt_ctx,
                fmt_filter,
                fmt_name.as_ptr(),
                c"yuva420p".as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create format filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, fmt_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →format layer={idx}"));
            }
            chain_end = fmt_ctx;

            // ── colorchannelmixer: scale alpha plane by opacity ────────────────
            let ccm_filter = ff_sys::avfilter_get_by_name(c"colorchannelmixer".as_ptr());
            if ccm_filter.is_null() {
                bail!(graph, "filter not found: colorchannelmixer");
            }
            let Ok(ccm_name) = CString::new(format!("ccm{idx}")) else {
                bail!(graph, "CString::new failed for colorchannelmixer name");
            };
            let Ok(ccm_args) = CString::new(format!("aa={opacity_initial:.6}")) else {
                bail!(graph, "CString::new failed for colorchannelmixer args");
            };
            let mut ccm_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut ccm_ctx,
                ccm_filter,
                ccm_name.as_ptr(),
                ccm_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create colorchannelmixer filter layer={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, ccm_ctx, 0);
            if ret < 0 {
                bail!(
                    graph,
                    format!("link failed: →colorchannelmixer layer={idx}")
                );
            }
            chain_end = ccm_ctx;

            // Register animation entry so tick() can update alpha per-frame.
            if let AnimatedValue::Track(ref track) = layer.opacity {
                animations.push(AnimationEntry {
                    node_name: format!("ccm{idx}"),
                    param: "aa",
                    track: track.clone(),
                    suffix: "",
                });
            }
        }

        // ── Per-layer video effects ───────────────────────────────────────────
        for (eff_idx, step) in layer.effects.iter().enumerate() {
            let combined_idx = idx * 1000 + eff_idx;
            let result = crate::filter_inner::add_and_link_step(
                graph,
                chain_end,
                step,
                combined_idx,
                "veff",
            );
            match result {
                Ok(ctx) => {
                    chain_end = ctx;
                    // setpts=PTS/factor compresses timestamps but in a pull-based filter graph
                    // the overlay consumes one movie frame per output frame (arrival order).
                    // An fps filter after setpts forces PTS-based frame selection, dropping or
                    // duplicating frames so the overlay receives the correct N/factor frames.
                    if let crate::FilterStep::Speed { .. } = step {
                        let fps_filter = ff_sys::avfilter_get_by_name(c"fps".as_ptr());
                        if fps_filter.is_null() {
                            bail!(graph, "filter not found: fps (speed)");
                        }
                        let Ok(fps_name) = CString::new(format!("fps_spd{combined_idx}")) else {
                            bail!(graph, "CString::new failed for fps_spd name");
                        };
                        let mut fps_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                        // Must match the canvas rate (`fps_str`), not a hardcoded 30.
                        let Ok(fps_args) = CString::new(format!("fps={fps_str}")) else {
                            bail!(graph, "CString::new failed for fps args");
                        };
                        let ret = ff_sys::avfilter_graph_create_filter(
                            &raw mut fps_ctx,
                            fps_filter,
                            fps_name.as_ptr(),
                            fps_args.as_ptr(),
                            std::ptr::null_mut(),
                            graph,
                        );
                        if ret < 0 {
                            bail!(
                                graph,
                                format!(
                                    "failed to create fps filter for speed \
                                     layer={idx} combined_idx={combined_idx}"
                                )
                            );
                        }
                        let ret = ff_sys::avfilter_link(chain_end, 0, fps_ctx, 0);
                        if ret < 0 {
                            bail!(graph, "link failed: setpts→fps_spd");
                        }
                        chain_end = fps_ctx;
                    }
                }
                Err(e) => bail!(
                    graph,
                    format!("failed to apply video effect layer={idx} eff={eff_idx}: {e}")
                ),
            }
        }

        // ── xfade (when this layer has an in_transition) ──────────────────────
        if let Some(ref t) = layer.in_transition {
            if !saved_chain.is_null() {
                // Wire: clip A (saved_chain) × clip B (chain_end) → xfade
                let xfade_filter = ff_sys::avfilter_get_by_name(c"xfade".as_ptr());
                if xfade_filter.is_null() {
                    bail!(graph, "filter not found: xfade");
                }
                let xfade_args_str = format!(
                    "transition={}:duration={}:offset={}",
                    t.kind.as_str(),
                    t.duration_secs,
                    t.offset_secs,
                );
                let Ok(xfade_args) = CString::new(xfade_args_str.as_str()) else {
                    bail!(graph, "CString::new failed for xfade args");
                };
                let Ok(xfade_name) = CString::new(format!("xfade{idx}")) else {
                    bail!(graph, "CString::new failed for xfade name");
                };
                let mut xfade_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                let ret = ff_sys::avfilter_graph_create_filter(
                    &raw mut xfade_ctx,
                    xfade_filter,
                    xfade_name.as_ptr(),
                    xfade_args.as_ptr(),
                    std::ptr::null_mut(),
                    graph,
                );
                if ret < 0 {
                    bail!(
                        graph,
                        format!("failed to create xfade filter layer={idx} code={ret}")
                    );
                }
                // clip A → xfade input 0; clip B → xfade input 1
                let ret = ff_sys::avfilter_link(saved_chain, 0, xfade_ctx, 0);
                if ret < 0 {
                    bail!(graph, format!("link failed: A→xfade[0] layer={idx}"));
                }
                let ret = ff_sys::avfilter_link(chain_end, 0, xfade_ctx, 1);
                if ret < 0 {
                    bail!(graph, format!("link failed: B→xfade[1] layer={idx}"));
                }
                saved_chain = std::ptr::null_mut();
                chain_end = xfade_ctx;
                log::debug!(
                    "video composition xfade layer={idx} kind={} dur={} offset={}",
                    t.kind.as_str(),
                    t.duration_secs,
                    t.offset_secs,
                );
            } else {
                log::warn!(
                    "video composition layer={idx} has in_transition but no preceding \
                     layer on same z_order; hard cut applied"
                );
            }
        }

        // ── Skip overlay (save chain_end for upcoming xfade) or overlay ────────
        if skip_overlay[idx] {
            // The next layer will xfade from this one; defer the overlay.
            saved_chain = chain_end;
        } else if layer.composite_op != CompositeOp::Over {
            // ── Porter-Duff alpha compositing (#1222) ─────────────────────────────
            //
            // A non-`Over` composite op drives this layer's compositing; the colour
            // `blend_mode` is not additionally applied. `A` = bottom (canvas) = pad 0,
            // `B` = top (layer) = pad 1.
            if let Some(expr) = layer.composite_op.blend_all_expr() {
                // In/Out/Atop/Xor → `blend all_expr=<expr>`. Normalise the layer to
                // yuv420p so both blend inputs share a format (as the photographic
                // blend branch does). Opacity is forwarded via `all_opacity`.
                let fmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
                if fmt_filter.is_null() {
                    bail!(graph, "filter not found: format (composite pre-norm)");
                }
                let Ok(cfmt_name) = CString::new(format!("composite_fmt{idx}")) else {
                    bail!(graph, "CString::new failed for composite format name");
                };
                let mut cfmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                let ret = ff_sys::avfilter_graph_create_filter(
                    &raw mut cfmt_ctx,
                    fmt_filter,
                    cfmt_name.as_ptr(),
                    c"yuv420p".as_ptr(),
                    std::ptr::null_mut(),
                    graph,
                );
                if ret < 0 {
                    bail!(
                        graph,
                        format!("failed to create composite format filter layer={idx} code={ret}")
                    );
                }
                let ret = ff_sys::avfilter_link(chain_end, 0, cfmt_ctx, 0);
                if ret < 0 {
                    bail!(graph, format!("link failed: →composite_fmt layer={idx}"));
                }
                chain_end = cfmt_ctx;

                let blend_filter = ff_sys::avfilter_get_by_name(c"blend".as_ptr());
                if blend_filter.is_null() {
                    bail!(graph, "filter not found: blend (composite)");
                }
                let Ok(cbl_name) = CString::new(format!("composite_blend{idx}")) else {
                    bail!(graph, "CString::new failed for composite blend name");
                };
                // `endall` on the last layer terminates output when its finite input
                // EOFs (see the photographic blend branch); without it the blend runs
                // forever against the infinite canvas.
                let eof_action = if is_last { "endall" } else { "pass" };
                let cbl_args_str = if (opacity_initial - 1.0).abs() < f64::from(f32::EPSILON) {
                    format!("all_expr={expr}:eof_action={eof_action}")
                } else {
                    format!(
                        "all_expr={expr}:all_opacity={opacity_initial:.6}:eof_action={eof_action}"
                    )
                };
                let Ok(cbl_args) = CString::new(cbl_args_str.as_str()) else {
                    bail!(graph, "CString::new failed for composite blend args");
                };
                let mut cblend_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                let ret = ff_sys::avfilter_graph_create_filter(
                    &raw mut cblend_ctx,
                    blend_filter,
                    cbl_name.as_ptr(),
                    cbl_args.as_ptr(),
                    std::ptr::null_mut(),
                    graph,
                );
                if ret < 0 {
                    bail!(
                        graph,
                        format!(
                            "failed to create composite blend layer={idx} args={cbl_args_str} code={ret}"
                        )
                    );
                }
                let ret = ff_sys::avfilter_link(prev_ctx, 0, cblend_ctx, 0);
                if ret < 0 {
                    bail!(
                        graph,
                        format!("link failed: base→composite_blend[0] layer={idx}")
                    );
                }
                let ret = ff_sys::avfilter_link(chain_end, 0, cblend_ctx, 1);
                if ret < 0 {
                    bail!(
                        graph,
                        format!("link failed: layer→composite_blend[1] layer={idx}")
                    );
                }
                log::debug!(
                    "video composition layer={idx} composite_op={:?} opacity={opacity_initial:.3}",
                    layer.composite_op
                );
                prev_ctx = cblend_ctx;
            } else {
                // Under → `overlay` with swapped pads: layer = pad 0 (bottom),
                // canvas = pad 1 (top), so the canvas composites over the layer.
                //
                // When opacity < 1.0, attenuate the layer's (pad 0) alpha via
                // colorchannelmixer before the swapped overlay — mirroring
                // `add_blend_under_step` so both Under paths handle opacity the
                // same way. The overlay below must use `format=auto` (not the
                // `yuv420` default, which carries no alpha) for the attenuated
                // alpha to survive. Animated opacity on a non-Over layer is not
                // tracked here; the initial value is used, consistent with the
                // expression operators above.
                if opacity_initial < 1.0 {
                    let fmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
                    if fmt_filter.is_null() {
                        bail!(graph, "filter not found: format (composite under opacity)");
                    }
                    let Ok(ufmt_name) = CString::new(format!("composite_under_fmt{idx}")) else {
                        bail!(graph, "CString::new failed for composite under format name");
                    };
                    let mut ufmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                    let ret = ff_sys::avfilter_graph_create_filter(
                        &raw mut ufmt_ctx,
                        fmt_filter,
                        ufmt_name.as_ptr(),
                        c"yuva420p".as_ptr(),
                        std::ptr::null_mut(),
                        graph,
                    );
                    if ret < 0 {
                        bail!(
                            graph,
                            format!(
                                "failed to create composite under format filter layer={idx} code={ret}"
                            )
                        );
                    }
                    let ret = ff_sys::avfilter_link(chain_end, 0, ufmt_ctx, 0);
                    if ret < 0 {
                        bail!(
                            graph,
                            format!("link failed: →composite_under_fmt layer={idx}")
                        );
                    }
                    chain_end = ufmt_ctx;

                    let ccm_filter = ff_sys::avfilter_get_by_name(c"colorchannelmixer".as_ptr());
                    if ccm_filter.is_null() {
                        bail!(
                            graph,
                            "filter not found: colorchannelmixer (composite under)"
                        );
                    }
                    let Ok(uccm_name) = CString::new(format!("composite_under_ccm{idx}")) else {
                        bail!(graph, "CString::new failed for composite under ccm name");
                    };
                    let Ok(uccm_args) = CString::new(format!("aa={opacity_initial:.6}")) else {
                        bail!(graph, "CString::new failed for composite under ccm args");
                    };
                    let mut uccm_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                    let ret = ff_sys::avfilter_graph_create_filter(
                        &raw mut uccm_ctx,
                        ccm_filter,
                        uccm_name.as_ptr(),
                        uccm_args.as_ptr(),
                        std::ptr::null_mut(),
                        graph,
                    );
                    if ret < 0 {
                        bail!(
                            graph,
                            format!(
                                "failed to create composite under colorchannelmixer layer={idx} code={ret}"
                            )
                        );
                    }
                    let ret = ff_sys::avfilter_link(chain_end, 0, uccm_ctx, 0);
                    if ret < 0 {
                        bail!(
                            graph,
                            format!("link failed: →composite_under_ccm layer={idx}")
                        );
                    }
                    chain_end = uccm_ctx;
                }

                let eof_action = if is_last { "endall" } else { "pass" };
                let overlay_filter = ff_sys::avfilter_get_by_name(c"overlay".as_ptr());
                if overlay_filter.is_null() {
                    bail!(graph, "filter not found: overlay (composite under)");
                }
                let Ok(uov_name) = CString::new(format!("composite_under{idx}")) else {
                    bail!(graph, "CString::new failed for composite under name");
                };
                let Ok(uov_args) = CString::new(format!("0:0:eof_action={eof_action}:format=auto"))
                else {
                    bail!(graph, "CString::new failed for composite under args");
                };
                let mut uov_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                let ret = ff_sys::avfilter_graph_create_filter(
                    &raw mut uov_ctx,
                    overlay_filter,
                    uov_name.as_ptr(),
                    uov_args.as_ptr(),
                    std::ptr::null_mut(),
                    graph,
                );
                if ret < 0 {
                    bail!(
                        graph,
                        format!("failed to create composite under overlay layer={idx} code={ret}")
                    );
                }
                let ret = ff_sys::avfilter_link(chain_end, 0, uov_ctx, 0);
                if ret < 0 {
                    bail!(
                        graph,
                        format!("link failed: layer→under_overlay[0] layer={idx}")
                    );
                }
                let ret = ff_sys::avfilter_link(prev_ctx, 0, uov_ctx, 1);
                if ret < 0 {
                    bail!(
                        graph,
                        format!("link failed: base→under_overlay[1] layer={idx}")
                    );
                }
                prev_ctx = uov_ctx;
            }
        } else if use_blend_filter {
            // ── Photographic blend mode (Multiply, Screen, Overlay, etc.) ────────
            //
            // FFmpeg's `blend` filter takes two same-sized inputs and applies a
            // pixel-level blend operation.  Opacity is forwarded via `all_opacity`.
            // The `blend` filter does not support positional (x/y) placement.
            //
            // Normalise the layer to yuv420p so both inputs share the same format.
            let fmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
            if fmt_filter.is_null() {
                bail!(graph, "filter not found: format (blend pre-norm)");
            }
            let Ok(bfmt_name) = CString::new(format!("blend_fmt{idx}")) else {
                bail!(graph, "CString::new failed for blend format name");
            };
            let mut bfmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut bfmt_ctx,
                fmt_filter,
                bfmt_name.as_ptr(),
                c"yuv420p".as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!(
                        "failed to create format filter (blend pre-norm) layer={idx} code={ret}"
                    )
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, bfmt_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →blend_fmt layer={idx}"));
            }
            chain_end = bfmt_ctx;

            let blend_filter = ff_sys::avfilter_get_by_name(c"blend".as_ptr());
            if blend_filter.is_null() {
                bail!(graph, "filter not found: blend");
            }
            let Ok(bl_name) = CString::new(format!("blend{idx}")) else {
                bail!(graph, "CString::new failed for blend name");
            };
            let mode_name = blend_mode_to_ffmpeg(layer.blend_mode);
            // Terminate the composition like the `overlay` path: `endall` on the
            // last layer ends output when its (finite) input EOFs. Without this the
            // `blend` filter's framesync defaults to `repeat` and runs forever
            // against the infinite `color` canvas that earlier `pass` overlays
            // forward — hanging the render.
            let eof_action = if is_last { "endall" } else { "pass" };
            let bl_args_str = if (opacity_initial - 1.0).abs() < f64::from(f32::EPSILON) {
                format!("all_mode={mode_name}:eof_action={eof_action}")
            } else {
                format!(
                    "all_mode={mode_name}:all_opacity={opacity_initial:.6}:eof_action={eof_action}"
                )
            };
            let Ok(bl_args) = CString::new(bl_args_str.as_str()) else {
                bail!(graph, "CString::new failed for blend args");
            };
            let mut blend_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut blend_ctx,
                blend_filter,
                bl_name.as_ptr(),
                bl_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!(
                        "failed to create blend filter layer={idx} args={bl_args_str} code={ret}"
                    )
                );
            }
            // blend pad 0 = bottom (base canvas), pad 1 = top (layer content)
            let ret = ff_sys::avfilter_link(prev_ctx, 0, blend_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: base→blend[0] layer={idx}"));
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, blend_ctx, 1);
            if ret < 0 {
                bail!(graph, format!("link failed: layer→blend[1] layer={idx}"));
            }
            log::debug!(
                "video composition layer={idx} blend mode={mode_name} opacity={opacity_initial:.3}"
            );
            prev_ctx = blend_ctx;
        } else {
            // ── Normal blend mode: overlay ────────────────────────────────────────
            // Last layer uses eof_action=endall so the graph terminates when that
            // layer's source ends.  Intermediate layers use pass so the canvas
            // continues while other layers are still producing.
            //
            // When the overlay input has an alpha channel (animated opacity path),
            // `format=auto` tells FFmpeg to blend using that alpha.
            let eof_action = if is_last { "endall" } else { "pass" };
            let overlay_filter = ff_sys::avfilter_get_by_name(c"overlay".as_ptr());
            if overlay_filter.is_null() {
                bail!(graph, "filter not found: overlay");
            }
            let Ok(ov_name) = CString::new(format!("overlay{idx}")) else {
                bail!(graph, "CString::new failed for overlay name");
            };
            let lx = layer.x.value_at(Duration::ZERO).round() as i64;
            let ly = layer.y.value_at(Duration::ZERO).round() as i64;
            let needs_eval_frame = matches!(layer.x, AnimatedValue::Track(_))
                || matches!(layer.y, AnimatedValue::Track(_));
            let eval_suffix = if needs_eval_frame { ":eval=frame" } else { "" };
            let format_suffix = if needs_alpha { ":format=auto" } else { "" };
            let Ok(ov_args) = CString::new(format!(
                "{lx}:{ly}:eof_action={eof_action}{eval_suffix}{format_suffix}"
            )) else {
                bail!(graph, "CString::new failed for overlay args");
            };
            let mut overlay_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut overlay_ctx,
                overlay_filter,
                ov_name.as_ptr(),
                ov_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create overlay filter layer={idx} code={ret}")
                );
            }
            // Link: base canvas → overlay pad 0
            let ret = ff_sys::avfilter_link(prev_ctx, 0, overlay_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: base→overlay[0] layer={idx}"));
            }
            // Link: layer content → overlay pad 1
            let ret = ff_sys::avfilter_link(chain_end, 0, overlay_ctx, 1);
            if ret < 0 {
                bail!(graph, format!("link failed: layer→overlay[1] layer={idx}"));
            }

            // Register animation entries for animated x/y.
            if let AnimatedValue::Track(ref track) = layer.x {
                animations.push(AnimationEntry {
                    node_name: format!("overlay{idx}"),
                    param: "x",
                    track: track.clone(),
                    suffix: "",
                });
            }
            if let AnimatedValue::Track(ref track) = layer.y {
                animations.push(AnimationEntry {
                    node_name: format!("overlay{idx}"),
                    param: "y",
                    track: track.clone(),
                    suffix: "",
                });
            }

            prev_ctx = overlay_ctx;
        }
    }

    // ── format=yuv420p: constrain output pixel format before sink ────────────
    let vfmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
    if vfmt_filter.is_null() {
        bail!(graph, "filter not found: format");
    }
    let mut vfmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vfmt_ctx,
        vfmt_filter,
        c"vformat".as_ptr(),
        c"pix_fmts=yuv420p".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create format filter code={ret}"));
    }
    let ret = ff_sys::avfilter_link(prev_ctx, 0, vfmt_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: last→format");
    }
    log::debug!("video composition format filter inserted output=yuv420p");

    // ── Video buffersink ──────────────────────────────────────────────────────
    let sink_filter = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if sink_filter.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filter,
        c"vsink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create buffersink code={ret}"));
    }
    let ret = ff_sys::avfilter_link(vfmt_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: format→buffersink");
    }

    // ── Configure graph ───────────────────────────────────────────────────────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        log::warn!("video composition avfilter_graph_config failed code={ret}");
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // SAFETY: ret >= 0 guarantees both pointers are non-null.
    let graph_nn = NonNull::new_unchecked(graph);
    let sink_nn = NonNull::new_unchecked(sink_ctx);
    let inner = FilterGraphInner::with_prebuilt_video_graph(graph_nn, sink_nn);
    log::info!(
        "video composition graph built layers={layer_count} canvas={canvas_width}x{canvas_height}"
    );
    if animations.is_empty() {
        Ok(FilterGraph::from_prebuilt(inner))
    } else {
        Ok(FilterGraph::from_prebuilt_animated(inner, animations))
    }
}

// ── Real-time composition graph builder ───────────────────────────────────────

/// Builds a `buffersrc`-fed composition graph for real-time preview.
///
/// Unlike [`build_video_composition`] (which decodes internally via `movie`
/// sources and is pulled to completion), this creates one `buffersrc` per layer
/// so a host feeds externally-decoded frames per layer per tick. Layer 0 is the
/// base (its [`effects`](super::realtime_composer::RealtimeLayer::effects) are
/// applied directly); layers 1.. are composited on top via the **same**
/// `add_blend_normal_step` / `add_blend_photographic_step` primitives the export
/// path uses, so the per-clip effects and blend modes match the rendered output.
/// Output is `rgba` for direct read-back.
///
/// # Safety
///
/// Follows the same avfilter ownership rules as [`build_video_composition`].
pub(super) unsafe fn build_realtime_composition(
    layers: &[super::realtime_composer::RealtimeLayer],
    canvas: Option<(u32, u32)>,
) -> Result<FilterGraph, FilterError> {
    use std::ffi::CString;

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::CompositionFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    if layers.is_empty() {
        return Err(FilterError::CompositionFailed {
            reason: "no layers".to_string(),
        });
    }

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::CompositionFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // ── One buffersrc per layer ───────────────────────────────────────────────
    let buffer_filter = ff_sys::avfilter_get_by_name(c"buffer".as_ptr());
    if buffer_filter.is_null() {
        bail!(graph, "filter not found: buffer");
    }
    let mut src_ctxs: Vec<Option<NonNull<ff_sys::AVFilterContext>>> =
        Vec::with_capacity(layers.len());
    for (idx, layer) in layers.iter().enumerate() {
        let pix = crate::filter_inner::pixel_format_to_av(layer.pixel_format);
        let args_str = crate::filter_inner::video_buffersrc_args(layer.width, layer.height, pix);
        let Ok(args) = CString::new(args_str.as_str()) else {
            bail!(graph, "CString::new failed for buffersrc args");
        };
        let Ok(name) = CString::new(format!("in{idx}")) else {
            bail!(graph, "CString::new failed for buffersrc name");
        };
        let mut ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut ctx,
            buffer_filter,
            name.as_ptr(),
            args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create buffersrc layer={idx} code={ret}")
            );
        }
        src_ctxs.push(Some(NonNull::new_unchecked(ctx)));
    }

    // ── Base = layer 0's buffersrc with its effects applied ───────────────────
    let Some(base_src) = src_ctxs[0] else {
        bail!(graph, "layer 0 buffersrc missing");
    };
    let mut acc = base_src.as_ptr();
    for (j, step) in layers[0].effects.iter().enumerate() {
        acc = match crate::filter_inner::add_and_link_step(graph, acc, step, j, "rt_base") {
            Ok(c) => c,
            Err(e) => bail!(graph, format!("base layer effect failed: {e:?}")),
        };
    }

    // Canvas dimensions are defined by the base layer; every other layer is
    // scaled to match so the `overlay`/`blend` inputs share the same size (the
    // `blend` filter requires identical input dimensions).
    let (canvas_w, canvas_h) = (layers[0].width, layers[0].height);

    // ── Composite layers 1.. onto the accumulator ─────────────────────────────
    for idx in 1..layers.len() {
        let layer = &layers[idx];
        let Some(top_src) = src_ctxs[idx] else {
            bail!(graph, format!("missing buffersrc for layer={idx}"));
        };
        // Scale the top layer to the canvas size first, then apply its effects.
        let mut top_steps: Vec<FilterStep> = Vec::with_capacity(layer.effects.len() + 1);
        top_steps.push(FilterStep::Scale {
            width: canvas_w,
            height: canvas_h,
            algorithm: ScaleAlgorithm::Fast,
        });
        top_steps.extend(layer.effects.iter().cloned());
        acc = if layer.blend_mode == BlendMode::Normal {
            match crate::filter_inner::add_blend_normal_step(
                graph,
                acc,
                top_src.as_ptr(),
                &top_steps,
                layer.opacity,
                ff_format::AlphaMode::Straight,
                idx,
            ) {
                Ok(c) => c,
                Err(e) => bail!(graph, format!("normal blend failed layer={idx}: {e:?}")),
            }
        } else {
            let mode_name = blend_mode_to_ffmpeg(layer.blend_mode);
            match crate::filter_inner::add_blend_photographic_step(
                graph,
                acc,
                top_src.as_ptr(),
                &top_steps,
                mode_name,
                layer.opacity,
                idx,
            ) {
                Ok(c) => c,
                Err(e) => bail!(graph, format!("blend failed layer={idx}: {e:?}")),
            }
        };
    }

    // ── Optional fit-to-canvas ────────────────────────────────────────────────
    // When a project canvas is set, letterbox/pillarbox the composited result to
    // exactly `canvas_w × canvas_h` so the preview frame matches the project's
    // output aspect. Runtime expressions (`force_original_aspect_ratio`,
    // `(ow-iw)/2`) keep this correct regardless of the accumulator's actual size
    // (per-clip effects may have resized it).
    if let Some((canvas_w, canvas_h)) = canvas {
        let scale_filter = ff_sys::avfilter_get_by_name(c"scale".as_ptr());
        if scale_filter.is_null() {
            bail!(graph, "filter not found: scale (fit-to-canvas)");
        }
        let Ok(fit_args) = CString::new(format!(
            "w={canvas_w}:h={canvas_h}:force_original_aspect_ratio=decrease:force_divisible_by=2"
        )) else {
            bail!(graph, "CString::new failed for fit-to-canvas scale args");
        };
        let mut fit_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut fit_ctx,
            scale_filter,
            c"canvas_fit".as_ptr(),
            fit_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create fit-to-canvas scale code={ret}")
            );
        }
        let ret = ff_sys::avfilter_link(acc, 0, fit_ctx, 0);
        if ret < 0 {
            bail!(graph, "link failed: composite→canvas_fit");
        }

        let pad_filter = ff_sys::avfilter_get_by_name(c"pad".as_ptr());
        if pad_filter.is_null() {
            bail!(graph, "filter not found: pad (fit-to-canvas)");
        }
        let Ok(pad_args) = CString::new(format!(
            "{canvas_w}:{canvas_h}:(ow-iw)/2:(oh-ih)/2:color=black"
        )) else {
            bail!(graph, "CString::new failed for fit-to-canvas pad args");
        };
        let mut pad_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut pad_ctx,
            pad_filter,
            c"canvas_pad".as_ptr(),
            pad_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create fit-to-canvas pad code={ret}")
            );
        }
        let ret = ff_sys::avfilter_link(fit_ctx, 0, pad_ctx, 0);
        if ret < 0 {
            bail!(graph, "link failed: canvas_fit→canvas_pad");
        }
        acc = pad_ctx;
    }

    // ── format=rgba: constrain output for direct read-back ────────────────────
    let vfmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
    if vfmt_filter.is_null() {
        bail!(graph, "filter not found: format");
    }
    let mut vfmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vfmt_ctx,
        vfmt_filter,
        c"rtformat".as_ptr(),
        c"pix_fmts=rgba".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create format filter code={ret}"));
    }
    let ret = ff_sys::avfilter_link(acc, 0, vfmt_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: last→format");
    }

    // ── Video buffersink ──────────────────────────────────────────────────────
    let sink_filter = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if sink_filter.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filter,
        c"vsink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create buffersink code={ret}"));
    }
    let ret = ff_sys::avfilter_link(vfmt_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: format→buffersink");
    }

    // ── Configure graph ───────────────────────────────────────────────────────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    let graph_nn = NonNull::new_unchecked(graph);
    let sink_nn = NonNull::new_unchecked(sink_ctx);
    let inner = FilterGraphInner::with_prebuilt_video_inputs(graph_nn, src_ctxs, sink_nn);
    log::info!("realtime composition graph built layers={}", layers.len());
    Ok(FilterGraph::from_prebuilt(inner))
}

// ── Audio mix graph builder ───────────────────────────────────────────────────

pub(super) unsafe fn build_audio_mix(
    sample_rate: u32,
    channel_layout: ChannelLayout,
    tracks: &[AudioTrack],
) -> Result<FilterGraph, FilterError> {
    use std::ffi::CString;

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::CompositionFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::CompositionFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    let track_count = tracks.len();
    let mut end_ctxs: Vec<*mut ff_sys::AVFilterContext> = Vec::with_capacity(track_count);
    let mut animations: Vec<AnimationEntry> = Vec::new();

    for (idx, track) in tracks.iter().enumerate() {
        let path = track
            .source
            .to_string_lossy()
            .replace('\\', "/")
            .replace(':', "\\:");

        // ── amovie= source ────────────────────────────────────────────────────
        let amovie_filter = ff_sys::avfilter_get_by_name(c"amovie".as_ptr());
        if amovie_filter.is_null() {
            bail!(graph, "filter not found: amovie");
        }
        let Ok(amovie_name) = CString::new(format!("amovie{idx}")) else {
            bail!(graph, "CString::new failed for amovie name");
        };
        let Ok(amovie_args) = CString::new(format!("filename={path}")) else {
            bail!(graph, "CString::new failed for amovie args");
        };
        let mut amovie_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut amovie_ctx,
            amovie_filter,
            amovie_name.as_ptr(),
            amovie_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create amovie filter track={idx} code={ret}")
            );
        }
        log::debug!("audio mix track={idx} amovie source path={path}");
        let mut chain_end = amovie_ctx;

        // ── Optional atrim + asetpts (source in/out point trim) ───────────────
        // Inserted immediately after amovie so that subsequent filters
        // (aresample, aformat, adelay, volume, effects) see only the desired
        // source window.  asetpts resets timestamps to zero after the trim so
        // that adelay positions the trimmed content correctly on the timeline.
        if track.in_point.is_some() || track.out_point.is_some() {
            let atrim_args_str = match (track.in_point, track.out_point) {
                (Some(ip), Some(op)) => {
                    format!("start={:.6}:end={:.6}", ip.as_secs_f64(), op.as_secs_f64())
                }
                (Some(ip), None) => format!("start={:.6}", ip.as_secs_f64()),
                (None, Some(op)) => format!("end={:.6}", op.as_secs_f64()),
                (None, None) => unreachable!(),
            };

            let atrim_filter = ff_sys::avfilter_get_by_name(c"atrim".as_ptr());
            if atrim_filter.is_null() {
                bail!(graph, "filter not found: atrim");
            }
            let Ok(atrim_name) = CString::new(format!("atrim{idx}")) else {
                bail!(graph, "CString::new failed for atrim name");
            };
            let Ok(atrim_args) = CString::new(atrim_args_str.as_str()) else {
                bail!(graph, "CString::new failed for atrim args");
            };
            let mut atrim_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut atrim_ctx,
                atrim_filter,
                atrim_name.as_ptr(),
                atrim_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create atrim filter track={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, atrim_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: amovie→atrim track={idx}"));
            }
            chain_end = atrim_ctx;

            let asetpts_filter = ff_sys::avfilter_get_by_name(c"asetpts".as_ptr());
            if asetpts_filter.is_null() {
                bail!(graph, "filter not found: asetpts");
            }
            let Ok(asp_name) = CString::new(format!("atrim_setpts{idx}")) else {
                bail!(graph, "CString::new failed for asetpts name");
            };
            let Ok(asp_args) = CString::new("PTS-STARTPTS") else {
                bail!(graph, "CString::new failed for asetpts args");
            };
            let mut asp_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut asp_ctx,
                asetpts_filter,
                asp_name.as_ptr(),
                asp_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create asetpts filter track={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, asp_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: atrim→asetpts track={idx}"));
            }
            chain_end = asp_ctx;
            log::debug!("audio track trimmed track={idx} args={atrim_args_str}");
        }

        // ── Optional aresample (sample rate conversion) ───────────────────────
        if track.sample_rate != sample_rate {
            let src_rate = track.sample_rate;
            let aresample_filter = ff_sys::avfilter_get_by_name(c"aresample".as_ptr());
            if aresample_filter.is_null() {
                bail!(graph, "filter not found: aresample");
            }
            let Ok(ar_name) = CString::new(format!("aresample{idx}")) else {
                bail!(graph, "CString::new failed for aresample name");
            };
            let Ok(ar_args) = CString::new(format!("{sample_rate}")) else {
                bail!(graph, "CString::new failed for aresample args");
            };
            let mut ar_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut ar_ctx,
                aresample_filter,
                ar_name.as_ptr(),
                ar_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create aresample filter track={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, ar_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: amovie→aresample track={idx}"));
            }
            chain_end = ar_ctx;
            log::info!(
                "audio track resampled track={idx} source_rate={src_rate} target_rate={sample_rate}"
            );
        }

        // ── Optional aformat (channel layout conversion) ──────────────────────
        if track.channel_layout != channel_layout {
            let af_args_str = match channel_layout {
                ChannelLayout::Other(_) => format!("sample_rates={sample_rate}"),
                _ => format!("channel_layouts={}", channel_layout.name()),
            };
            let aformat_filter = ff_sys::avfilter_get_by_name(c"aformat".as_ptr());
            if aformat_filter.is_null() {
                bail!(graph, "filter not found: aformat");
            }
            let Ok(af_name) = CString::new(format!("aformat_layout{idx}")) else {
                bail!(graph, "CString::new failed for aformat layout name");
            };
            let Ok(af_args) = CString::new(af_args_str.as_str()) else {
                bail!(graph, "CString::new failed for aformat layout args");
            };
            let mut af_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut af_ctx,
                aformat_filter,
                af_name.as_ptr(),
                af_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create aformat layout filter track={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, af_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →aformat_layout track={idx}"));
            }
            chain_end = af_ctx;
            log::debug!(
                "audio track reformatted track={idx} layout={}",
                channel_layout.name()
            );
        }

        // ── Optional timeline offset ──────────────────────────────────────────
        // adelay inserts real silence samples, which is required for correct
        // multi-track mixing via amix (unlike asetpts, which only shifts PTS).
        if track.time_offset > Duration::ZERO {
            let delay_ms = track.time_offset.as_millis();
            let adelay_filter = ff_sys::avfilter_get_by_name(c"adelay".as_ptr());
            if adelay_filter.is_null() {
                bail!(graph, "filter not found: adelay");
            }
            let Ok(ad_name) = CString::new(format!("adelay{idx}")) else {
                bail!(graph, "CString::new failed for adelay name");
            };
            // all=1 applies the same delay to every channel
            let Ok(ad_args) = CString::new(format!("{delay_ms}:all=1")) else {
                bail!(graph, "CString::new failed for adelay args");
            };
            let mut ad_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut ad_ctx,
                adelay_filter,
                ad_name.as_ptr(),
                ad_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create adelay filter track={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, ad_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →adelay track={idx}"));
            }
            chain_end = ad_ctx;
            log::debug!("audio track delayed track={idx} delay_ms={delay_ms}");
        }

        // ── Volume (always inserted so the node can be targeted by send_command) ─
        {
            // Warn if animated pan was requested — not yet implemented.
            if matches!(track.pan, AnimatedValue::Track(_)) {
                log::warn!("animated pan not supported; using initial value track_index={idx}");
            }

            let vol_db = track.volume.value_at(Duration::ZERO);
            let node_name = format!("audio_{idx}_volume");
            let vol_filter = ff_sys::avfilter_get_by_name(c"volume".as_ptr());
            if vol_filter.is_null() {
                bail!(graph, "filter not found: volume");
            }
            let Ok(vol_name) = CString::new(node_name.as_str()) else {
                bail!(graph, "CString::new failed for volume name");
            };
            let Ok(vol_args) = CString::new(format!("{vol_db}dB")) else {
                bail!(graph, "CString::new failed for volume args");
            };
            let mut vol_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut vol_ctx,
                vol_filter,
                vol_name.as_ptr(),
                vol_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create volume filter track={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, vol_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →volume track={idx}"));
            }
            chain_end = vol_ctx;

            // Register animation entry if the volume is time-varying.
            // The `volume` filter option is a string expression: append "dB" so
            // that `apply_animations` sends e.g. "-60.000000dB" not "-60.000000".
            if let AnimatedValue::Track(ref vol_track) = track.volume {
                animations.push(AnimationEntry {
                    node_name: node_name.clone(),
                    param: "volume",
                    track: vol_track.clone(),
                    suffix: "dB",
                });
            }
        }

        // ── Per-track effects chain ───────────────────────────────────────────
        for (eff_idx, step) in track.effects.iter().enumerate() {
            let combined_idx = idx * 1000 + eff_idx;
            // FilterStep::Speed uses `setpts` for video but needs audio-specific
            // handling here.  For factor in [0.5, 2.0] use atempo (pitch-preserving
            // WSOLA).  For factor > 2.0 the FFmpeg atempo ring buffer silently
            // overflows (the internal assert skips the check when tempo > 2.0),
            // producing NaN samples.  Fall back to asetrate+aresample (vinyl effect:
            // pitch shifts proportionally) which is free of WSOLA arithmetic.
            let result = if let crate::FilterStep::Speed { factor } = step {
                // Always use asetrate+aresample for any non-unity speed.
                // atempo (WSOLA) produces NaN when both the current fragment and the
                // ring buffer are silent (cross-correlation denominator → 0/0).
                // asetrate+aresample avoids all WSOLA arithmetic; for small ratios
                // (e.g. 1.5x) the SWR FIR is only ~48 taps and never produces NaN.
                // amovie outputs at the file's native rate (e.g. 44100 Hz) while
                // `sample_rate` is the target mix rate (e.g. 48000 Hz).  The optional
                // aresample earlier in the chain fires only when track.sample_rate !=
                // sample_rate, but track.sample_rate is set to the target rate in the
                // pipeline, so the audio may still be at the file's native rate here.
                // Normalise to sample_rate before the speed chain so that
                //   new_sr = sample_rate * factor
                // is correct regardless of the source file's sample rate.
                let norm_filter = ff_sys::avfilter_get_by_name(c"aresample".as_ptr());
                if norm_filter.is_null() {
                    bail!(graph, "filter not found: aresample (speed pre-norm)");
                }
                let Ok(norm_name) = CString::new(format!("spd_norm_sr{combined_idx}")) else {
                    bail!(graph, "CString::new failed for spd_norm_sr name");
                };
                let Ok(norm_args) = CString::new(format!("{sample_rate}")) else {
                    bail!(graph, "CString::new failed for spd_norm_sr args");
                };
                let mut norm_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
                let ret = ff_sys::avfilter_graph_create_filter(
                    &raw mut norm_ctx,
                    norm_filter,
                    norm_name.as_ptr(),
                    norm_args.as_ptr(),
                    std::ptr::null_mut(),
                    graph,
                );
                if ret < 0 {
                    bail!(
                        graph,
                        format!(
                            "failed to create aresample for speed pre-norm \
                             track={idx} code={ret}"
                        )
                    );
                }
                let ret = ff_sys::avfilter_link(chain_end, 0, norm_ctx, 0);
                if ret < 0 {
                    bail!(graph, format!("link failed: →spd_norm_sr track={idx}"));
                }
                chain_end = norm_ctx;

                // Now chain_end is at sample_rate; new_sr = sample_rate * factor
                // gives the correct asetrate value for the desired speedup.
                let new_sr = (f64::from(sample_rate) * factor).round() as u32;
                crate::filter_inner::add_asetrate_resample_chain(
                    graph,
                    chain_end,
                    new_sr,
                    sample_rate,
                    channel_layout.channels(),
                    combined_idx,
                )
            } else {
                crate::filter_inner::add_and_link_step(graph, chain_end, step, combined_idx, "eff")
            };
            if let Ok(ctx) = result {
                chain_end = ctx;
            } else {
                bail!(
                    graph,
                    format!(
                        "failed to apply effect track={idx} effect={eff_idx} filter={}",
                        step.filter_name()
                    )
                );
            }
        }

        end_ctxs.push(chain_end);
    }

    // ── amix ──────────────────────────────────────────────────────────────────
    let amix_filter = ff_sys::avfilter_get_by_name(c"amix".as_ptr());
    if amix_filter.is_null() {
        bail!(graph, "filter not found: amix");
    }
    let Ok(amix_args) = CString::new(format!("inputs={track_count}:normalize=0")) else {
        bail!(graph, "CString::new failed for amix args");
    };
    let mut amix_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut amix_ctx,
        amix_filter,
        c"amix".as_ptr(),
        amix_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create amix filter code={ret}"));
    }
    for (i, &end_ctx) in end_ctxs.iter().enumerate() {
        let ret = ff_sys::avfilter_link(end_ctx, 0, amix_ctx, i as u32);
        if ret < 0 {
            bail!(graph, format!("link failed: track{i}→amix[{i}]"));
        }
    }

    // ── aformat ───────────────────────────────────────────────────────────────
    let aformat_args_str = match channel_layout {
        ChannelLayout::Other(_) => format!("sample_rates={sample_rate}"),
        _ => format!(
            "sample_rates={sample_rate}:channel_layouts={}",
            channel_layout.name()
        ),
    };
    let aformat_filter = ff_sys::avfilter_get_by_name(c"aformat".as_ptr());
    if aformat_filter.is_null() {
        bail!(graph, "filter not found: aformat");
    }
    let Ok(aformat_args) = CString::new(aformat_args_str.as_str()) else {
        bail!(graph, "CString::new failed for aformat args");
    };
    let mut aformat_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut aformat_ctx,
        aformat_filter,
        c"aformat".as_ptr(),
        aformat_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create aformat filter code={ret}"));
    }
    let ret = ff_sys::avfilter_link(amix_ctx, 0, aformat_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: amix→aformat");
    }

    // ── abuffersink ───────────────────────────────────────────────────────────
    let sink_filter = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if sink_filter.is_null() {
        bail!(graph, "filter not found: abuffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filter,
        c"asink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create abuffersink code={ret}"));
    }
    let ret = ff_sys::avfilter_link(aformat_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: aformat→abuffersink");
    }

    // ── Configure graph ───────────────────────────────────────────────────────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        log::warn!("audio mix avfilter_graph_config failed code={ret}");
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // SAFETY: ret >= 0 guarantees both pointers are non-null.
    let graph_nn = NonNull::new_unchecked(graph);
    let sink_nn = NonNull::new_unchecked(sink_ctx);
    let inner = FilterGraphInner::with_prebuilt_audio_graph(graph_nn, sink_nn);
    log::info!("audio mix graph built tracks={track_count} sample_rate={sample_rate}");
    Ok(FilterGraph::from_prebuilt_animated(inner, animations))
}

// ── Video concat graph builder ────────────────────────────────────────────────

pub(super) unsafe fn build_video_concat(
    clips: &[PathBuf],
    output_width: Option<u32>,
    output_height: Option<u32>,
) -> Result<FilterGraph, FilterError> {
    use std::ffi::CString;

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::CompositionFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::CompositionFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    let clip_count = clips.len();
    let mut end_ctxs: Vec<*mut ff_sys::AVFilterContext> = Vec::with_capacity(clip_count);

    for (idx, clip) in clips.iter().enumerate() {
        let path = clip
            .to_string_lossy()
            .replace('\\', "/")
            .replace(':', "\\:");

        // ── movie= source ─────────────────────────────────────────────────────
        let movie_filter = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
        if movie_filter.is_null() {
            bail!(graph, "filter not found: movie");
        }
        let Ok(movie_name) = CString::new(format!("concat_movie{idx}")) else {
            bail!(graph, "CString::new failed for movie name");
        };
        let Ok(movie_args) = CString::new(format!("filename={path}")) else {
            bail!(graph, "CString::new failed for movie args");
        };
        let mut movie_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut movie_ctx,
            movie_filter,
            movie_name.as_ptr(),
            movie_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create movie filter clip={idx} code={ret}")
            );
        }
        log::debug!("video concat clip={idx} movie source path={path}");
        let mut chain_end = movie_ctx;

        // ── Optional scale ────────────────────────────────────────────────────
        if let (Some(w), Some(h)) = (output_width, output_height) {
            let scale_filter = ff_sys::avfilter_get_by_name(c"scale".as_ptr());
            if scale_filter.is_null() {
                bail!(graph, "filter not found: scale");
            }
            let Ok(sc_name) = CString::new(format!("concat_scale{idx}")) else {
                bail!(graph, "CString::new failed for scale name");
            };
            let Ok(sc_args) = CString::new(format!("{w}:{h}")) else {
                bail!(graph, "CString::new failed for scale args");
            };
            let mut sc_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut sc_ctx,
                scale_filter,
                sc_name.as_ptr(),
                sc_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create scale filter clip={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, sc_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: movie→scale clip={idx}"));
            }
            chain_end = sc_ctx;
        }

        end_ctxs.push(chain_end);
    }

    // ── concat (skipped for single clip) ─────────────────────────────────────
    let pre_sink_ctx = if clip_count == 1 {
        end_ctxs[0]
    } else {
        let concat_filter = ff_sys::avfilter_get_by_name(c"concat".as_ptr());
        if concat_filter.is_null() {
            bail!(graph, "filter not found: concat");
        }
        let Ok(concat_args) = CString::new(format!("n={clip_count}:v=1:a=0")) else {
            bail!(graph, "CString::new failed for concat args");
        };
        let mut concat_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut concat_ctx,
            concat_filter,
            c"concat".as_ptr(),
            concat_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(graph, format!("failed to create concat filter code={ret}"));
        }
        for (i, &end_ctx) in end_ctxs.iter().enumerate() {
            let ret = ff_sys::avfilter_link(end_ctx, 0, concat_ctx, i as u32);
            if ret < 0 {
                bail!(graph, format!("link failed: clip{i}→concat[{i}]"));
            }
        }
        concat_ctx
    };

    // ── format=yuv420p: constrain output pixel format before sink ────────────
    let vfmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
    if vfmt_filter.is_null() {
        bail!(graph, "filter not found: format");
    }
    let mut vfmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vfmt_ctx,
        vfmt_filter,
        c"vformat".as_ptr(),
        c"pix_fmts=yuv420p".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create format filter code={ret}"));
    }
    let ret = ff_sys::avfilter_link(pre_sink_ctx, 0, vfmt_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: last→format");
    }
    log::debug!("video concat format filter inserted output=yuv420p");

    // ── buffersink ────────────────────────────────────────────────────────────
    let sink_filter = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if sink_filter.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filter,
        c"vsink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create buffersink code={ret}"));
    }
    let ret = ff_sys::avfilter_link(vfmt_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: format→buffersink");
    }

    // ── Configure graph ───────────────────────────────────────────────────────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        log::warn!("video concat avfilter_graph_config failed code={ret}");
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // SAFETY: ret >= 0 guarantees both pointers are non-null.
    let graph_nn = NonNull::new_unchecked(graph);
    let sink_nn = NonNull::new_unchecked(sink_ctx);
    let inner = FilterGraphInner::with_prebuilt_video_graph(graph_nn, sink_nn);
    log::info!("video concat graph built clips={clip_count}");
    Ok(FilterGraph::from_prebuilt(inner))
}

// ── Audio concat graph builder ────────────────────────────────────────────────

pub(super) unsafe fn build_audio_concat(
    clips: &[PathBuf],
    output_sample_rate: Option<u32>,
    output_channel_layout: Option<ChannelLayout>,
) -> Result<FilterGraph, FilterError> {
    use std::ffi::CString;

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::CompositionFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::CompositionFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    let clip_count = clips.len();
    let mut end_ctxs: Vec<*mut ff_sys::AVFilterContext> = Vec::with_capacity(clip_count);

    for (idx, clip) in clips.iter().enumerate() {
        let path = clip
            .to_string_lossy()
            .replace('\\', "/")
            .replace(':', "\\:");

        // ── amovie= source ────────────────────────────────────────────────────
        let amovie_filter = ff_sys::avfilter_get_by_name(c"amovie".as_ptr());
        if amovie_filter.is_null() {
            bail!(graph, "filter not found: amovie");
        }
        let Ok(amovie_name) = CString::new(format!("aconcat_amovie{idx}")) else {
            bail!(graph, "CString::new failed for amovie name");
        };
        let Ok(amovie_args) = CString::new(format!("filename={path}")) else {
            bail!(graph, "CString::new failed for amovie args");
        };
        let mut amovie_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut amovie_ctx,
            amovie_filter,
            amovie_name.as_ptr(),
            amovie_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(
                graph,
                format!("failed to create amovie filter clip={idx} code={ret}")
            );
        }
        log::debug!("audio concat clip={idx} amovie source path={path}");
        let mut chain_end = amovie_ctx;

        // ── Optional aresample (sample rate conversion) ───────────────────────
        if let Some(sample_rate) = output_sample_rate {
            let aresample_filter = ff_sys::avfilter_get_by_name(c"aresample".as_ptr());
            if aresample_filter.is_null() {
                bail!(graph, "filter not found: aresample");
            }
            let Ok(ar_name) = CString::new(format!("aconcat_aresample{idx}")) else {
                bail!(graph, "CString::new failed for aresample name");
            };
            let Ok(ar_args) = CString::new(format!("{sample_rate}")) else {
                bail!(graph, "CString::new failed for aresample args");
            };
            let mut ar_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut ar_ctx,
                aresample_filter,
                ar_name.as_ptr(),
                ar_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create aresample filter clip={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, ar_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: amovie→aresample clip={idx}"));
            }
            chain_end = ar_ctx;
            log::debug!("audio concat clip={idx} aresample target_rate={sample_rate}");
        }

        // ── Optional aformat (channel layout conversion) ──────────────────────
        if let Some(layout) = output_channel_layout {
            let af_args_str = match layout {
                ChannelLayout::Other(_) => {
                    let sr = output_sample_rate.unwrap_or(48_000);
                    format!("sample_rates={sr}")
                }
                _ => format!("channel_layouts={}", layout.name()),
            };
            let aformat_filter = ff_sys::avfilter_get_by_name(c"aformat".as_ptr());
            if aformat_filter.is_null() {
                bail!(graph, "filter not found: aformat");
            }
            let Ok(af_name) = CString::new(format!("aconcat_aformat{idx}")) else {
                bail!(graph, "CString::new failed for aformat name");
            };
            let Ok(af_args) = CString::new(af_args_str.as_str()) else {
                bail!(graph, "CString::new failed for aformat args");
            };
            let mut af_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut af_ctx,
                aformat_filter,
                af_name.as_ptr(),
                af_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(
                    graph,
                    format!("failed to create aformat filter clip={idx} code={ret}")
                );
            }
            let ret = ff_sys::avfilter_link(chain_end, 0, af_ctx, 0);
            if ret < 0 {
                bail!(graph, format!("link failed: →aformat clip={idx}"));
            }
            chain_end = af_ctx;
            log::debug!("audio concat clip={idx} aformat layout={}", layout.name());
        }

        end_ctxs.push(chain_end);
    }

    // ── concat (skipped for single clip) ─────────────────────────────────────
    let pre_sink_ctx = if clip_count == 1 {
        end_ctxs[0]
    } else {
        let concat_filter = ff_sys::avfilter_get_by_name(c"concat".as_ptr());
        if concat_filter.is_null() {
            bail!(graph, "filter not found: concat");
        }
        let Ok(concat_args) = CString::new(format!("n={clip_count}:v=0:a=1")) else {
            bail!(graph, "CString::new failed for concat args");
        };
        let mut concat_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut concat_ctx,
            concat_filter,
            c"aconcat".as_ptr(),
            concat_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(graph, format!("failed to create concat filter code={ret}"));
        }
        for (i, &end_ctx) in end_ctxs.iter().enumerate() {
            let ret = ff_sys::avfilter_link(end_ctx, 0, concat_ctx, i as u32);
            if ret < 0 {
                bail!(graph, format!("link failed: clip{i}→aconcat[{i}]"));
            }
        }
        concat_ctx
    };

    // ── abuffersink ───────────────────────────────────────────────────────────
    let sink_filter = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if sink_filter.is_null() {
        bail!(graph, "filter not found: abuffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filter,
        c"aconcat_asink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create abuffersink code={ret}"));
    }
    let ret = ff_sys::avfilter_link(pre_sink_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: last→abuffersink");
    }

    // ── Configure graph ───────────────────────────────────────────────────────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        log::warn!("audio concat avfilter_graph_config failed code={ret}");
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // SAFETY: ret >= 0 guarantees both pointers are non-null.
    let graph_nn = NonNull::new_unchecked(graph);
    let sink_nn = NonNull::new_unchecked(sink_ctx);
    let inner = FilterGraphInner::with_prebuilt_audio_graph(graph_nn, sink_nn);
    log::info!("audio concat graph built clips={clip_count}");
    Ok(FilterGraph::from_prebuilt(inner))
}

// ── Dissolve-join graph builder ────────────────────────────────────────────────

/// Probe a clip's duration in seconds using `avformat_open_input` +
/// `avformat_find_stream_info`.  Returns `CompositionFailed` if the file
/// cannot be opened or has an unknown duration.
pub(super) unsafe fn probe_clip_duration_sec(path: &PathBuf) -> Result<f64, FilterError> {
    let ctx = ff_sys::avformat::open_input(path.as_ref()).map_err(|code| {
        FilterError::CompositionFailed {
            reason: format!(
                "failed to probe clip duration code={code} path={}",
                path.display()
            ),
        }
    })?;

    if let Err(code) = ff_sys::avformat::find_stream_info(ctx) {
        let mut p = ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(FilterError::CompositionFailed {
            reason: format!("avformat_find_stream_info failed code={code}"),
        });
    }

    let duration_val = (*ctx).duration;
    let mut p = ctx;
    ff_sys::avformat::close_input(&raw mut p);

    if duration_val <= 0 {
        return Err(FilterError::CompositionFailed {
            reason: format!("clip has unknown duration path={}", path.display()),
        });
    }
    // AV_TIME_BASE = 1_000_000 microseconds per second.
    Ok(duration_val as f64 / 1_000_000.0)
}

pub(super) unsafe fn build_dissolve_join(
    clip_a: &PathBuf,
    clip_b: &PathBuf,
    dissolve_sec: f64,
) -> Result<FilterGraph, FilterError> {
    use std::ffi::CString;

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::CompositionFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    // ── Dissolve-duration == 0: use concat (plain concatenation) ──────────────
    if dissolve_sec == 0.0 {
        return build_video_concat(&[clip_a.clone(), clip_b.clone()], None, None);
    }

    // ── Probe clip durations ──────────────────────────────────────────────────
    let clip_a_dur = probe_clip_duration_sec(clip_a)?;
    let clip_b_dur = probe_clip_duration_sec(clip_b)?;

    if dissolve_sec > clip_a_dur {
        return Err(FilterError::CompositionFailed {
            reason: format!(
                "dissolve_duration ({dissolve_sec:.3}s) exceeds clip_a duration ({clip_a_dur:.3}s)"
            ),
        });
    }
    if dissolve_sec > clip_b_dur {
        return Err(FilterError::CompositionFailed {
            reason: format!(
                "dissolve_duration ({dissolve_sec:.3}s) exceeds clip_b duration ({clip_b_dur:.3}s)"
            ),
        });
    }

    // xfade offset = when the crossfade starts (measured from the beginning of
    // the first stream).
    let xfade_offset = clip_a_dur - dissolve_sec;

    // ── Allocate graph ────────────────────────────────────────────────────────
    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::CompositionFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // ── movie[a] source ───────────────────────────────────────────────────────
    let movie_filter = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filter.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let path_a = clip_a.to_string_lossy();
    let Ok(movie_a_name) = CString::new("jd_movie_a") else {
        bail!(graph, "CString::new failed for movie_a name");
    };
    let Ok(movie_a_args) = CString::new(format!("filename={path_a}")) else {
        bail!(graph, "CString::new failed for movie_a args");
    };
    let mut movie_a_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut movie_a_ctx,
        movie_filter,
        movie_a_name.as_ptr(),
        movie_a_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create movie_a filter code={ret}"));
    }
    log::debug!("dissolve join clip_a movie source path={path_a}");

    // ── movie[b] source ───────────────────────────────────────────────────────
    let path_b = clip_b.to_string_lossy();
    let Ok(movie_b_name) = CString::new("jd_movie_b") else {
        bail!(graph, "CString::new failed for movie_b name");
    };
    let Ok(movie_b_args) = CString::new(format!("filename={path_b}")) else {
        bail!(graph, "CString::new failed for movie_b args");
    };
    let mut movie_b_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut movie_b_ctx,
        movie_filter,
        movie_b_name.as_ptr(),
        movie_b_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create movie_b filter code={ret}"));
    }
    log::debug!("dissolve join clip_b movie source path={path_b}");

    // ── xfade ─────────────────────────────────────────────────────────────────
    let xfade_filter = ff_sys::avfilter_get_by_name(c"xfade".as_ptr());
    if xfade_filter.is_null() {
        bail!(graph, "filter not found: xfade");
    }
    let xfade_args_str =
        format!("transition=dissolve:duration={dissolve_sec}:offset={xfade_offset}");
    let Ok(xfade_args) = CString::new(xfade_args_str.as_str()) else {
        bail!(graph, "CString::new failed for xfade args");
    };
    let mut xfade_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut xfade_ctx,
        xfade_filter,
        c"jd_xfade".as_ptr(),
        xfade_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("failed to create xfade filter args={xfade_args_str} code={ret}")
        );
    }
    log::debug!("dissolve join xfade args={xfade_args_str}");

    // movie_a → xfade[0]
    let ret = ff_sys::avfilter_link(movie_a_ctx, 0, xfade_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: movie_a→xfade[0]");
    }
    // movie_b → xfade[1]
    let ret = ff_sys::avfilter_link(movie_b_ctx, 0, xfade_ctx, 1);
    if ret < 0 {
        bail!(graph, "link failed: movie_b→xfade[1]");
    }

    // ── format=yuv420p: constrain output pixel format before sink ────────────
    let vfmt_filter = ff_sys::avfilter_get_by_name(c"format".as_ptr());
    if vfmt_filter.is_null() {
        bail!(graph, "filter not found: format");
    }
    let mut vfmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vfmt_ctx,
        vfmt_filter,
        c"jd_vformat".as_ptr(),
        c"pix_fmts=yuv420p".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create format filter code={ret}"));
    }
    let ret = ff_sys::avfilter_link(xfade_ctx, 0, vfmt_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: xfade→format");
    }
    log::debug!("dissolve join format filter inserted output=yuv420p");

    // ── buffersink ────────────────────────────────────────────────────────────
    let sink_filter = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if sink_filter.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filter,
        c"jd_vsink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("failed to create buffersink code={ret}"));
    }
    let ret = ff_sys::avfilter_link(vfmt_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, "link failed: format→buffersink");
    }

    // ── Configure graph ───────────────────────────────────────────────────────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        log::warn!("dissolve join avfilter_graph_config failed code={ret}");
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // SAFETY: ret >= 0 guarantees both pointers are non-null.
    let graph_nn = NonNull::new_unchecked(graph);
    let sink_nn = NonNull::new_unchecked(sink_ctx);
    let inner = FilterGraphInner::with_prebuilt_video_graph(graph_nn, sink_nn);
    log::info!("dissolve join graph built dissolve_sec={dissolve_sec} xfade_offset={xfade_offset}");
    Ok(FilterGraph::from_prebuilt(inner))
}
