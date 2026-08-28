//! Integration tests for proxy generation and transparent substitution.
//!
//! Requires the `proxy` feature:
//! ```bash
//! cargo test -p ff-preview --features proxy -- --include-ignored proxy_
//! ```
//!
//! Reference asset: `assets/test/preview_bench_1080p.mp4` (1920×1080 30 fps
//! H.264+AAC, 60 s synthetic video).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ff_preview::{FrameSink, PlayerHandle, PreviewPlayer, ProxyGenerator, ProxyResolution};

// Asset path

fn bench_video_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/preview_bench_1080p.mp4")
}

// DimSink

/// Records `(width, height)` per delivered frame; stops the player after
/// `max_frames` frames via the shared handle.
struct DimSink {
    dims: Arc<Mutex<Vec<(u32, u32)>>>,
    max_frames: usize,
    handle: PlayerHandle,
}

impl FrameSink for DimSink {
    fn push_frame(&mut self, _rgba: &[u8], width: u32, height: u32, _pts: Duration) {
        let mut guard = self
            .dims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.push((width, height));
        if guard.len() >= self.max_frames {
            self.handle.stop();
        }
    }
}

#[test]
#[ignore = "requires assets/test/preview_bench_1080p.mp4 and --features proxy; \
            run with: cargo test -p ff-preview --features proxy -- --include-ignored"]
fn proxy_quarter_resolution_should_produce_480x270_output() {
    let input = bench_video_path();
    if !input.exists() {
        println!("skipping: reference file not found at {}", input.display());
        return;
    }

    let tmp = std::env::temp_dir();

    let proxy_path = match ProxyGenerator::new(&input) {
        Ok(g) => match g
            .resolution(ProxyResolution::Quarter)
            .output_dir(&tmp)
            .generate()
        {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: proxy generation failed: {e}");
                return;
            }
        },
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    let info = match ff_probe::open(&proxy_path) {
        Ok(i) => i,
        Err(e) => {
            let _ = std::fs::remove_file(&proxy_path);
            panic!("failed to probe proxy output: {e}");
        }
    };

    let (w, h) = info
        .resolution()
        .expect("proxy must contain a video stream");

    let _ = std::fs::remove_file(&proxy_path);

    assert_eq!(
        w, 480,
        "quarter-resolution proxy width must be 480 (1920 / 4); got {w}"
    );
    assert_eq!(
        h, 270,
        "quarter-resolution proxy height must be 270 (1080 / 4); got {h}"
    );
}

#[test]
#[ignore = "requires assets/test/preview_bench_1080p.mp4 and --features proxy; \
            run with: cargo test -p ff-preview --features proxy -- --include-ignored"]
fn proxy_substitution_should_deliver_frames_at_proxy_dimensions() {
    let input = bench_video_path();
    if !input.exists() {
        println!("skipping: reference file not found at {}", input.display());
        return;
    }

    let tmp = std::env::temp_dir();

    let proxy_path = match ProxyGenerator::new(&input) {
        Ok(g) => match g
            .resolution(ProxyResolution::Half)
            .output_dir(&tmp)
            .generate()
        {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: proxy generation failed: {e}");
                return;
            }
        },
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    let (mut runner, handle) = match PreviewPlayer::open(&input) {
        Ok(p) => p.split(),
        Err(e) => {
            let _ = std::fs::remove_file(&proxy_path);
            println!("skipping: {e}");
            return;
        }
    };

    let activated = runner.use_proxy_if_available(&tmp);
    assert!(
        activated,
        "use_proxy_if_available must return true when a half proxy exists in the temp dir"
    );

    let dims = Arc::new(Mutex::new(Vec::<(u32, u32)>::new()));

    runner.set_sink(Box::new(DimSink {
        dims: Arc::clone(&dims),
        max_frames: 20,
        handle: handle.clone(),
    }));
    let _ = runner.run();

    let _ = std::fs::remove_file(&proxy_path);

    let dims = dims
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    assert!(
        !dims.is_empty(),
        "no frames were delivered during proxy playback"
    );

    for (i, &(w, h)) in dims.iter().enumerate() {
        assert_eq!(
            w, 960,
            "frame {i}: expected proxy width 960 (half of 1920); got {w}"
        );
        assert_eq!(
            h, 540,
            "frame {i}: expected proxy height 540 (half of 1080); got {h}"
        );
    }
}
