//! Compile-time integration test: verifies the avio facade compiles correctly
//! with each feature combination.
//!
//! Each `#[test]` function contains only name-resolution and construction
//! expressions; they always pass when they compile.  Running with specific
//! feature sets (or `--all-features`) activates the corresponding sections:
//!
//! ```text
//! cargo test -p avio                        # default: probe + decode + encode
//! cargo test -p avio --features filter      # adds filter
//! cargo test -p avio --features pipeline    # adds pipeline (implies filter)
//! cargo test -p avio --features stream      # adds stream  (implies pipeline)
//! cargo test -p avio --all-features         # all combinations at once
//! ```

// Always-on types (ff-format, no feature gate)

#[test]
fn format_types_should_be_accessible_without_any_feature() {
    let _: avio::VideoCodec = avio::VideoCodec::default();
    let _: avio::AudioCodec = avio::AudioCodec::default();
    let _: avio::PixelFormat = avio::PixelFormat::default();
    let _: avio::SampleFormat = avio::SampleFormat::default();
    let _: avio::ChannelLayout = avio::ChannelLayout::default();
    let _: avio::Rational = avio::Rational::default();
    let _: avio::Timestamp = avio::Timestamp::default();
    let _: avio::MediaInfo = avio::MediaInfo::default();
}

// probe feature

#[cfg(feature = "probe")]
#[test]
fn probe_feature_should_expose_probe_error_and_open() {
    // open() is a function — a missing file returns ProbeError
    let result = avio::open("/no/such/file.mp4");
    assert!(matches!(result, Err(avio::ProbeError::FileNotFound { .. })));
}

// decode feature

#[cfg(feature = "decode")]
#[test]
fn decode_feature_should_expose_decode_error_and_decoders() {
    let _: avio::DecodeError = avio::DecodeError::decoding_failed("test");
    let _: avio::SeekMode = avio::SeekMode::Keyframe;
    let _: avio::HardwareAccel = avio::HardwareAccel::None;
    // VecPool::new() returns Arc<VecPool>
    let pool = avio::VecPool::new(4);
    assert_eq!(pool.capacity(), 4);
}

// encode feature

#[cfg(feature = "encode")]
#[test]
fn encode_feature_should_expose_encode_error_and_bitrate_mode() {
    let _: avio::EncodeError = avio::EncodeError::Cancelled;
    let _: avio::BitrateMode = avio::BitrateMode::Cbr(2_000_000);
    let _: avio::BitrateMode = avio::BitrateMode::Crf(28);
}

// filter feature

#[cfg(feature = "filter")]
#[test]
fn filter_feature_should_expose_filter_error_and_graph_builder() {
    let _: avio::FilterError = avio::FilterError::BuildFailed;
    let _builder: avio::FilterGraphBuilder = avio::FilterGraphBuilder::new();
    let _: avio::ToneMap = avio::ToneMap::Hable;
    let _: avio::HwAccel = avio::HwAccel::Cuda;
}

// pipeline feature

#[cfg(feature = "pipeline")]
#[test]
fn pipeline_feature_should_expose_pipeline_error_and_builder() {
    let _: avio::PipelineError = avio::PipelineError::NoInput;
    let _builder: avio::PipelineBuilder = avio::Pipeline::builder();
    let _t: avio::ThumbnailPipeline = avio::ThumbnailPipeline::new("/no/such/file.mp4");
    let _cb: avio::ProgressCallback = Box::new(|_: &avio::Progress| true);
}

// stream feature

#[cfg(feature = "stream")]
#[test]
fn stream_feature_should_expose_stream_error_and_output_builders() {
    let _: avio::StreamError = avio::StreamError::InvalidConfig {
        reason: "test".into(),
    };
    let _hls: avio::HlsOutput = avio::HlsOutput::new("/tmp/hls");
    let _dash: avio::DashOutput = avio::DashOutput::new("/tmp/dash");
    let _ladder: avio::AbrLadder = avio::AbrLadder::new("/no/such/file.mp4");
    let _r: avio::Rendition = avio::Rendition {
        width: 1280,
        height: 720,
        bitrate: 3_000_000,
    };
}

// all-features combination

#[cfg(all(feature = "filter", feature = "pipeline", feature = "stream"))]
#[test]
fn all_features_should_expose_symbols_without_conflicts() {
    // EncodeProgress (from encode) and Progress (from pipeline) are distinct
    // types that coexist without collision.
    assert!(std::mem::size_of::<avio::EncodeProgress>() > 0);
    let p = avio::Progress {
        frames_processed: 5,
        total_frames: Some(10),
        elapsed: std::time::Duration::from_secs(1),
    };
    assert_eq!(p.percent(), Some(50.0));

    // All error types are accessible simultaneously.
    let _: avio::FilterError = avio::FilterError::BuildFailed;
    let _: avio::PipelineError = avio::PipelineError::Cancelled;
    let _: avio::StreamError = avio::StreamError::InvalidConfig {
        reason: "all-features test".into(),
    };
}
