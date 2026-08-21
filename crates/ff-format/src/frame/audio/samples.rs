//! Utility and PCM conversion methods for [`AudioFrame`].

use crate::SampleFormat;

use super::AudioFrame;

impl AudioFrame {
    // ==========================================================================
    // Utility Methods
    // ==========================================================================

    /// Returns the total size in bytes of all sample data.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.total_size(), 1024 * 2 * 4);
    /// ```
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.planes.iter().map(Vec::len).sum()
    }

    /// Returns the size in bytes of a single sample (one channel).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.bytes_per_sample(), 4);
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::I16).unwrap();
    /// assert_eq!(frame.bytes_per_sample(), 2);
    /// ```
    #[must_use]
    #[inline]
    pub fn bytes_per_sample(&self) -> usize {
        self.format.bytes_per_sample()
    }

    /// Returns the total number of samples across all channels (`samples * channels`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.sample_count(), 2048);
    /// ```
    #[must_use]
    #[inline]
    pub fn sample_count(&self) -> usize {
        self.samples * self.channels as usize
    }

    // ==========================================================================
    // PCM Conversion
    // ==========================================================================

    /// Converts the audio frame to interleaved 32-bit float PCM.
    ///
    /// All [`SampleFormat`] variants are supported. Planar formats are transposed
    /// to interleaved layout (L0 R0 L1 R1 ...). Returns an empty `Vec` for
    /// [`SampleFormat::Other`].
    ///
    /// # Scaling
    ///
    /// | Source format | Normalization |
    /// |---|---|
    /// | U8  | `(sample − 128) / 128.0` → `[−1.0, 1.0]` |
    /// | I16 | `sample / 32767.0` → `[−1.0, 1.0]` |
    /// | I32 | `sample / 2147483647.0` → `[−1.0, 1.0]` |
    /// | F32 | identity |
    /// | F64 | narrowed to f32 (`as f32`) |
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(4, 2, 48000, SampleFormat::F32p).unwrap();
    /// let pcm = frame.to_f32_interleaved();
    /// assert_eq!(pcm.len(), 8); // 4 samples × 2 channels
    /// ```
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::too_many_lines
    )]
    pub fn to_f32_interleaved(&self) -> Vec<f32> {
        let total = self.sample_count();
        if total == 0 {
            return Vec::new();
        }

        match self.format {
            SampleFormat::F32 => self.as_f32().map(<[f32]>::to_vec).unwrap_or_default(),
            SampleFormat::F32p => {
                let mut out = vec![0f32; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(plane) = self.channel_as_f32(ch) {
                        for (i, &s) in plane.iter().enumerate() {
                            out[i * ch_count + ch] = s;
                        }
                    }
                }
                out
            }
            SampleFormat::F64 => {
                let bytes = self.data();
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| {
                        f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
                    })
                    .collect()
            }
            SampleFormat::F64p => {
                let mut out = vec![0f32; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, b) in bytes.as_chunks::<8>().0.iter().enumerate() {
                            out[i * ch_count + ch] = f64::from_le_bytes([
                                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                            ]) as f32;
                        }
                    }
                }
                out
            }
            SampleFormat::I16 => {
                let bytes = self.data();
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / f32::from(i16::MAX))
                    .collect()
            }
            SampleFormat::I16p => {
                let mut out = vec![0f32; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, b) in bytes.as_chunks::<2>().0.iter().enumerate() {
                            out[i * ch_count + ch] =
                                f32::from(i16::from_le_bytes([b[0], b[1]])) / f32::from(i16::MAX);
                        }
                    }
                }
                out
            }
            SampleFormat::I32 => {
                let bytes = self.data();
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / i32::MAX as f32)
                    .collect()
            }
            SampleFormat::I32p => {
                let mut out = vec![0f32; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, b) in bytes.as_chunks::<4>().0.iter().enumerate() {
                            out[i * ch_count + ch] = i32::from_le_bytes([b[0], b[1], b[2], b[3]])
                                as f32
                                / i32::MAX as f32;
                        }
                    }
                }
                out
            }
            SampleFormat::U8 => {
                let bytes = self.data();
                bytes
                    .iter()
                    .map(|&b| (f32::from(b) - 128.0) / 128.0)
                    .collect()
            }
            SampleFormat::U8p => {
                let mut out = vec![0f32; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, &b) in bytes.iter().enumerate() {
                            out[i * ch_count + ch] = (f32::from(b) - 128.0) / 128.0;
                        }
                    }
                }
                out
            }
            SampleFormat::Other(_) => Vec::new(),
        }
    }

    /// Converts the audio frame to interleaved 16-bit signed integer PCM.
    ///
    /// Suitable for use with `rodio::buffer::SamplesBuffer<i16>`. All
    /// [`SampleFormat`] variants are supported. Returns an empty `Vec` for
    /// [`SampleFormat::Other`].
    ///
    /// # Scaling
    ///
    /// | Source format | Conversion |
    /// |---|---|
    /// | I16 | identity |
    /// | I32 | `sample >> 16` (high 16 bits) |
    /// | U8  | `(sample − 128) << 8` |
    /// | F32/F64 | `clamp(−1, 1) × 32767`, truncated |
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(4, 2, 48000, SampleFormat::I16p).unwrap();
    /// let pcm = frame.to_i16_interleaved();
    /// assert_eq!(pcm.len(), 8);
    /// ```
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)] // float→i16 and i32→i16 are intentional truncations
    pub fn to_i16_interleaved(&self) -> Vec<i16> {
        let total = self.sample_count();
        if total == 0 {
            return Vec::new();
        }

        match self.format {
            SampleFormat::I16 => self.as_i16().map(<[i16]>::to_vec).unwrap_or_default(),
            SampleFormat::I16p => {
                let mut out = vec![0i16; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(plane) = self.channel_as_i16(ch) {
                        for (i, &s) in plane.iter().enumerate() {
                            out[i * ch_count + ch] = s;
                        }
                    }
                }
                out
            }
            SampleFormat::F32 => {
                let bytes = self.data();
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| {
                        let s = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                        (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
                    })
                    .collect()
            }
            SampleFormat::F32p => {
                let mut out = vec![0i16; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, b) in bytes.as_chunks::<4>().0.iter().enumerate() {
                            let s = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                            out[i * ch_count + ch] =
                                (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                        }
                    }
                }
                out
            }
            SampleFormat::F64 => {
                let bytes = self.data();
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| {
                        let s =
                            f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
                        (s.clamp(-1.0, 1.0) * f64::from(i16::MAX)) as i16
                    })
                    .collect()
            }
            SampleFormat::F64p => {
                let mut out = vec![0i16; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, b) in bytes.as_chunks::<8>().0.iter().enumerate() {
                            let s = f64::from_le_bytes([
                                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                            ]);
                            out[i * ch_count + ch] =
                                (s.clamp(-1.0, 1.0) * f64::from(i16::MAX)) as i16;
                        }
                    }
                }
                out
            }
            SampleFormat::I32 => {
                let bytes = self.data();
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 16) as i16)
                    .collect()
            }
            SampleFormat::I32p => {
                let mut out = vec![0i16; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, b) in bytes.as_chunks::<4>().0.iter().enumerate() {
                            out[i * ch_count + ch] =
                                (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 16) as i16;
                        }
                    }
                }
                out
            }
            SampleFormat::U8 => {
                let bytes = self.data();
                bytes.iter().map(|&b| (i16::from(b) - 128) << 8).collect()
            }
            SampleFormat::U8p => {
                let mut out = vec![0i16; total];
                let ch_count = self.channels as usize;
                for ch in 0..ch_count {
                    if let Some(bytes) = self.channel(ch) {
                        for (i, &b) in bytes.iter().enumerate() {
                            out[i * ch_count + ch] = (i16::from(b) - 128) << 8;
                        }
                    }
                }
                out
            }
            SampleFormat::Other(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Rational, Timestamp};

    // ==========================================================================
    // Utility Tests
    // ==========================================================================

    #[test]
    fn total_size_packed_should_equal_samples_times_channels_times_bytes() {
        // Packed stereo F32: 1024 samples * 2 channels * 4 bytes
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        assert_eq!(frame.total_size(), 1024 * 2 * 4);

        // Planar stereo F32p: 2 planes * 1024 samples * 4 bytes
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32p).unwrap();
        assert_eq!(frame.total_size(), 1024 * 4 * 2);
    }

    #[test]
    fn bytes_per_sample_should_match_format_width() {
        assert_eq!(
            AudioFrame::empty(1024, 2, 48000, SampleFormat::U8)
                .unwrap()
                .bytes_per_sample(),
            1
        );
        assert_eq!(
            AudioFrame::empty(1024, 2, 48000, SampleFormat::I16)
                .unwrap()
                .bytes_per_sample(),
            2
        );
        assert_eq!(
            AudioFrame::empty(1024, 2, 48000, SampleFormat::F32)
                .unwrap()
                .bytes_per_sample(),
            4
        );
        assert_eq!(
            AudioFrame::empty(1024, 2, 48000, SampleFormat::F64)
                .unwrap()
                .bytes_per_sample(),
            8
        );
    }

    #[test]
    fn sample_count_should_return_samples_times_channels() {
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        assert_eq!(frame.sample_count(), 2048);

        let mono = AudioFrame::empty(512, 1, 44100, SampleFormat::I16).unwrap();
        assert_eq!(mono.sample_count(), 512);
    }

    // ==========================================================================
    // PCM Conversion Tests
    // ==========================================================================

    #[test]
    fn to_f32_interleaved_f32p_should_transpose_to_interleaved() {
        // Stereo F32p: L=[1.0, 2.0], R=[3.0, 4.0]
        // Expected interleaved: [1.0, 3.0, 2.0, 4.0]
        let left: Vec<u8> = [1.0f32, 2.0f32]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let right: Vec<u8> = [3.0f32, 4.0f32]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();

        let frame = AudioFrame::new(
            vec![left, right],
            2,
            2,
            48000,
            SampleFormat::F32p,
            Timestamp::default(),
        )
        .unwrap();

        let pcm = frame.to_f32_interleaved();
        assert_eq!(pcm.len(), 4);
        assert!((pcm[0] - 1.0).abs() < f32::EPSILON); // L0
        assert!((pcm[1] - 3.0).abs() < f32::EPSILON); // R0
        assert!((pcm[2] - 2.0).abs() < f32::EPSILON); // L1
        assert!((pcm[3] - 4.0).abs() < f32::EPSILON); // R1
    }

    #[test]
    fn to_f32_interleaved_i16p_should_scale_to_minus_one_to_one() {
        // i16::MAX → ~1.0,  i16::MIN → ~-1.0,  0 → 0.0
        let make_i16_bytes = |v: i16| v.to_le_bytes().to_vec();

        let left: Vec<u8> = [i16::MAX, 0i16]
            .iter()
            .flat_map(|&v| make_i16_bytes(v))
            .collect();
        let right: Vec<u8> = [i16::MIN, 0i16]
            .iter()
            .flat_map(|&v| make_i16_bytes(v))
            .collect();

        let frame = AudioFrame::new(
            vec![left, right],
            2,
            2,
            48000,
            SampleFormat::I16p,
            Timestamp::default(),
        )
        .unwrap();

        let pcm = frame.to_f32_interleaved();
        // i16 is asymmetric: MIN=-32768, MAX=32767, so MIN/MAX ≈ -1.00003
        // Values should be very close to [-1.0, 1.0]
        for &s in &pcm {
            assert!(s >= -1.001 && s <= 1.001, "out of range: {s}");
        }
        assert!((pcm[0] - 1.0).abs() < 0.0001); // i16::MAX → ~1.0
        assert!((pcm[1] - (-1.0)).abs() < 0.001); // i16::MIN → ~-1.00003
    }

    #[test]
    fn to_f32_interleaved_unknown_should_return_empty() {
        // AudioFrame::new() (unlike empty()) accepts Other(_): Other is treated
        // as packed so expected_planes = 1.
        let frame = AudioFrame::new(
            vec![vec![0u8; 16]],
            4,
            1,
            48000,
            SampleFormat::Other(999),
            Timestamp::default(),
        )
        .unwrap();
        assert_eq!(frame.to_f32_interleaved(), Vec::<f32>::new());
    }

    #[test]
    fn to_i16_interleaved_i16p_should_transpose_to_interleaved() {
        let left: Vec<u8> = [100i16, 200i16]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let right: Vec<u8> = [300i16, 400i16]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();

        let frame = AudioFrame::new(
            vec![left, right],
            2,
            2,
            48000,
            SampleFormat::I16p,
            Timestamp::default(),
        )
        .unwrap();

        let pcm = frame.to_i16_interleaved();
        assert_eq!(pcm, vec![100, 300, 200, 400]);
    }

    #[test]
    fn to_i16_interleaved_f32_should_scale_and_clamp() {
        // 1.0 → i16::MAX, -1.0 → -i16::MAX, 2.0 → clamped to i16::MAX
        let samples: &[f32] = &[1.0, -1.0, 2.0, -2.0];
        let bytes: Vec<u8> = samples.iter().flat_map(|f| f.to_le_bytes()).collect();

        let frame = AudioFrame::new(
            vec![bytes],
            4,
            1,
            48000,
            SampleFormat::F32,
            Timestamp::default(),
        )
        .unwrap();

        let pcm = frame.to_i16_interleaved();
        assert_eq!(pcm.len(), 4);
        assert_eq!(pcm[0], i16::MAX);
        assert_eq!(pcm[1], -i16::MAX);
        // Clamped values
        assert_eq!(pcm[2], i16::MAX);
        assert_eq!(pcm[3], -i16::MAX);
    }

    #[test]
    fn audio_frame_clone_should_have_identical_data() {
        let samples = 512;
        let channels = 2u32;
        let bytes_per_sample = 4; // F32
        let plane_data = vec![7u8; samples * bytes_per_sample];
        let ts = Timestamp::new(500, Rational::new(1, 1000));

        let original = AudioFrame::new(
            vec![plane_data.clone()],
            samples,
            channels,
            44100,
            SampleFormat::F32,
            ts,
        )
        .unwrap();

        let clone = original.clone();

        assert_eq!(clone.samples(), original.samples());
        assert_eq!(clone.channels(), original.channels());
        assert_eq!(clone.sample_rate(), original.sample_rate());
        assert_eq!(clone.format(), original.format());
        assert_eq!(clone.timestamp(), original.timestamp());
        assert_eq!(clone.num_planes(), original.num_planes());
        assert_eq!(clone.plane(0), original.plane(0));
    }
}
