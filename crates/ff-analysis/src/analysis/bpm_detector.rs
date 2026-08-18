//! BPM (tempo) detection result type.
//!
//! This module defines [`BpmResult`], the public output type for BPM detection.
//! The detector itself (spectral flux + autocorrelation) is added separately.

use std::time::Duration;

/// Result of BPM (beats-per-minute) detection over an audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct BpmResult {
    /// Detected tempo in beats per minute.
    pub bpm: f64,
    /// Timestamp of each detected beat, measured from the start of the file.
    pub beats: Vec<Duration>,
    /// Detection confidence in `[0.0, 1.0]`; values below `0.4` indicate an
    /// ambiguous rhythm.
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_result_should_hold_fields() {
        let result = BpmResult {
            bpm: 128.0,
            beats: vec![Duration::from_millis(0), Duration::from_millis(469)],
            confidence: 0.9,
        };
        assert!((result.bpm - 128.0).abs() < f64::EPSILON);
        assert_eq!(result.beats.len(), 2);
        assert!(result.confidence > 0.4);
    }
}
