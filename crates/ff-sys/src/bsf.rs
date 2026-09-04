//! RAII owner for an `AVBSFContext` (a bitstream filter, or a chain of them).
//!
//! [`BsfContext`] rewrites packet payloads in place between a demuxer and a muxer,
//! without decoding: Annex B / AVCC conversion, extradata extraction, container
//! metadata rewrites. The context is freed exactly once on drop.
//!
//! # What this is *not* for
//!
//! libavformat already inserts the bitstream filter a container requires, on its
//! own: a muxer's `check_bitstream` callback runs from `write_packets_common`, which
//! both `av_write_frame` and `av_interleaved_write_frame` go through, gated only on
//! `AVFMT_FLAG_AUTO_BSF` (set by default). Copying H.264 from MP4 into MPEG-TS
//! therefore already produces Annex B without any caller involvement. This type is
//! for the filters libavformat never applies by itself — `extract_extradata`,
//! `dump_extra`, `h264_metadata`, `setts` — which a caller has to ask for. See
//! ADR-0011.
//!
//! # Lifecycle
//!
//! [`open`](BsfContext::open) allocates, fills the input parameters and time base,
//! and calls `av_bsf_init` as one step, so an uninitialised context cannot be
//! observed. That is why [`send_packet`](BsfContext::send_packet) and
//! [`receive_packet`](BsfContext::receive_packet) are safe functions where
//! [`CodecContext`](crate::CodecContext)'s equivalents are not: there, an unopened
//! context is reachable and the caller has to promise it opened one.
//!
//! Drive it exactly as a codec: send a packet, then receive until the outcome is no
//! longer [`ReceiveOutcome::Frame`] (one input packet may yield several outputs, or
//! none), and at the end [`send_eof`](BsfContext::send_eof) and drain to
//! [`ReceiveOutcome::Drained`].

use std::ffi::CString;
use std::ptr::NonNull;

use crate::codec_context::classify_receive;
use crate::{
    AVBSFContext, AVRational, AvError, CodecParameters, Packet, ReceiveOutcome,
    av_bsf_free as ffi_av_bsf_free, av_bsf_get_null_filter as ffi_av_bsf_get_null_filter,
    av_bsf_init as ffi_av_bsf_init, av_bsf_list_parse_str as ffi_av_bsf_list_parse_str,
    av_bsf_receive_packet as ffi_av_bsf_receive_packet,
    av_bsf_send_packet as ffi_av_bsf_send_packet, avcodec_parameters_copy,
};

/// An owned, initialised `AVBSFContext`.
///
/// Freed exactly once on drop, guaranteed by construction: the value owns a
/// [`NonNull`] and is neither `Copy` nor `Clone`, so it drops exactly once and
/// cannot be duplicated.
#[derive(Debug)]
pub struct BsfContext {
    ptr: NonNull<AVBSFContext>,
}

impl BsfContext {
    /// Builds and initialises a filter chain from an `FFmpeg` filter spec.
    ///
    /// `spec` is the syntax `ffmpeg -bsf` takes: a comma-separated chain, each
    /// element optionally carrying `=`-separated options, for example
    /// `"h264_mp4toannexb"` or `"h264_metadata=level=40,dump_extra"`.
    ///
    /// `par_in` describes the packets that will be sent; `None` leaves the
    /// zero-initialised block `av_bsf_alloc` provides, which filters that do not
    /// inspect the stream parameters accept. `time_base_in` is the time base those
    /// packets' timestamps are in; the filtered packets come out in
    /// [`output_time_base`](Self::output_time_base).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] when the spec names an unregistered filter
    /// ([`BSF_NOT_FOUND`](crate::error_codes::BSF_NOT_FOUND)), is malformed or empty
    /// ([`EINVAL`](crate::error_codes::EINVAL)), contains an interior NUL byte, or
    /// when allocation or `av_bsf_init` fails.
    pub fn open(
        spec: &str,
        par_in: Option<CodecParameters<'_>>,
        time_base_in: AVRational,
    ) -> Result<Self, AvError> {
        crate::ensure_initialized();
        let c_spec = CString::new(spec).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        let mut raw: *mut AVBSFContext = std::ptr::null_mut();
        // SAFETY: `c_spec` is a valid NUL-terminated string that outlives the call,
        //         and `raw` is a valid out-pointer. On success FFmpeg stores a fresh
        //         context there; on failure it leaves it null and frees its own
        //         partial state.
        let ret = unsafe { ffi_av_bsf_list_parse_str(c_spec.as_ptr(), &raw mut raw) };
        if ret < 0 {
            return Err(AvError::new(ret));
        }
        Self::finish(raw, par_in, time_base_in)
    }

    /// Builds and initialises a filter that passes packets through unchanged.
    ///
    /// `av_bsf_get_null_filter` is the neutral element of a chain. It is available in
    /// every `FFmpeg` build: where the `null` bitstream filter is not compiled in it
    /// falls back to the internal empty filter list, so unlike
    /// [`open`](Self::open)`("null", …)` this never depends on the build's filter set.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if allocation or `av_bsf_init` fails.
    pub fn passthrough(
        par_in: Option<CodecParameters<'_>>,
        time_base_in: AVRational,
    ) -> Result<Self, AvError> {
        crate::ensure_initialized();
        let mut raw: *mut AVBSFContext = std::ptr::null_mut();
        // SAFETY: `raw` is a valid out-pointer; the function stores a fresh context
        //         there or leaves it null and returns a negative code.
        let ret = unsafe { ffi_av_bsf_get_null_filter(&raw mut raw) };
        if ret < 0 {
            return Err(AvError::new(ret));
        }
        Self::finish(raw, par_in, time_base_in)
    }

    /// Fills the input parameters and time base of a freshly allocated `raw`, then
    /// initialises it. Takes ownership of `raw`, so every failure path frees it.
    fn finish(
        raw: *mut AVBSFContext,
        par_in: Option<CodecParameters<'_>>,
        time_base_in: AVRational,
    ) -> Result<Self, AvError> {
        let Some(ptr) = NonNull::new(raw) else {
            return Err(AvError::new(crate::error_codes::ENOMEM));
        };
        // From here the context is owned: `Self` frees it on drop, including on the
        // early return below, so no path leaks it.
        let ctx = Self { ptr };

        if let Some(par) = par_in {
            // SAFETY: `ctx.ptr` is a valid allocated context whose `par_in` FFmpeg
            //         allocated alongside it, and `par.as_raw()` borrows a live
            //         parameters block; `avcodec_parameters_copy` deep-copies.
            let ret = unsafe { avcodec_parameters_copy((*ctx.ptr.as_ptr()).par_in, par.as_raw()) };
            if ret < 0 {
                return Err(AvError::new(ret));
            }
        }
        // SAFETY: `ctx.ptr` is a valid allocated context; `time_base_in` is a plain field.
        unsafe { (*ctx.ptr.as_ptr()).time_base_in = time_base_in };

        // SAFETY: `ctx.ptr` is a valid allocated context with its input side filled in.
        let ret = unsafe { ffi_av_bsf_init(ctx.ptr.as_ptr()) };
        if ret < 0 {
            return Err(AvError::new(ret));
        }
        Ok(ctx)
    }

    /// Returns the parameters of the filtered stream, set by `av_bsf_init`.
    ///
    /// A filter may rewrite the stream description — `h264_mp4toannexb` replaces the
    /// `avcC` extradata with Annex B parameter sets — so an output stream fed by this
    /// filter must be configured from here, not from the input stream.
    #[must_use]
    pub fn output_params(&self) -> CodecParameters<'_> {
        // SAFETY: `self.ptr` is a valid initialised context. `av_bsf_alloc` allocates
        //         `par_out` with the context and `av_bsf_init` fills it, so it is
        //         non-null for the whole of `self`'s life.
        unsafe {
            let par = (*self.ptr.as_ptr()).par_out;
            CodecParameters::from_raw(NonNull::new_unchecked(par))
        }
    }

    /// Returns the time base the filtered packets' timestamps are in, set by
    /// `av_bsf_init`.
    #[must_use]
    pub fn output_time_base(&self) -> AVRational {
        // SAFETY: `self.ptr` is a valid initialised context; `time_base_out` is a plain field.
        unsafe { (*self.ptr.as_ptr()).time_base_out }
    }

    /// Submits a packet for filtering.
    ///
    /// The filter **takes ownership of the packet's contents and blanks `pkt`**, so
    /// the caller neither may nor need unref it afterwards, and may reuse it as the
    /// destination of [`receive_packet`](Self::receive_packet). `pkt` is untouched if
    /// this returns an error.
    ///
    /// # Sending an empty packet ends the stream
    ///
    /// `av_bsf_send_packet` treats an *empty* packet — one with no data and no side
    /// data — as the end-of-stream signal, exactly as a null one: it unrefs the
    /// packet, latches the filter into EOF and returns success. Every later
    /// non-empty send then fails with `EINVAL` ("A non-NULL packet sent after an
    /// EOF"). So a caller that forwards whatever a demuxer produced can end the
    /// stream by accident and only find out one packet later. Use
    /// [`send_eof`](Self::send_eof) to end the stream deliberately, and do not send
    /// packets with no payload.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the filter has output waiting
    /// ([`EAGAIN`](crate::error_codes::EAGAIN) — drain it first) or on a real failure.
    pub fn send_packet(&mut self, pkt: &mut Packet) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid initialised context and `pkt` is a valid owned
        //         packet borrowed mutably for the call, which is what lets FFmpeg move
        //         its contents out.
        let ret = unsafe { ffi_av_bsf_send_packet(self.ptr.as_ptr(), pkt.as_mut_ptr()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Signals end of stream by sending an empty packet, so the filter flushes
    /// whatever it has buffered.
    ///
    /// After this, loop [`receive_packet`](Self::receive_packet) until it returns
    /// [`ReceiveOutcome::Drained`]. This is the one supported way to drain, so a
    /// caller cannot forget the flush.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] on a real failure.
    pub fn send_eof(&mut self) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid initialised context; a null packet is the
        //         documented end-of-stream signal for `av_bsf_send_packet`.
        let ret = unsafe { ffi_av_bsf_send_packet(self.ptr.as_ptr(), std::ptr::null_mut()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Receives a filtered packet into `pkt`, returning a typed [`ReceiveOutcome`].
    ///
    /// `EAGAIN` (send more input) and `EOF` (drained) are returned as outcomes rather
    /// than errors. One input packet may produce several outputs, so call this until
    /// it stops returning [`ReceiveOutcome::Frame`].
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] on a real failure (neither drain state).
    pub fn receive_packet(&mut self, pkt: &mut Packet) -> Result<ReceiveOutcome, AvError> {
        // SAFETY: `self.ptr` is a valid initialised context; `pkt` is a valid owned
        //         packet whose contents FFmpeg overwrites on success and leaves alone
        //         otherwise.
        let ret = unsafe { ffi_av_bsf_receive_packet(self.ptr.as_ptr(), pkt.as_mut_ptr()) };
        classify_receive(if ret < 0 { Err(ret) } else { Ok(()) })
    }
}

impl Drop for BsfContext {
    fn drop(&mut self) {
        let mut raw = self.ptr.as_ptr();
        // SAFETY: `raw` is the context this value owns and has not freed; `av_bsf_free`
        //         frees it and nulls the local, and `self` is being dropped, so nothing
        //         can reach the pointer again.
        unsafe { ffi_av_bsf_free(&raw mut raw) };
    }
}

// SAFETY: `AVBSFContext` is not thread-safe for concurrent access, but ownership
//         transfer between threads is safe because Rust's ownership model guarantees
//         exclusive access. `Sync` is deliberately not implemented.
unsafe impl Send for BsfContext {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A time base to hand the filter; the value only has to be non-degenerate.
    const TB: AVRational = AVRational { num: 1, den: 1000 };

    #[test]
    fn bsf_context_should_reject_an_unknown_filter_name() {
        // Deterministic on every build: the lookup fails before any filter runs, so
        // this does not depend on which filters FFmpeg was compiled with.
        let err = BsfContext::open("no_such_bitstream_filter", None, TB)
            .expect_err("an unregistered filter name must not open");
        assert_eq!(err.code(), crate::error_codes::BSF_NOT_FOUND);
    }

    #[test]
    fn bsf_context_should_reject_an_empty_spec() {
        // An empty spec is not "no filter": `av_bsf_list_parse_str` takes the null
        // filter only for a null pointer, and rejects an empty token with EINVAL.
        let err = BsfContext::open("", None, TB).expect_err("an empty spec must not open");
        assert_eq!(err.code(), crate::error_codes::EINVAL);
    }

    #[test]
    fn passthrough_should_drain_from_need_input_to_drained() {
        // Drives the whole lifecycle -- alloc, init, receive, send, receive, drop --
        // with no media fixture and no dependence on the build's filter set, because
        // `av_bsf_get_null_filter` falls back to the internal empty filter list.
        let mut bsf = BsfContext::passthrough(None, TB).expect("the null filter must open");
        // bindgen's `AVRational` derives no `PartialEq`, so compare its fields.
        let out_tb = bsf.output_time_base();
        assert_eq!(
            (out_tb.num, out_tb.den),
            (TB.num, TB.den),
            "the null filter must pass the time base through"
        );

        let mut pkt = Packet::new().expect("packet");
        // Nothing sent yet, so the filter wants input rather than being drained. An
        // `open` that skipped `av_bsf_init` cannot get this far.
        assert!(
            matches!(bsf.receive_packet(&mut pkt), Ok(ReceiveOutcome::NeedInput)),
            "a fresh filter must ask for input"
        );
        bsf.send_eof().expect("send_eof");
        assert!(
            matches!(bsf.receive_packet(&mut pkt), Ok(ReceiveOutcome::Drained)),
            "after end of stream the filter must report drained"
        );
    }
}
