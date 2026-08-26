use ff_sys::{CodecContext, HwDeviceContext};

use super::{AVHWDeviceType, AVPixelFormat, DecodeError, HardwareAccel, VideoDecoderInner};

impl VideoDecoderInner {
    /// Maps our `HardwareAccel` enum to the corresponding FFmpeg `AVHWDeviceType`.
    ///
    /// Returns `None` for `Auto` and `None` variants as they require special handling.
    pub(super) fn hw_accel_to_device_type(accel: HardwareAccel) -> Option<AVHWDeviceType> {
        match accel {
            HardwareAccel::Auto => None,
            HardwareAccel::None => None,
            HardwareAccel::Nvdec => Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_CUDA),
            HardwareAccel::Qsv => Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_QSV),
            HardwareAccel::Amf => Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_D3D11VA), // AMF uses D3D11
            HardwareAccel::VideoToolbox => {
                Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_VIDEOTOOLBOX)
            }
            HardwareAccel::Vaapi => Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_VAAPI),
        }
    }

    /// Returns the hardware decoders to try in priority order for Auto mode.
    const fn hw_accel_auto_priority() -> &'static [HardwareAccel] {
        // Priority order: NVDEC, QSV, VideoToolbox, VA-API, AMF
        &[
            HardwareAccel::Nvdec,
            HardwareAccel::Qsv,
            HardwareAccel::VideoToolbox,
            HardwareAccel::Vaapi,
            HardwareAccel::Amf,
        ]
    }

    /// Attempts to initialize hardware acceleration.
    ///
    /// # Arguments
    ///
    /// * `codec_ctx` - The codec context to configure
    /// * `accel` - Requested hardware acceleration mode
    ///
    /// # Returns
    ///
    /// Returns `Ok((Some(device), active_accel))` if hardware acceleration was
    /// initialized (the owned [`HwDeviceContext`] must be kept alive as long as
    /// the codec context uses it), or `Ok((None, HardwareAccel::None))` if
    /// software decoding should be used.
    ///
    /// # Errors
    ///
    /// Returns an error only if a specific hardware accelerator was requested but failed to initialize.
    pub(super) fn init_hardware_accel(
        codec_ctx: &mut CodecContext,
        accel: HardwareAccel,
    ) -> Result<(Option<HwDeviceContext>, HardwareAccel), DecodeError> {
        match accel {
            HardwareAccel::Auto => {
                // Try hardware accelerators in priority order
                for &hw_type in Self::hw_accel_auto_priority() {
                    match Self::try_init_hw_device(codec_ctx, hw_type) {
                        Ok((Some(device), active)) => {
                            log::info!("hwaccel selected backend={}", active.name());
                            return Ok((Some(device), active));
                        }
                        _ => {
                            log::debug!(
                                "hwaccel probe failed backend={} trying next",
                                hw_type.name()
                            );
                        }
                    }
                }
                // All hardware accelerators failed, fall back to software
                Ok((None, HardwareAccel::None))
            }
            HardwareAccel::None => {
                // Software decoding explicitly requested
                Ok((None, HardwareAccel::None))
            }
            _ => {
                // Specific hardware accelerator requested
                Self::try_init_hw_device(codec_ctx, accel)
            }
        }
    }

    /// Tries to initialize a specific hardware device and attach it to `codec_ctx`.
    fn try_init_hw_device(
        codec_ctx: &mut CodecContext,
        accel: HardwareAccel,
    ) -> Result<(Option<HwDeviceContext>, HardwareAccel), DecodeError> {
        // Get the FFmpeg device type
        let Some(device_type) = Self::hw_accel_to_device_type(accel) else {
            return Ok((None, HardwareAccel::None));
        };

        // Create the hardware device context (owned; freed on drop). Failure here
        // means the backend is unavailable on this system.
        let device = HwDeviceContext::new(device_type)
            .map_err(|_| DecodeError::HwAccelUnavailable { accel })?;

        // Attach it to the codec context, which takes its own reference. `device`
        // keeps ours for the caller to hold alongside the decoder.
        codec_ctx
            .set_hw_device_ctx(&device)
            .map_err(|_| DecodeError::HwAccelUnavailable { accel })?;

        Ok((Some(device), accel))
    }

    /// Returns the currently active hardware acceleration mode.
    pub(crate) fn hardware_accel(&self) -> HardwareAccel {
        self.active_hw_accel
    }

    /// Checks if a pixel format is a hardware format.
    ///
    /// Hardware formats include: D3D11, CUDA, VAAPI, VideoToolbox, QSV, etc.
    const fn is_hardware_format(format: AVPixelFormat) -> bool {
        matches!(
            format,
            ff_sys::AVPixelFormat_AV_PIX_FMT_D3D11
                | ff_sys::AVPixelFormat_AV_PIX_FMT_CUDA
                | ff_sys::AVPixelFormat_AV_PIX_FMT_VAAPI
                | ff_sys::AVPixelFormat_AV_PIX_FMT_VIDEOTOOLBOX
                | ff_sys::AVPixelFormat_AV_PIX_FMT_QSV
                | ff_sys::AVPixelFormat_AV_PIX_FMT_VDPAU
                | ff_sys::AVPixelFormat_AV_PIX_FMT_DXVA2_VLD
                | ff_sys::AVPixelFormat_AV_PIX_FMT_OPENCL
                | ff_sys::AVPixelFormat_AV_PIX_FMT_MEDIACODEC
                | ff_sys::AVPixelFormat_AV_PIX_FMT_VULKAN
        )
    }

    /// Transfers a hardware frame to CPU memory if needed.
    ///
    /// If `self.frame` is a hardware frame, creates a new software frame
    /// and transfers the data from GPU to CPU memory.
    pub(super) fn transfer_hardware_frame_if_needed(&mut self) -> Result<(), DecodeError> {
        let frame_format = self.frame.format();

        if !Self::is_hardware_format(frame_format) {
            // Not a hardware frame, no transfer needed
            return Ok(());
        }

        // Create a temporary software frame for transfer (owned; freed on scope exit).
        let mut sw_frame = ff_sys::Frame::new().map_err(|e| DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to allocate software frame for hardware transfer: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // Transfer GPU-side data from the hardware frame into the software frame.
        sw_frame
            .hwframe_transfer_data(&self.frame, 0)
            .map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to transfer hardware frame to CPU memory: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Copy metadata (pts, duration, etc.) from hardware frame to software frame
        sw_frame.set_pts(self.frame.pts());
        sw_frame.set_pkt_dts(self.frame.pkt_dts());
        sw_frame.set_duration(self.frame.duration());
        sw_frame.set_time_base(self.frame.time_base());

        // Replace self.frame with the software frame. `move_ref` transfers
        // sw_frame's buffers into self.frame, leaving sw_frame blank (freed on
        // scope exit).
        self.frame.unref();
        self.frame.move_ref(&mut sw_frame);

        Ok(())
    }
}
