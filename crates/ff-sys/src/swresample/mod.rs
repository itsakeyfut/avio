//! SwResample wrapper functions for audio resampling and format conversion.
//!
//! This module provides thin wrapper functions around FFmpeg's libswresample API
//! for resampling audio data and converting between sample formats and channel layouts.
//!
//! The public surface is pointer-free (`estimate_output_samples`, the
//! `channel_layout` / `sample_format` submodules); the raw pointer-taking wrappers
//! (`alloc_set_opts2` / `init` / `convert` / …) are `pub(crate)`, driven only by
//! the owned [`ResampleContext`](crate::ResampleContext). (`audio_fifo` remains a
//! separate, still-raw public submodule.)
//!
//! # Safety (crate-internal wrappers)
//!
//! For the `pub(crate)` wrappers, callers are responsible for:
//! - Ensuring pointers are valid before passing to these functions
//! - Properly freeing resources using the corresponding free functions
//! - Not using pointers after they have been freed
//! - Ensuring source and destination buffers match the expected sample counts and formats

mod context;
mod convert;

pub mod audio_fifo;
pub mod channel_layout;
pub mod sample_format;

pub(crate) use context::{alloc_set_opts2, init};
pub(crate) use convert::convert;
pub use convert::estimate_output_samples;

#[cfg(test)]
mod tests {
    // Test-only scaffolding drives a raw FFmpeg decode pipeline directly, so the
    // helper `unsafe fn`s keep the terse pre-2024 style. The library's safe layer
    // is held to `unsafe_op_in_unsafe_fn` at the crate root.
    #![allow(unsafe_op_in_unsafe_fn)]

    use super::*;

    // ========================================================================
    // Integration tests with real audio file
    // ========================================================================

    /// Helper to load audio file path from assets.
    fn get_test_audio_path() -> std::path::PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(format!(
            "{}/../../assets/audio/konekonoosanpo.mp3",
            manifest_dir
        ))
    }

    /// Helper struct to manage decoded audio context.
    struct AudioDecoder {
        format_ctx: *mut crate::AVFormatContext,
        codec_ctx: crate::CodecContext,
        stream_index: i32,
        frame: *mut crate::AVFrame,
        packet: *mut crate::AVPacket,
    }

    impl AudioDecoder {
        /// Open an audio file and prepare for decoding.
        unsafe fn open(path: &std::path::Path) -> Result<Self, i32> {
            use crate::{
                AVMediaType_AVMEDIA_TYPE_AUDIO, av_frame_alloc, av_packet_alloc,
                avformat::{close_input, find_stream_info, open_input},
            };

            // Open input file
            let format_ctx = open_input(path)?;
            find_stream_info(format_ctx)?;

            // Find audio stream
            let mut stream_index = -1;
            let nb_streams = (*format_ctx).nb_streams;

            for i in 0..nb_streams {
                let stream = *(*format_ctx).streams.add(i as usize);
                let codecpar = (*stream).codecpar;
                if (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
                    stream_index = i as i32;
                    break;
                }
            }

            if stream_index < 0 {
                close_input(&mut (format_ctx as *mut _));
                return Err(crate::error_codes::EINVAL);
            }

            // Get codec parameters
            let stream = *(*format_ctx).streams.add(stream_index as usize);
            let codecpar = (*stream).codecpar;

            // Find decoder
            let codec = crate::Codec::find_decoder((*codecpar).codec_id)
                .ok_or(crate::error_codes::EINVAL)?;

            // Allocate, configure, and open the owned codec context (frees on drop).
            let mut codec_ctx = crate::CodecContext::new(Some(codec)).map_err(|e| e.code())?;
            codec_ctx
                .parameters_to_context(codecpar)
                .map_err(|e| e.code())?;
            codec_ctx.open_codec(codec).map_err(|e| e.code())?;

            // Allocate frame and packet. On failure the owned `codec_ctx` drops (frees).
            let frame = av_frame_alloc();
            if frame.is_null() {
                close_input(&mut (format_ctx as *mut _));
                return Err(crate::error_codes::ENOMEM);
            }

            let packet = av_packet_alloc();
            if packet.is_null() {
                crate::av_frame_free(&mut (frame as *mut _));
                close_input(&mut (format_ctx as *mut _));
                return Err(crate::error_codes::ENOMEM);
            }

            Ok(Self {
                format_ctx,
                codec_ctx,
                stream_index,
                frame,
                packet,
            })
        }

        /// Decode next audio frame.
        /// Returns None when EOF is reached.
        unsafe fn decode_frame(&mut self) -> Option<&crate::AVFrame> {
            use crate::{avcodec, avformat, error_codes};

            loop {
                // Try to receive a frame from the decoder
                match avcodec::receive_frame(self.codec_ctx.as_mut_ptr(), self.frame) {
                    Ok(()) => return Some(&*self.frame),
                    Err(e) if e == error_codes::EAGAIN => {
                        // Need more input
                    }
                    Err(_) => return None,
                }

                // Read next packet
                loop {
                    match avformat::read_frame(self.format_ctx, self.packet) {
                        Ok(()) => {
                            if (*self.packet).stream_index == self.stream_index {
                                break;
                            }
                            // Wrong stream, unref and continue
                            crate::av_packet_unref(self.packet);
                        }
                        Err(_) => {
                            // EOF or error - flush decoder
                            let _ =
                                avcodec::send_packet(self.codec_ctx.as_mut_ptr(), std::ptr::null());
                            match avcodec::receive_frame(self.codec_ctx.as_mut_ptr(), self.frame) {
                                Ok(()) => return Some(&*self.frame),
                                Err(_) => return None,
                            }
                        }
                    }
                }

                // Send packet to decoder
                let _ = avcodec::send_packet(self.codec_ctx.as_mut_ptr(), self.packet);
                crate::av_packet_unref(self.packet);
            }
        }

        /// Get sample format of the decoded audio.
        unsafe fn sample_format(&self) -> crate::AVSampleFormat {
            (*self.codec_ctx.as_ptr()).sample_fmt
        }

        /// Get sample rate of the decoded audio.
        unsafe fn sample_rate(&self) -> i32 {
            (*self.codec_ctx.as_ptr()).sample_rate
        }

        /// Get channel layout of the decoded audio.
        unsafe fn channel_layout(&self) -> &crate::AVChannelLayout {
            &(*self.codec_ctx.as_ptr()).ch_layout
        }
    }

    impl Drop for AudioDecoder {
        fn drop(&mut self) {
            // SAFETY: each pointer is null-checked before it is freed and is owned
            //         solely by this decoder; FFmpeg frees each and writes null back,
            //         so no double free is possible.
            unsafe {
                if !self.packet.is_null() {
                    crate::av_packet_free(&mut self.packet);
                }
                if !self.frame.is_null() {
                    crate::av_frame_free(&mut self.frame);
                }
                if !self.format_ctx.is_null() {
                    crate::avformat::close_input(&mut self.format_ctx);
                }
                // `codec_ctx` (owned CodecContext) frees itself via field drop
                // after this body; the codec and format contexts are independent
                // allocations, so the order is sound.
            }
        }
    }

    #[test]
    fn test_integration_decode_and_resample_mp3() {
        let audio_path = get_test_audio_path();
        if !audio_path.exists() {
            eprintln!("Skipping test: audio file not found at {:?}", audio_path);
            return;
        }

        unsafe {
            // Open and decode audio file
            let mut decoder = AudioDecoder::open(&audio_path).expect("Failed to open audio file");

            let in_sample_rate = decoder.sample_rate();
            let in_sample_fmt = decoder.sample_format();

            // Create resampler: convert to 48kHz stereo float
            let out_layout = channel_layout::stereo();
            let in_layout = decoder.channel_layout();

            let mut swr_ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLTP,
                48000,
                in_layout,
                in_sample_fmt,
                in_sample_rate,
            )
            .expect("Failed to create resampler context");

            init(swr_ctx).expect("Failed to initialize resampler");

            // Decode and resample a few frames
            let mut total_input_samples = 0;
            let mut total_output_samples = 0;
            let mut frames_processed = 0;

            while let Some(frame) = decoder.decode_frame() {
                let nb_samples = (*frame).nb_samples;
                if nb_samples <= 0 {
                    continue;
                }

                total_input_samples += nb_samples as usize;

                // Prepare output buffers (FLTP = planar)
                let out_count = estimate_output_samples(48000, in_sample_rate, nb_samples);
                let mut out_left: Vec<f32> = vec![0.0; out_count as usize];
                let mut out_right: Vec<f32> = vec![0.0; out_count as usize];
                let mut out_ptrs: [*mut u8; 2] =
                    [out_left.as_mut_ptr().cast(), out_right.as_mut_ptr().cast()];

                // Convert
                let result = convert(
                    swr_ctx,
                    out_ptrs.as_mut_ptr(),
                    out_count,
                    (*frame).extended_data.cast(),
                    nb_samples,
                );

                assert!(
                    result.is_ok(),
                    "Conversion failed at frame {}",
                    frames_processed
                );
                let samples_out = result.unwrap();
                total_output_samples += samples_out as usize;

                frames_processed += 1;

                // Process only first 100 frames for test speed
                if frames_processed >= 100 {
                    break;
                }
            }

            // Flush remaining samples
            let out_count = 4096;
            let mut out_left: Vec<f32> = vec![0.0; out_count as usize];
            let mut out_right: Vec<f32> = vec![0.0; out_count as usize];
            let mut out_ptrs: [*mut u8; 2] =
                [out_left.as_mut_ptr().cast(), out_right.as_mut_ptr().cast()];

            let flush_result = convert(
                swr_ctx,
                out_ptrs.as_mut_ptr(),
                out_count,
                std::ptr::null(),
                0,
            );
            if let Ok(flushed) = flush_result {
                total_output_samples += flushed as usize;
            }

            crate::swr_free(&mut swr_ctx);

            // Verify we processed some audio
            assert!(frames_processed > 0, "Should process at least one frame");
            assert!(total_input_samples > 0, "Should have input samples");
            assert!(total_output_samples > 0, "Should have output samples");

            println!(
                "Integration test: processed {} frames, {} input samples -> {} output samples",
                frames_processed, total_input_samples, total_output_samples
            );
        }
    }

    #[test]
    fn test_integration_format_conversion_chain() {
        let audio_path = get_test_audio_path();
        if !audio_path.exists() {
            eprintln!("Skipping test: audio file not found at {:?}", audio_path);
            return;
        }

        unsafe {
            let mut decoder = AudioDecoder::open(&audio_path).expect("Failed to open audio file");

            let in_sample_rate = decoder.sample_rate();
            let in_sample_fmt = decoder.sample_format();
            let in_layout = decoder.channel_layout();

            // Chain of conversions: input -> S16 -> FLT -> FLTP
            // First: input -> S16 (44.1kHz)
            let mid_layout = channel_layout::stereo();
            let mut swr_to_s16 = alloc_set_opts2(
                &mid_layout,
                sample_format::S16,
                44100,
                in_layout,
                in_sample_fmt,
                in_sample_rate,
            )
            .expect("Failed to create S16 resampler");
            init(swr_to_s16).expect("Failed to init S16 resampler");

            // Second: S16 -> FLTP (48kHz)
            let out_layout = channel_layout::stereo();
            let mut swr_to_fltp = alloc_set_opts2(
                &out_layout,
                sample_format::FLTP,
                48000,
                &mid_layout,
                sample_format::S16,
                44100,
            )
            .expect("Failed to create FLTP resampler");
            init(swr_to_fltp).expect("Failed to init FLTP resampler");

            // Process a few frames through the chain
            let mut frames_processed = 0;

            while let Some(frame) = decoder.decode_frame() {
                let nb_samples = (*frame).nb_samples;
                if nb_samples <= 0 {
                    continue;
                }

                // Stage 1: Convert to S16
                let mid_count = estimate_output_samples(44100, in_sample_rate, nb_samples);
                let mut mid_data: Vec<i16> = vec![0; mid_count as usize * 2];
                let mid_ptr = mid_data.as_mut_ptr() as *mut u8;
                let mut mid_ptrs: [*mut u8; 1] = [mid_ptr];

                let mid_result = convert(
                    swr_to_s16,
                    mid_ptrs.as_mut_ptr(),
                    mid_count,
                    (*frame).extended_data.cast(),
                    nb_samples,
                );
                assert!(mid_result.is_ok(), "S16 conversion failed");
                let mid_samples = mid_result.unwrap();

                // Stage 2: Convert S16 to FLTP
                let out_count = estimate_output_samples(48000, 44100, mid_samples);
                let mut out_left: Vec<f32> = vec![0.0; out_count as usize];
                let mut out_right: Vec<f32> = vec![0.0; out_count as usize];
                let mut out_ptrs: [*mut u8; 2] =
                    [out_left.as_mut_ptr().cast(), out_right.as_mut_ptr().cast()];

                let mid_in_ptr = mid_data.as_ptr() as *const u8;
                let mid_in_ptrs: [*const u8; 1] = [mid_in_ptr];

                let out_result = convert(
                    swr_to_fltp,
                    out_ptrs.as_mut_ptr(),
                    out_count,
                    mid_in_ptrs.as_ptr(),
                    mid_samples,
                );
                assert!(out_result.is_ok(), "FLTP conversion failed");

                frames_processed += 1;
                if frames_processed >= 50 {
                    break;
                }
            }

            crate::swr_free(&mut swr_to_s16);
            crate::swr_free(&mut swr_to_fltp);

            assert!(frames_processed > 0, "Should process frames through chain");
            println!(
                "Format chain test: processed {} frames through S16 -> FLTP pipeline",
                frames_processed
            );
        }
    }

    #[test]
    fn test_integration_mono_to_stereo_conversion() {
        let audio_path = get_test_audio_path();
        if !audio_path.exists() {
            eprintln!("Skipping test: audio file not found at {:?}", audio_path);
            return;
        }

        unsafe {
            let mut decoder = AudioDecoder::open(&audio_path).expect("Failed to open audio file");

            let in_sample_rate = decoder.sample_rate();
            let in_sample_fmt = decoder.sample_format();
            let in_layout = decoder.channel_layout();

            // First convert to mono
            let mono_layout = channel_layout::mono();
            let mut swr_to_mono = alloc_set_opts2(
                &mono_layout,
                sample_format::FLTP,
                in_sample_rate,
                in_layout,
                in_sample_fmt,
                in_sample_rate,
            )
            .expect("Failed to create mono resampler");
            init(swr_to_mono).expect("Failed to init mono resampler");

            // Then convert mono back to stereo
            let stereo_layout = channel_layout::stereo();
            let mut swr_to_stereo = alloc_set_opts2(
                &stereo_layout,
                sample_format::FLTP,
                48000,
                &mono_layout,
                sample_format::FLTP,
                in_sample_rate,
            )
            .expect("Failed to create stereo resampler");
            init(swr_to_stereo).expect("Failed to init stereo resampler");

            let mut frames_processed = 0;

            while let Some(frame) = decoder.decode_frame() {
                let nb_samples = (*frame).nb_samples;
                if nb_samples <= 0 {
                    continue;
                }

                // Convert to mono
                let mono_count = nb_samples + 256;
                let mut mono_data: Vec<f32> = vec![0.0; mono_count as usize];
                let mono_ptr = mono_data.as_mut_ptr() as *mut u8;
                let mut mono_ptrs: [*mut u8; 1] = [mono_ptr];

                let mono_result = convert(
                    swr_to_mono,
                    mono_ptrs.as_mut_ptr(),
                    mono_count,
                    (*frame).extended_data.cast(),
                    nb_samples,
                );
                assert!(mono_result.is_ok(), "Mono conversion failed");
                let mono_samples = mono_result.unwrap();

                // Convert mono to stereo
                let stereo_count = estimate_output_samples(48000, in_sample_rate, mono_samples);
                let mut stereo_left: Vec<f32> = vec![0.0; stereo_count as usize];
                let mut stereo_right: Vec<f32> = vec![0.0; stereo_count as usize];
                let mut stereo_ptrs: [*mut u8; 2] = [
                    stereo_left.as_mut_ptr().cast(),
                    stereo_right.as_mut_ptr().cast(),
                ];

                let mono_in_ptr = mono_data.as_ptr() as *const u8;
                let mono_in_ptrs: [*const u8; 1] = [mono_in_ptr];

                let stereo_result = convert(
                    swr_to_stereo,
                    stereo_ptrs.as_mut_ptr(),
                    stereo_count,
                    mono_in_ptrs.as_ptr(),
                    mono_samples,
                );
                assert!(stereo_result.is_ok(), "Stereo conversion failed");

                // Verify both channels have similar data (mono duplicated to stereo)
                let stereo_samples = stereo_result.unwrap() as usize;
                if stereo_samples > 10 {
                    // Check first few samples are similar between channels
                    for i in 0..10 {
                        let diff = (stereo_left[i] - stereo_right[i]).abs();
                        assert!(
                            diff < 0.001,
                            "Mono->stereo should have identical channels, diff={}",
                            diff
                        );
                    }
                }

                frames_processed += 1;
                if frames_processed >= 30 {
                    break;
                }
            }

            crate::swr_free(&mut swr_to_mono);
            crate::swr_free(&mut swr_to_stereo);

            assert!(frames_processed > 0, "Should process frames");
            println!(
                "Mono/stereo test: processed {} frames through mono->stereo pipeline",
                frames_processed
            );
        }
    }
}
