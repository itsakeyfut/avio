//! Push/pull operations for filter graph frames.

use super::build::video_buffersrc_args;
use super::convert::{
    audio_pts_ticks, av_frame_to_audio_frame, av_frame_to_video_frame, copy_audio_planes_to_av,
    copy_video_planes_to_av, pixel_format_to_av, sample_format_to_av, sample_format_to_av_name,
};
use super::{FilterGraphInner, FilterStep, build, convert, normalize};
use crate::error::FilterError;
use std::ptr::NonNull;

impl FilterGraphInner {
    // Animation

    /// Evaluates every registered [`AnimationEntry`] at time `t` and sends the
    /// updated value to the corresponding filter node via
    /// `avfilter_graph_send_command`.
    ///
    /// **No-op** when the graph has not yet been initialised (i.e. before the
    /// first `push_video` / `push_audio` call).  Individual `send_command`
    /// failures are logged as `warn!` and do not abort the loop.
    pub(crate) fn apply_animations(
        &self,
        animations: &[crate::animation::AnimationEntry],
        t: std::time::Duration,
    ) {
        use std::ffi::CString;

        // Declare `avfilter_graph_send_command` locally: bindgen includes it
        // on some platforms (Windows/VCPKG) but omits it on others (Linux apt).
        // Declaring it here avoids a platform-specific ff-sys gap while still
        // linking against the same libavfilter that ff-sys already pulls in.
        unsafe extern "C" {
            fn avfilter_graph_send_command(
                graph: *mut ff_sys::AVFilterGraph,
                target: *const std::os::raw::c_char,
                cmd: *const std::os::raw::c_char,
                arg: *const std::os::raw::c_char,
                res: *mut std::os::raw::c_char,
                res_len: std::os::raw::c_int,
                flags: std::os::raw::c_int,
            ) -> std::os::raw::c_int;
        }

        let Some(graph_ptr) = self.graph.map(|nn| nn.as_ptr()) else {
            return;
        };

        for entry in animations {
            let value = entry.track.value_at(t);
            let arg_str = format!("{value:.6}{}", entry.suffix);

            let Ok(target_cstr) = CString::new(entry.node_name.as_str()) else {
                continue;
            };
            let Ok(cmd_cstr) = CString::new(entry.param) else {
                continue;
            };
            let Ok(arg_cstr) = CString::new(arg_str.as_str()) else {
                continue;
            };

            // `c_char` is `i8` on all supported targets.
            let mut resp = [0_i8; 64];

            // SAFETY: `graph_ptr` is non-null (derived from `NonNull` after a
            // `Some` check above). `target_cstr`, `cmd_cstr`, and `arg_cstr`
            // are valid null-terminated C strings held alive for the duration
            // of this call. `resp` is a valid stack-allocated mutable buffer
            // of 64 bytes. `FilterGraph` does not implement `Sync`, so this
            // call is always single-threaded with no concurrent access to the
            // same graph.
            let ret = unsafe {
                avfilter_graph_send_command(
                    graph_ptr,
                    target_cstr.as_ptr(),
                    cmd_cstr.as_ptr(),
                    arg_cstr.as_ptr(),
                    resp.as_mut_ptr(),
                    resp.len() as std::os::raw::c_int,
                    0,
                )
            };

            if ret < 0 {
                log::warn!(
                    "send_command failed target={} cmd={} value={value:.6} code={ret}",
                    entry.node_name,
                    entry.param,
                );
            }
        }
    }

    // Video

    /// Lazily initialise the video filter graph from the first pushed frame.
    pub(super) fn ensure_video_graph(
        &mut self,
        frame: &ff_format::VideoFrame,
    ) -> Result<(), FilterError> {
        if self.graph.is_some() {
            return Ok(());
        }

        let pix_fmt = pixel_format_to_av(frame.format());
        let args = video_buffersrc_args(frame.width(), frame.height(), pix_fmt);
        let num_inputs = self.video_input_count();

        // SAFETY: all raw pointers are checked for null after allocation; the
        // graph pointer is stored in `self.graph` and kept alive for the
        // lifetime of this struct.
        unsafe {
            let graph_ptr = ff_sys::avfilter_graph_alloc();
            if graph_ptr.is_null() {
                return Err(FilterError::BuildFailed);
            }
            // SAFETY: checked non-null above.
            let graph_nn = NonNull::new_unchecked(graph_ptr);

            match Self::build_video_graph(
                graph_nn,
                &args,
                num_inputs,
                &self.steps,
                self.hw.as_ref(),
            ) {
                Ok((src_ctxs, vsink_ctx, hw_device_ctx)) => {
                    self.graph = Some(graph_nn);
                    self.src_ctxs = src_ctxs;
                    self.vsink_ctx = Some(vsink_ctx);
                    self.hw_device_ctx = hw_device_ctx;
                    log::info!(
                        "filter graph configured inputs={} filters={}",
                        num_inputs,
                        self.steps.len()
                    );
                    Ok(())
                }
                Err(e) => {
                    let mut raw = graph_nn.as_ptr();
                    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(raw));
                    Err(e)
                }
            }
        }
    }

    /// Push a video frame into the filter graph.
    pub(crate) fn push_video(
        &mut self,
        slot: usize,
        frame: &ff_format::VideoFrame,
    ) -> Result<(), FilterError> {
        self.ensure_video_graph(frame)?;

        let src_ctx = self
            .src_ctxs
            .get(slot)
            .and_then(|opt| *opt)
            .ok_or_else(|| FilterError::InvalidInput {
                slot,
                reason: format!("slot {slot} out of range (have {})", self.src_ctxs.len()),
            })?;

        // SAFETY: we allocate the `AVFrame`, fill it with `VideoFrame` data,
        // push it to the buffersrc, then immediately free it.  The buffersrc
        // keeps its own reference (`AV_BUFFERSRC_FLAG_KEEP_REF`).
        unsafe {
            let raw_frame = ff_sys::av_frame_alloc();
            if raw_frame.is_null() {
                return Err(FilterError::ProcessFailed);
            }

            (*raw_frame).width = frame.width() as std::os::raw::c_int;
            (*raw_frame).height = frame.height() as std::os::raw::c_int;
            (*raw_frame).format = pixel_format_to_av(frame.format());
            let pts = convert::video_pts_ticks(frame.timestamp());
            if pts == ff_sys::AV_NOPTS_VALUE {
                log::warn!("pts invalid, passing AV_NOPTS_VALUE to filter graph slot={slot}");
            }
            (*raw_frame).pts = pts;

            let ret = ff_sys::av_frame_get_buffer(raw_frame, 0);
            if ret < 0 {
                let mut ptr = raw_frame;
                ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
                return Err(FilterError::ProcessFailed);
            }

            copy_video_planes_to_av(frame, raw_frame);

            let ret = ff_sys::av_buffersrc_add_frame_flags(
                src_ctx.as_ptr(),
                raw_frame,
                ff_sys::BUFFERSRC_FLAG_KEEP_REF,
            );
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));

            if ret < 0 {
                return Err(FilterError::ProcessFailed);
            }
        }
        Ok(())
    }

    /// Pull the next filtered video frame, or `None` if not yet available.
    pub(crate) fn pull_video(&mut self) -> Result<Option<ff_format::VideoFrame>, FilterError> {
        let Some(sink_ctx) = self.vsink_ctx else {
            return Ok(None);
        };

        // SAFETY: `frame` is an owned frame handed to `buffersink_get_frame`; on a
        // pulled frame we convert it, then `frame` drops at end of scope.
        unsafe {
            let Ok(mut frame) = ff_sys::Frame::new() else {
                return Err(FilterError::ProcessFailed);
            };

            // NeedMore (EAGAIN) / Drained (EOF) / Err all return `None`, matching
            // the prior "any negative code → None" behavior.
            if !matches!(
                ff_sys::buffersink_get_frame(sink_ctx.as_ptr(), &mut frame),
                Ok(ff_sys::BufferSinkOutcome::Frame)
            ) {
                return Ok(None);
            }

            // `frame` drops at end of scope after the conversion reads it.
            match av_frame_to_video_frame(&frame) {
                Ok(vframe) => Ok(Some(vframe)),
                Err(()) => Err(FilterError::ProcessFailed),
            }
        }
    }

    // Audio

    /// Returns the number of video input slots required by the configured steps.
    ///
    /// Returns 2 when [`FilterStep::Overlay`] or [`FilterStep::XFade`] is present
    /// (each needs a main stream on slot 0 and a secondary stream on slot 1),
    /// 1 otherwise.
    pub(super) fn video_input_count(&self) -> usize {
        for step in &self.steps {
            if matches!(
                step,
                FilterStep::Overlay { .. }
                    | FilterStep::XFade { .. }
                    | FilterStep::Blend { .. }
                    | FilterStep::Composite { .. }
                    | FilterStep::AlphaMatte { .. }
            ) {
                return 2;
            }
            if let FilterStep::ConcatVideo { n } = step {
                return *n as usize;
            }
        }
        1
    }

    /// Returns the number of audio input slots required by the configured steps.
    pub(super) fn audio_input_count(&self) -> usize {
        for step in &self.steps {
            if let FilterStep::Amix(n) = step {
                return *n;
            }
            if let FilterStep::ConcatAudio { n } = step {
                return *n as usize;
            }
            if matches!(step, FilterStep::Duck { .. }) {
                return 2;
            }
        }
        1
    }

    /// Lazily initialise the audio filter graph from the first pushed frame.
    pub(super) fn ensure_audio_graph(
        &mut self,
        frame: &ff_format::AudioFrame,
    ) -> Result<(), FilterError> {
        if self.asink_ctx.is_some() {
            return Ok(());
        }

        let num_inputs = self.audio_input_count();
        let sample_fmt = sample_format_to_av_name(frame.format());
        let sample_rate = frame.sample_rate();
        let channels = frame.channels();

        let args = build::audio_buffersrc_args(sample_rate, sample_fmt, channels);

        // SAFETY: same contract as `ensure_video_graph` — pointers checked for
        // null, stored in `self`, freed in `Drop`.
        unsafe {
            let graph_ptr = ff_sys::avfilter_graph_alloc();
            if graph_ptr.is_null() {
                return Err(FilterError::BuildFailed);
            }
            // SAFETY: checked non-null above.
            let graph_nn = NonNull::new_unchecked(graph_ptr);

            match Self::build_audio_graph(
                graph_nn,
                &args,
                num_inputs,
                &self.steps,
                self.hw.as_ref(),
            ) {
                Ok((src_ctxs, asink_ctx)) => {
                    // The audio contexts (src_ctxs, asink_ctx) live inside `graph_nn`.
                    // When a video graph already occupies `self.graph`, keep the audio
                    // graph in `self.audio_graph` — freeing it here would leave the
                    // audio contexts dangling and segfault on the next pull_audio.
                    if self.graph.is_none() {
                        self.graph = Some(graph_nn);
                    } else {
                        self.audio_graph = Some(graph_nn);
                    }
                    let video_slots = self.src_ctxs.len();
                    self.src_ctxs.resize(video_slots + num_inputs, None);
                    for (i, ctx) in src_ctxs.into_iter().enumerate() {
                        self.src_ctxs[video_slots + i] = ctx;
                    }
                    self.asink_ctx = Some(asink_ctx);
                    log::info!(
                        "filter graph configured inputs={} filters={}",
                        num_inputs,
                        self.steps.len()
                    );
                    Ok(())
                }
                Err(e) => {
                    let mut raw = graph_nn.as_ptr();
                    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(raw));
                    Err(e)
                }
            }
        }
    }

    /// Push an audio frame into the filter graph.
    pub(crate) fn push_audio(
        &mut self,
        slot: usize,
        frame: &ff_format::AudioFrame,
    ) -> Result<(), FilterError> {
        // Two-pass loudness normalization: buffer frames instead of feeding the
        // graph.  The measurement + correction passes run on the first pull_audio.
        if self
            .steps
            .iter()
            .any(|s| matches!(s, FilterStep::LoudnessNormalize { .. }))
        {
            self.loudness_buf.push(frame.clone());
            return Ok(());
        }

        // Two-pass peak normalization: buffer frames instead of feeding the graph.
        if self
            .steps
            .iter()
            .any(|s| matches!(s, FilterStep::NormalizePeak { .. }))
        {
            self.peak_buf.push(frame.clone());
            return Ok(());
        }

        self.ensure_audio_graph(frame)?;

        let audio_inputs = self.audio_input_count();
        let video_slots = self.src_ctxs.len().saturating_sub(audio_inputs);
        let audio_slot = video_slots + slot;

        let src_ctx = self
            .src_ctxs
            .get(audio_slot)
            .and_then(|opt| *opt)
            .ok_or_else(|| FilterError::InvalidInput {
                slot,
                reason: format!("audio slot {slot} out of range (have {audio_inputs})"),
            })?;

        // SAFETY: allocate `AVFrame`, copy `AudioFrame` data, push, free.
        unsafe {
            let raw_frame = ff_sys::av_frame_alloc();
            if raw_frame.is_null() {
                return Err(FilterError::ProcessFailed);
            }

            (*raw_frame).nb_samples = frame.samples() as std::os::raw::c_int;
            (*raw_frame).sample_rate = frame.sample_rate() as std::os::raw::c_int;
            (*raw_frame).format = sample_format_to_av(frame.format());
            let pts = audio_pts_ticks(frame.timestamp(), frame.sample_rate());
            if pts == ff_sys::AV_NOPTS_VALUE {
                log::warn!("pts invalid, passing AV_NOPTS_VALUE to filter graph slot={slot}");
            }
            (*raw_frame).pts = pts;
            (*raw_frame).ch_layout.nb_channels = frame.channels() as std::os::raw::c_int;

            let ret = ff_sys::av_frame_get_buffer(raw_frame, 0);
            if ret < 0 {
                let mut ptr = raw_frame;
                ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
                return Err(FilterError::ProcessFailed);
            }

            copy_audio_planes_to_av(frame, raw_frame);

            let ret = ff_sys::av_buffersrc_add_frame_flags(
                src_ctx.as_ptr(),
                raw_frame,
                ff_sys::BUFFERSRC_FLAG_KEEP_REF,
            );
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));

            if ret < 0 {
                return Err(FilterError::ProcessFailed);
            }
        }
        Ok(())
    }

    /// Pull the next filtered audio frame, or `None` if not yet available.
    pub(crate) fn pull_audio(&mut self) -> Result<Option<ff_format::AudioFrame>, FilterError> {
        // Two-pass loudness normalization: run both passes on the first call,
        // then drain the corrected output frame-by-frame.
        if self
            .steps
            .iter()
            .any(|s| matches!(s, FilterStep::LoudnessNormalize { .. }))
        {
            if !self.loudness_pass2_done {
                self.run_loudness_normalization()?;
            }
            if self.loudness_output_idx < self.loudness_output.len() {
                let frame = self.loudness_output[self.loudness_output_idx].clone();
                self.loudness_output_idx += 1;
                return Ok(Some(frame));
            }
            return Ok(None);
        }

        // Two-pass peak normalization: run both passes on the first call,
        // then drain the corrected output frame-by-frame.
        if self
            .steps
            .iter()
            .any(|s| matches!(s, FilterStep::NormalizePeak { .. }))
        {
            if !self.peak_pass2_done {
                self.run_peak_normalization()?;
            }
            if self.peak_output_idx < self.peak_output.len() {
                let frame = self.peak_output[self.peak_output_idx].clone();
                self.peak_output_idx += 1;
                return Ok(Some(frame));
            }
            return Ok(None);
        }

        let Some(sink_ctx) = self.asink_ctx else {
            return Ok(None);
        };

        // SAFETY: `frame` is an owned frame filled via `buffersink_get_frame`; on a
        // pulled frame we convert it, then `frame` drops at end of scope.
        unsafe {
            let Ok(mut frame) = ff_sys::Frame::new() else {
                return Err(FilterError::ProcessFailed);
            };

            // NeedMore (EAGAIN) / Drained (EOF) / Err all return `None`, matching
            // the prior "any negative code → None" behavior.
            if !matches!(
                ff_sys::buffersink_get_frame(sink_ctx.as_ptr(), &mut frame),
                Ok(ff_sys::BufferSinkOutcome::Frame)
            ) {
                return Ok(None);
            }

            // `frame` drops at end of scope after the conversion reads it.
            match av_frame_to_audio_frame(&frame) {
                Ok(aframe) => Ok(Some(aframe)),
                Err(()) => Err(FilterError::ProcessFailed),
            }
        }
    }

    /// Signal end-of-stream to every audio `buffersrc`, so buffering filters
    /// (e.g. `atempo`) flush their pending output for the following
    /// [`pull_audio`](Self::pull_audio) drain. No-op if the audio graph has not
    /// been built yet (nothing has been pushed).
    pub(crate) fn flush_audio(&mut self) {
        if self.asink_ctx.is_none() {
            return;
        }
        let audio_inputs = self.audio_input_count();
        let video_slots = self.src_ctxs.len().saturating_sub(audio_inputs);
        for slot in 0..audio_inputs {
            if let Some(src_ctx) = self.src_ctxs.get(video_slots + slot).and_then(|opt| *opt) {
                // SAFETY: `src_ctx` is a valid audio buffersrc owned by this
                // graph; `av_buffersrc_close` signals EOF on it.
                unsafe {
                    ff_sys::av_buffersrc_close(src_ctx.as_ptr(), ff_sys::AV_NOPTS_VALUE, 0u32);
                }
            }
        }
    }

    // Two-pass loudness normalization

    /// Run EBU R128 two-pass loudness normalization over `self.loudness_buf`:
    ///
    /// 1. Measure integrated loudness with an `ebur128=peak=true:metadata=1` graph.
    /// 2. Compute `gain_db = target_lufs − measured_lufs`.
    /// 3. Apply gain with a `volume={gain_db}dB` graph.
    /// 4. Store corrected frames in `self.loudness_output`.
    fn run_loudness_normalization(&mut self) -> Result<(), FilterError> {
        let target_lufs = self
            .steps
            .iter()
            .find_map(|s| {
                if let FilterStep::LoudnessNormalize { target_lufs, .. } = s {
                    Some(*target_lufs)
                } else {
                    None
                }
            })
            .ok_or(FilterError::BuildFailed)?;

        // Mark done early to prevent re-entry on error.
        self.loudness_pass2_done = true;

        if self.loudness_buf.is_empty() {
            return Ok(());
        }

        // === Pass 1: measure integrated loudness ===
        let measured_lufs = unsafe {
            let graph = ff_sys::avfilter_graph_alloc();
            if graph.is_null() {
                return Err(FilterError::BuildFailed);
            }
            let result = normalize::run_ebur128_graph(graph, &self.loudness_buf);
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            result?
        };

        let gain_db = target_lufs - measured_lufs;
        log::info!(
            "loudness normalization measured_lufs={:.1} target_lufs={:.1} gain_db={:.2}",
            measured_lufs,
            target_lufs,
            gain_db,
        );

        // === Pass 2: apply volume correction ===
        self.loudness_output = unsafe {
            let graph = ff_sys::avfilter_graph_alloc();
            if graph.is_null() {
                return Err(FilterError::BuildFailed);
            }
            let result = normalize::run_volume_graph(graph, &self.loudness_buf, gain_db);
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            result?
        };

        Ok(())
    }

    // Two-pass peak normalization

    /// Run peak-level two-pass normalization over `self.peak_buf`:
    ///
    /// 1. Measure the true peak with an `astats=metadata=1` graph.
    /// 2. Compute `gain_db = target_db − measured_peak_db`.
    /// 3. Apply gain with a `volume={gain_db}dB` graph.
    /// 4. Store corrected frames in `self.peak_output`.
    fn run_peak_normalization(&mut self) -> Result<(), FilterError> {
        let target_db = self
            .steps
            .iter()
            .find_map(|s| {
                if let FilterStep::NormalizePeak { target_db } = s {
                    Some(*target_db)
                } else {
                    None
                }
            })
            .ok_or(FilterError::BuildFailed)?;

        // Mark done early to prevent re-entry on error.
        self.peak_pass2_done = true;

        if self.peak_buf.is_empty() {
            return Ok(());
        }

        // === Pass 1: measure peak level ===
        let measured_peak_db = unsafe {
            let graph = ff_sys::avfilter_graph_alloc();
            if graph.is_null() {
                return Err(FilterError::BuildFailed);
            }
            let result = normalize::run_astats_graph(graph, &self.peak_buf);
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            result?
        };

        let gain_db = target_db - measured_peak_db;
        log::info!(
            "peak normalization measured_peak_db={:.2} target_db={:.2} gain_db={:.2}",
            measured_peak_db,
            target_db,
            gain_db,
        );

        // === Pass 2: apply volume correction ===
        self.peak_output = unsafe {
            let graph = ff_sys::avfilter_graph_alloc();
            if graph.is_null() {
                return Err(FilterError::BuildFailed);
            }
            let result = normalize::run_volume_graph(graph, &self.peak_buf, gain_db);
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            result?
        };

        Ok(())
    }
}

impl Drop for FilterGraphInner {
    fn drop(&mut self) {
        if let Some(ptr) = self.graph.take() {
            // SAFETY: `graph` is non-null (guaranteed by `NonNull`), and we are
            // the sole owner.  `avfilter_graph_free` also frees all
            // `AVFilterContext`s attached to the graph, so `src_ctxs`,
            // `vsink_ctx`, and `asink_ctx` must NOT be freed individually.
            // Filter contexts that held `av_buffer_ref` refs to `hw_device_ctx`
            // release those refs here as well.
            unsafe {
                let mut raw = ptr.as_ptr();
                ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(raw));
            }
        }
        // Free the secondary audio graph when present (built when both video and
        // audio chains are used). Its filter contexts are freed along with it.
        if let Some(ptr) = self.audio_graph.take() {
            // SAFETY: non-null `NonNull`, sole owner; frees its attached contexts.
            unsafe {
                let mut raw = ptr.as_ptr();
                ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(raw));
            }
        }
        // Free our own reference to the hardware device context AFTER the graph
        // has been freed.  The graph's filter contexts held their own references
        // (created via `av_buffer_ref` in `create_hw_filter`); those were
        // released by `avfilter_graph_free` above.
        if let Some(mut hw_ctx) = self.hw_device_ctx.take() {
            // SAFETY: `hw_ctx` is the sole remaining reference owned by this
            // struct; the filter graph (and its filter contexts) has already
            // been freed above.
            unsafe {
                ff_sys::av_buffer_unref(std::ptr::addr_of_mut!(hw_ctx));
            }
        }
    }
}
