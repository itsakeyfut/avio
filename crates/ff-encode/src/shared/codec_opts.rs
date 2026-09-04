//! Applying caller-supplied codec-private options to an encoder context.
//!
//! The escape hatch behind [`VideoEncoderBuilder::codec_opt`] and
//! [`AudioEncoderBuilder::codec_opt`], kept in one place because three separate
//! sites open a codec context (video single-pass, video two-pass, audio) and an
//! option dropped at any of them fails silently.
//!
//! [`VideoEncoderBuilder::codec_opt`]: crate::VideoEncoderBuilder::codec_opt
//! [`AudioEncoderBuilder::codec_opt`]: crate::AudioEncoderBuilder::codec_opt

use crate::EncodeError;

/// Apply caller-supplied `(key, value)` options to `ctx` before `avcodec_open2`.
///
/// Targets the codec's `priv_data`, so these are codec-*private* options
/// (`x264-params`, `aq-mode`, libopus tuning) rather than the `AVCodecContext`
/// fields the typed builders already own.
///
/// Pairs are applied in the order given, so a repeated key resolves the way the
/// caller wrote it.
///
/// # Errors
///
/// Returns [`EncodeError::InvalidConfig`] naming the key, the value and the
/// encoder as soon as one option is rejected. Unlike the typed codec options,
/// which log and continue because `preset`/`tune` legitimately fail on hardware
/// encoders, a key here was named by hand and dropping it silently would defeat
/// the escape hatch.
///
/// The variant is `InvalidConfig` rather than [`EncodeError::Ffmpeg`] because
/// the fault is the caller's configuration, and only that variant can carry the
/// key and value that make the message actionable. `FFmpeg`'s numeric code is
/// kept in the text so nothing is lost: `AVERROR_OPTION_NOT_FOUND` (the key does
/// not exist) and `AVERROR(ERANGE)` (the value is out of range) stay
/// distinguishable.
pub(crate) fn apply_codec_opts(
    ctx: &mut ff_sys::CodecContext,
    opts: &[(String, String)],
    encoder_name: &str,
) -> Result<(), EncodeError> {
    for (key, value) in opts {
        ctx.set_opt(key, value)
            .map_err(|e| EncodeError::InvalidConfig {
                reason: format!(
                    "codec option rejected key={key} value={value} encoder={encoder_name} error={} (code={})",
                    ff_sys::av_error_string(e.code()),
                    e.code()
                ),
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generic context (no codec) has null `priv_data`, so `av_opt_set`
    /// reports any key as unknown. That holds on every FFmpeg build, which is
    /// what makes these assertions build-independent — the same fact `ff-sys`
    /// relies on in `codec_context.rs`.
    fn generic_ctx() -> ff_sys::CodecContext {
        ff_sys::CodecContext::new(None).expect("alloc should succeed")
    }

    #[test]
    fn apply_codec_opts_should_reject_an_unknown_option() {
        let mut ctx = generic_ctx();
        let opts = [("no_such_option_xyz".to_string(), "1".to_string())];
        let err = apply_codec_opts(&mut ctx, &opts, "libx264")
            .expect_err("an unknown key must not be accepted");
        assert!(
            matches!(err, EncodeError::InvalidConfig { .. }),
            "expected InvalidConfig, got {err:?}"
        );
    }

    #[test]
    fn apply_codec_opts_should_name_the_failing_key_and_encoder() {
        // A bare "invalid configuration" would leave the caller guessing which
        // of several options FFmpeg refused.
        let mut ctx = generic_ctx();
        let opts = [("no_such_option_xyz".to_string(), "banana".to_string())];
        let err = apply_codec_opts(&mut ctx, &opts, "libx264").expect_err("must fail");
        let message = err.to_string();
        assert!(
            message.contains("no_such_option_xyz"),
            "the error must name the key; got {message:?}"
        );
        assert!(
            message.contains("banana"),
            "the error must name the value; got {message:?}"
        );
        assert!(
            message.contains("libx264"),
            "the error must name the encoder; got {message:?}"
        );
        // The numeric code is what separates "no such key" from "value out of
        // range"; `docs/rules/error-handling.md` keeps FFmpeg's codes rather
        // than flattening them into prose.
        assert!(
            message.contains("code="),
            "the error must keep FFmpeg's numeric code; got {message:?}"
        );
        // A `\`-continued format string silently bakes the next line's indent
        // into the literal, which is how this message once acquired a 22-space
        // gap. Cheap to assert, invisible otherwise.
        assert!(
            !message.contains("  "),
            "the message must not contain a run of spaces; got {message:?}"
        );
    }

    #[test]
    fn apply_codec_opts_empty_should_succeed() {
        // Non-vacuity guard: a helper that always returned an error would pass
        // both tests above.
        let mut ctx = generic_ctx();
        apply_codec_opts(&mut ctx, &[], "libx264").expect("no options must be a no-op");
    }

    #[test]
    fn apply_codec_opts_should_stop_at_the_first_rejected_option() {
        // Reporting the first failure rather than the last is what makes the
        // message actionable when several keys are wrong.
        let mut ctx = generic_ctx();
        let opts = [
            ("first_bad_option".to_string(), "1".to_string()),
            ("second_bad_option".to_string(), "2".to_string()),
        ];
        let err = apply_codec_opts(&mut ctx, &opts, "libx264").expect_err("must fail");
        let message = err.to_string();
        assert!(
            message.contains("first_bad_option"),
            "the first failure must be reported; got {message:?}"
        );
        assert!(
            !message.contains("second_bad_option"),
            "application must stop at the first failure; got {message:?}"
        );
    }
}
