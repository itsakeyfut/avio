# FFmpeg Token Mapping

Per-enum mapping of avio enum variants to FFmpeg tokens. `status` is `OK`/`NG`; for `NG`, `expected`
holds the correct token, or `want to remove` for variants with no FFmpeg equivalent. Each table's
`Reference` links the actual C source (FFmpeg `release/7.1` and `release/8.0`).

## ScaleAlgorithm

**Reference:** swscale `sws_flags` unit (`libswscale/options.c`) — consumed by the `scale` filter `flags=` via `ScaleAlgorithm::as_flags_str()` — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libswscale/options.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libswscale/options.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Fast | fast_bilinear | OK | - |
| Bilinear | bilinear | OK | - |
| Bicubic | bicubic | OK | - |
| Lanczos | lanczos | OK | - |

> Full swscale scaler set (11): `fast_bilinear, bilinear, bicubic, experimental, neighbor, area, bicublin, gauss, sinc, lanczos, spline` — avio covers 4. The Scale step emits `ScaleAlgorithm::as_flags_str()` (identical values to the `FfmpegToken` impl — a duplicate). The `libplacebo` filter (`vf_libplacebo`, `upscaler`/`downscaler`: `spline36`/`mitchell`/`ewa_lanczos`/…) is a **separate** kernel set and is **not** avio's scaler.

## ToneMap

**Reference:** `tonemap` filter `tonemap=` option (`tonemap` unit) — emitted via `ToneMap::as_str()` — `libavfilter/vf_tonemap.c` — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_tonemap.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_tonemap.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Hable | hable | OK | - |
| Reinhard | reinhard | OK | - |
| Mobius | mobius | OK | - |

> Full `tonemap` set (7): `none, linear, gamma, clip, reinhard, hable, mobius` — avio covers 3. `as_str()` duplicates the `FfmpegToken` impl.

## YadifMode

**Reference:** `yadif` filter `mode=` option (`AV_OPT_TYPE_INT`, **range 0–3**) — emitted **numerically** via `*mode as i32` — `libavfilter/yadif_common.c` — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/yadif_common.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/yadif_common.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Frame | 0 | OK | - |
| Field | 1 | OK | - |
| FrameNospatial | 2 | OK | - |
| FieldNospatial | 3 | OK | - |

> avio emits `mode=0`…`mode=3` (numeric), valid because `mode` is an INT in range 0–3 (`{.i64=YADIF_MODE_SEND_FRAME}, 0, 3`). Named aliases `send_frame`/`send_field`/`send_frame_nospatial`/`send_field_nospatial` = 0/1/2/3. **Full coverage (4/4).** The `FfmpegToken` impl (`"0"`…`"3"`) duplicates the cast.

## XfadeTransition

**Reference:** `xfade` filter `transition=` option (`transition` unit) — emitted via `XfadeTransition::as_str()` — `libavfilter/vf_xfade.c` — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_xfade.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_xfade.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Dissolve | dissolve | OK | - |
| Fade | fade | OK | - |
| WipeLeft | wipeleft | OK | - |
| WipeRight | wiperight | OK | - |
| WipeUp | wipeup | OK | - |
| WipeDown | wipedown | OK | - |
| SlideLeft | slideleft | OK | - |
| SlideRight | slideright | OK | - |
| SlideUp | slideup | OK | - |
| SlideDown | slidedown | OK | - |
| CircleOpen | circleopen | OK | - |
| CircleClose | circleclose | OK | - |
| FadeGrays | fadegrays | OK | - |
| Pixelize | pixelize | OK | - |

> Full `xfade` transition set: **59** — avio covers 14 (enum is `#[non_exhaustive]`). `as_str()` duplicates the `FfmpegToken` impl.

## BlendMode

**Reference:** `libavfilter/vf_blend.c` (`all_mode` / `mode` unit) — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_blend.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_blend.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Normal | normal | OK | - |
| Multiply | multiply | OK | - |
| Screen | screen | OK | - |
| Overlay | overlay | OK | - |
| SoftLight | softlight | OK | - |
| HardLight | hardlight | OK | - |
| ColorDodge | dodge | OK | - |
| ColorBurn | burn | OK | - |
| Darken | darken | OK | - |
| Lighten | lighten | OK | - |
| Difference | difference | OK | - |
| Exclusion | exclusion | OK | - |
| Add | addition | OK | - |
| Subtract | subtract | OK | - |
| Hue | None | NG | want to remove |
| Saturation | None | NG | want to remove |
| Color | None | NG | want to remove |
| Luminosity | None | NG | want to remove |

> `PorterDuffOver/Under/In/Out/Atop/Xor` are separated into `CompositeOp` (below) — #1218.

## CompositeOp

**Reference:** no FFmpeg token — built via `libavfilter/vf_overlay.c` (overlay) + `libavfilter/vf_blend.c` (`c0_expr`). overlay [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_overlay.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_overlay.c) · blend [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_blend.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_blend.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Over | None | OK | - |
| Under | None | OK | - |
| In | None | OK | - |
| Out | None | OK | - |
| Atop | None | OK | - |
| Xor | None | OK | - |

## ColorRange

**Reference:** `libavutil/pixdesc.c` (`color_range_names[]`, via `av_color_range_from_name`) — consumed by the `format` filter `color_ranges=` (`vf_format.c`) — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavutil/pixdesc.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Limited | tv | OK | - |
| Full | pc | OK | - |
| Unknown | None | OK | - |

> The `format` step emits `.name()` (`Limited`→`"limited"`, `Full`→`"full"`), which `av_color_range_from_name` **rejects** — it accepts only `tv`/`pc`/`unknown` (`color_range_names[]`). The correct tokens (`tv`/`pc`) are in `FfmpegToken`, not yet wired (#1212). The `colorspace` filter `range` unit additionally accepts `mpeg`/`jpeg` aliases, but avio's consumer is the `format` filter.

## ColorSpace

**Reference:** `libavutil/pixdesc.c` (`color_space_names[]`, via `av_color_space_from_name`) — consumed by the `format` filter `color_spaces=` (`vf_format.c`) — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavutil/pixdesc.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Bt709 | bt709 | OK | - |
| Bt470bg | bt470bg | OK | - |
| Smpte170m | smpte170m | OK | - |
| Bt2020Ncl | bt2020nc | OK | - |
| Bt2020Cl | bt2020c | OK | - |
| Rgb | gbr | OK | - |
| Fcc | fcc | OK | - |
| Smpte240m | smpte240m | OK | - |
| Ycgco | ycgco | OK | - |
| Unknown | None | OK | - |

> Redesigned in #1217: `Bt601` split into `Bt470bg`/`Smpte170m`, `Bt2020` split into `Bt2020Ncl`/`Bt2020Cl`, `Srgb`→`Rgb` (token `gbr`), `DciP3` moved to ColorPrimaries; added `Fcc`/`Smpte240m`/`Ycgco`. `name()` is the human label; `FfmpegToken` is the canonical token above.

## ColorPrimaries

**Reference:** `libavfilter/vf_setparams.c` (`color_primaries` unit; tokens match `pixdesc.c color_primaries_names[]`) — planned consumer per the #1217 follow-up (the `format` filter has no `color_primaries` option) — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_setparams.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_setparams.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Bt709 | bt709 | OK | - |
| Bt470bg | bt470bg | OK | - |
| Smpte170m | smpte170m | OK | - |
| Smpte240m | smpte240m | OK | - |
| Film | film | OK | - |
| DciP3 | smpte431 | OK | - |
| DisplayP3 | smpte432 | OK | - |
| Bt2020 | bt2020 | OK | - |
| Unknown | None | OK | - |

> Redesigned in #1217: `Bt601` split into `Bt470bg`/`Smpte170m`; added `Smpte240m`/`Film`/`DciP3` (token `smpte431`)/`DisplayP3` (token `smpte432`). `FfmpegToken` impl added; the setparams consumer is the #1217 follow-up.

## ColorTransfer

**Reference:** `libavfilter/vf_setparams.c` (`color_trc` unit; tokens match `pixdesc.c color_transfer_names[]`, incl. `arib-std-b67`/`smpte2084`) — planned consumer per the #1217 follow-up (the `format` filter has no `color_transfer` option) — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_setparams.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_setparams.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Bt709 | bt709 | OK | - |
| Gamma22 | gamma22 | OK | - |
| Gamma28 | gamma28 | OK | - |
| Smpte170m | smpte170m | OK | - |
| Smpte240m | smpte240m | OK | - |
| Linear | linear | OK | - |
| Srgb | iec61966-2-1 | OK | - |
| Bt2020_10 | bt2020-10 | OK | - |
| Bt2020_12 | bt2020-12 | OK | - |
| Pq | smpte2084 | OK | - |
| Hlg | arib-std-b67 | OK | - |
| Unknown | None | OK | - |

> Redesigned in #1217: `Hlg`/`Pq` keep their human names but `FfmpegToken` now emits the canonical `arib-std-b67`/`smpte2084`; added `Gamma22`/`Gamma28`/`Smpte170m`/`Smpte240m`/`Srgb` (token `iec61966-2-1`). The setparams consumer is the #1217 follow-up.

## AlphaMode

**Reference:** the `overlay` filter `alpha=` option (`alpha_format` unit; **single value**: `straight`/`premultiplied`) — `libavfilter/vf_overlay.c` — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/vf_overlay.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_overlay.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Straight | straight | OK | - |
| Premultiplied | premultiplied | OK | - |
| Unknown | None | OK | - |

> Wiring bug: the `format` step emits `format=…:alpha_modes=…`, but the `format` filter has **no** alpha option (7.1/8.0) — AlphaMode must instead drive the `overlay` `alpha=` option (a single value, not a `|`-list). Also note the `format` step uses `.name()`, not `FfmpegToken` (#1212).
>
> Version note: in **7.1/8.0** the `overlay` `alpha` option (`alpha_format` unit) is **`straight`/`premultiplied` only, default `straight`** (`{.i64=0}`, range 0–1). The ffmpeg.org docs' third value **`auto`** (default auto) is **master-only** — alpha was refactored there to the `alpha_mode` unit (`{.i64=AVALPHA_MODE_UNSPECIFIED}`). Not present in avio's 7.1/8.0 targets; if ever targeted, gate `auto` and map it to `AlphaMode::Unknown`.

## PixelFormat

**Reference:** `libavutil/pixdesc.c` (`av_pix_fmt_descriptors[]` `.name`/`.alias`, via `av_get_pix_fmt` which resolves aliases) — consumed by the `format` filter `pix_fmts=` (`vf_format.c`) — [7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavutil/pixdesc.c) · [8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c)

| avio variant | FFmpeg token | status | expected |
|---|---|---|---|
| Rgb24 | rgb24 | OK | - |
| Rgba | rgba | OK | - |
| Bgr24 | bgr24 | OK | - |
| Bgra | bgra | OK | - |
| Yuv420p | yuv420p | OK | - |
| Yuv422p | yuv422p | OK | - |
| Yuv444p | yuv444p | OK | - |
| Nv12 | nv12 | OK | - |
| Nv21 | nv21 | OK | - |
| Yuv420p10le | yuv420p10le | OK | - |
| Yuv422p10le | yuv422p10le | OK | - |
| Yuv444p10le | yuv444p10le | OK | - |
| Yuva444p10le | yuva444p10le | OK | - |
| P010le | p010le | OK | - |
| Gray8 | gray | OK | - |
| Gbrpf32le | gbrpf32le | OK | - |
| Other(u32) | None | OK | - |

> The `format` step emits `.name()`, not `FfmpegToken` (#1212): `Gray8`→`"gray8"` is accepted (a valid `.alias` of `"gray"` resolved by `av_get_pix_fmt`), but `Other(_)`→`"unknown"` is **invalid** as a pix_fmt (should skip / use `av_get_pix_fmt_name(value)`).
