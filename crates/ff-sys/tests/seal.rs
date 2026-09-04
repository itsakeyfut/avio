//! Regression guard for the ff-sys safe-layer seal (#1506 / #1473, ADR-0003).
//!
//! The sealed part of the hand-written safe layer must expose **no** raw pointer
//! in a public signature: every `*const` / `*mut` lives behind an owned RAII type
//! or a `pub(crate)` wrapper, and the only raw FFI a consumer may touch is the
//! bindgen layer (`pub use raw::*`), which this guard does not scan (nor the
//! `docsrs_stubs`, which mix that raw layer with the wrapper stubs).
//!
//! This test reads the sources as text (no FFmpeg needed at runtime) and fails if
//! any `pub` (not `pub(crate)`) `fn` / `unsafe fn` / `const fn` signature names a
//! raw pointer. A signature can opt out with a `seal-allow-raw` marker comment
//! placed in the contiguous comment block directly above it (used for
//! `buffersink_get_frame`'s `AVFilterContext` and `Codec::as_ptr`, deliberately
//! outside the #1506 owned-type seal).
//!
//! Not everything is in scope: the `audio_fifo` (`*mut AVAudioFifo`) and
//! `channel_layout` POD helpers (`*mut AVChannelLayout`) are separate unsealed raw
//! surfaces, excluded below until their own RAII sealing lands.

/// The hand-written safe-layer source files, embedded at compile time.
const SAFE_LAYER: &[(&str, &str)] = &[
    ("frame.rs", include_str!("../src/frame.rs")),
    ("packet.rs", include_str!("../src/packet.rs")),
    ("codec.rs", include_str!("../src/codec.rs")),
    ("codec_context.rs", include_str!("../src/codec_context.rs")),
    (
        "format_context.rs",
        include_str!("../src/format_context.rs"),
    ),
    ("scale_context.rs", include_str!("../src/scale_context.rs")),
    (
        "resample_context.rs",
        include_str!("../src/resample_context.rs"),
    ),
    ("hwdevice.rs", include_str!("../src/hwdevice.rs")),
    ("buffersink.rs", include_str!("../src/buffersink.rs")),
    ("bsf.rs", include_str!("../src/bsf.rs")),
    ("avcodec.rs", include_str!("../src/avcodec.rs")),
    ("avformat.rs", include_str!("../src/avformat.rs")),
    ("swscale.rs", include_str!("../src/swscale.rs")),
    (
        "swresample/mod.rs",
        include_str!("../src/swresample/mod.rs"),
    ),
    (
        "swresample/context.rs",
        include_str!("../src/swresample/context.rs"),
    ),
    (
        "swresample/convert.rs",
        include_str!("../src/swresample/convert.rs"),
    ),
    // NOT scanned — `docsrs_stubs.rs` intentionally mixes the sealed wrapper stubs
    // with the *bindgen-layer* stubs (`av_frame_alloc`, `av_dict_get`, …) that
    // mirror the intentionally-unsealed `pub use raw::*`; a whole-file scan would
    // wrongly flag that raw layer, so it is excluded here.
    //
    // NOT scanned — separate raw surfaces outside the #1506 seal, each operating on
    // a non-owned FFmpeg type (like buffersink's `AVFilterContext`): the
    // `audio_fifo` submodule (`*mut AVAudioFifo`) and the `channel_layout` POD
    // helpers (`*mut AVChannelLayout`, an embedded frame/context field). They gain
    // owned wrappers (and this guard) when their own sealing lands. `sample_format`
    // is pointer-free but omitted with its siblings for a single, clear boundary.
];

/// Returns the public signatures in `src` that name a raw pointer type.
///
/// A signature starts at a line whose trimmed text begins with `pub fn` /
/// `pub unsafe fn` / `pub const fn` / `pub async fn` (crucially NOT `pub(crate)`
/// or `pub(super)`, which don't match) and runs until the line that opens the
/// body (`{`) or ends the declaration (`;`). A signature is skipped when a
/// `seal-allow-raw` marker appears in the contiguous comment/attribute block
/// directly above it.
fn raw_pointer_public_signatures(src: &str) -> Vec<String> {
    const STARTS: [&str; 4] = [
        "pub fn ",
        "pub unsafe fn ",
        "pub const fn ",
        "pub async fn ",
    ];
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if STARTS.iter().any(|s| trimmed.starts_with(s)) {
            // Accumulate the signature until the body `{` or a `;`.
            let mut sig = String::new();
            let mut j = i;
            loop {
                sig.push_str(lines[j]);
                sig.push('\n');
                if lines[j].contains('{') || lines[j].trim_end().ends_with(';') {
                    break;
                }
                j += 1;
                if j >= lines.len() {
                    break;
                }
            }

            // Exemption: a `seal-allow-raw` marker in the contiguous comment /
            // attribute / blank block immediately above the signature.
            let mut exempt = sig.contains("seal-allow-raw");
            let mut k = i;
            while !exempt && k > 0 {
                k -= 1;
                let above = lines[k].trim_start();
                if above.starts_with("//") || above.starts_with("#[") || above.is_empty() {
                    if lines[k].contains("seal-allow-raw") {
                        exempt = true;
                    }
                } else {
                    break;
                }
            }

            if !exempt && (sig.contains("*const") || sig.contains("*mut")) {
                hits.push(sig.trim().to_string());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    hits
}

#[test]
fn seal_guard_should_reject_public_raw_pointer_signatures() {
    let mut offenders = Vec::new();
    for (name, src) in SAFE_LAYER {
        for sig in raw_pointer_public_signatures(src) {
            offenders.push(format!("{name}:\n{sig}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "the ff-sys safe layer must expose no raw pointer in a public signature; \
         demote the offender(s) to `pub(crate)` (or add a `seal-allow-raw` marker if the \
         raw surface is intentionally unsealed):\n\n{}",
        offenders.join("\n\n")
    );
}

#[test]
fn seal_guard_scanner_should_flag_a_synthetic_raw_signature() {
    // The scanner must actually detect a public raw-pointer signature (guards
    // against a vacuous pass), across params, returns, and multi-line sigs.
    assert_eq!(
        raw_pointer_public_signatures("pub fn bad() -> *mut u8 { core::ptr::null_mut() }").len(),
        1,
        "a public raw-pointer return must be flagged"
    );
    assert_eq!(
        raw_pointer_public_signatures("pub unsafe fn bad(p: *const u8) -> bool { p.is_null() }")
            .len(),
        1,
        "a public raw-pointer parameter must be flagged"
    );
    assert_eq!(
        raw_pointer_public_signatures("pub fn bad(\n    p: *mut u8,\n) -> i32 {\n    0\n}").len(),
        1,
        "a multi-line public raw-pointer signature must be flagged"
    );
    assert_eq!(
        raw_pointer_public_signatures("pub const fn bad(&self) -> *const u8 { core::ptr::null() }")
            .len(),
        1,
        "a public const-fn raw-pointer return must be flagged"
    );

    // And it must NOT flag the legitimate cases the seal permits.
    assert!(
        raw_pointer_public_signatures("pub(crate) unsafe fn ok(p: *mut u8) {}").is_empty(),
        "a pub(crate) raw-pointer signature is sealed and must not be flagged"
    );
    assert!(
        raw_pointer_public_signatures("pub fn ok(x: i32) -> bool { x > 0 }").is_empty(),
        "a public non-pointer signature must not be flagged"
    );
    assert!(
        raw_pointer_public_signatures(
            "// seal-allow-raw: intentionally unsealed\npub unsafe fn ok(p: *mut u8) {}"
        )
        .is_empty(),
        "a seal-allow-raw-marked signature must not be flagged"
    );
    assert!(
        raw_pointer_public_signatures("pub(super) unsafe fn ok(p: *mut u8) {}").is_empty(),
        "a pub(super) raw-pointer signature is sealed and must not be flagged"
    );
    // Mirrors the real `Codec::as_ptr`: the marker is separated from the signature
    // by a continuation comment line and a `#[must_use]` attribute.
    assert!(
        raw_pointer_public_signatures(
            "// seal-allow-raw: x\n// continued\n#[must_use]\npub const fn ok(&self) -> *const u8 { core::ptr::null() }"
        )
        .is_empty(),
        "a seal-allow-raw exemption separated by an attribute/comment block must still apply"
    );
}
