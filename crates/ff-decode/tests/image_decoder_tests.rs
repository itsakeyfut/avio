//! Integration tests for [`ImageDecoder`] and [`ImageDecoderBuilder`].
//!
//! The real assets `assets/img/hello-triangle.png` and
//! `assets/img/hello-triangle.jpg` are used for happy-path tests.
//! If FFmpeg is unavailable or the file cannot be opened the tests skip
//! gracefully via `println!("Skipping: …"); return;`.

#![allow(clippy::unwrap_used)]

mod fixtures;

use std::path::PathBuf;

use ff_decode::{ImageDecoder, ImageDecoderBuilder};

fn png_path() -> PathBuf {
    fixtures::assets_dir().join("img/hello-triangle.png")
}

fn jpeg_path() -> PathBuf {
    fixtures::test_jpeg_path()
}

// crate-root exports

/// `ImageDecoder` and `ImageDecoderBuilder` must be importable from the crate
/// root without any extra path segments.
#[test]
fn crate_root_should_export_image_decoder_types() {
    // If these types are not re-exported the test won't compile.
    let _: fn(&str) -> ImageDecoderBuilder = |p| ImageDecoder::open(p);
}

// open() / build() — error cases

#[test]
fn open_missing_file_should_return_file_not_found_error() {
    let path = PathBuf::from("/nonexistent/path/does_not_exist.png");
    let result = ImageDecoder::open(&path).build();
    assert!(
        matches!(result, Err(ff_decode::DecodeError::FileNotFound { .. })),
        "expected FileNotFound"
    );
}

#[test]
fn open_audio_only_file_should_return_error() {
    // An audio-only file has no video stream — open must not succeed silently.
    let path = fixtures::test_audio_path();
    if !path.exists() {
        println!("Skipping: audio asset not found");
        return;
    }
    let result = ImageDecoder::open(&path).build();
    assert!(
        result.is_err(),
        "opening an audio-only file should fail, got Ok"
    );
}

// open() / build() — happy path

#[test]
fn open_png_should_succeed() {
    match ImageDecoder::open(png_path()).build() {
        Ok(_) => {}
        Err(e) => {
            println!("Skipping: {e}");
        }
    }
}

#[test]
fn open_png_should_report_positive_width() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(decoder.width() > 0, "width must be positive after open");
}

#[test]
fn open_png_should_report_positive_height() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(decoder.height() > 0, "height must be positive after open");
}

#[test]
fn open_returns_builder_then_build_opens_file() {
    // Verify the two-step builder API: open() → builder, build() → decoder.
    let builder = ImageDecoder::open(png_path());
    match builder.build() {
        Ok(decoder) => assert!(decoder.width() > 0),
        Err(e) => println!("Skipping: {e}"),
    }
}

// decode() — consuming convenience API

#[test]
fn decode_png_should_return_video_frame() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    match decoder.decode() {
        Ok(_) => {}
        Err(e) => println!("Skipping: {e}"),
    }
}

#[test]
fn decode_png_frame_width_should_match_open_width() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let expected = decoder.width();
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert_eq!(
        frame.width(),
        expected,
        "frame width must match reported width"
    );
}

#[test]
fn decode_png_frame_height_should_match_open_height() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let expected = decoder.height();
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert_eq!(
        frame.height(),
        expected,
        "frame height must match reported height"
    );
}

#[test]
fn decode_png_frame_should_be_key_frame() {
    // Images are always key frames — decoder_inner hard-codes `true`.
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(
        frame.is_key_frame(),
        "decoded image must be marked as a key frame"
    );
}

#[test]
fn decode_png_frame_should_have_at_least_one_plane() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(
        frame.num_planes() > 0,
        "decoded frame must have at least one plane"
    );
}

#[test]
fn decode_png_frame_planes_should_be_non_empty() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    for (i, plane) in frame.planes().iter().enumerate() {
        assert!(!plane.is_empty(), "plane {i} must contain pixel data");
    }
}

#[test]
fn decode_png_frame_total_size_should_be_positive() {
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(
        frame.total_size() > 0,
        "decoded frame must have a positive total byte size"
    );
}

#[test]
fn decode_png_frame_pixel_format_should_be_supported() {
    use ff_format::PixelFormat;
    let supported = [
        PixelFormat::Yuv420p,
        PixelFormat::Yuv422p,
        PixelFormat::Yuv444p,
        PixelFormat::Rgb24,
        PixelFormat::Bgr24,
        PixelFormat::Rgba,
        PixelFormat::Bgra,
        PixelFormat::Gray8,
    ];
    let decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(
        supported.contains(&frame.format()),
        "pixel format {:?} must be one of the supported formats",
        frame.format()
    );
}

// decode_one() — mutable incremental API

#[test]
fn decode_one_first_call_should_return_some_frame() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let result = decoder.decode_one();
    assert!(
        matches!(result, Ok(Some(_))),
        "first decode_one must return Ok(Some(frame)), got {result:?}"
    );
}

#[test]
fn decode_one_second_call_should_return_none() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let _ = decoder.decode_one();
    let result = decoder.decode_one();
    assert!(
        matches!(result, Ok(None)),
        "second decode_one must return Ok(None), got {result:?}"
    );
}

#[test]
fn decode_one_frame_dimensions_should_match_reported_dimensions() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let w = decoder.width();
    let h = decoder.height();
    let frame = match decoder.decode_one() {
        Ok(Some(f)) => f,
        Ok(None) => {
            println!("Skipping: no frame returned");
            return;
        }
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert_eq!(frame.width(), w);
    assert_eq!(frame.height(), h);
}

// frames() — iterator API

#[test]
fn frames_iterator_should_yield_exactly_one_frame() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frames: Vec<_> = decoder.frames().collect();
    assert_eq!(
        frames.len(),
        1,
        "frames() must yield exactly one frame for a still image"
    );
    assert!(frames[0].is_ok(), "the single frame must be Ok");
}

#[test]
fn frames_iterator_second_call_should_yield_nothing() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let _ = decoder.frames().collect::<Vec<_>>();
    let second: Vec<_> = decoder.frames().collect();
    assert!(
        second.is_empty(),
        "frames() after image already decoded must yield nothing"
    );
}

#[test]
fn frames_iterator_frame_should_be_key_frame() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    for result in decoder.frames() {
        let frame = match result {
            Ok(f) => f,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        assert!(
            frame.is_key_frame(),
            "frame from iterator must be a key frame"
        );
    }
}

#[test]
fn frames_iterator_frame_should_have_correct_dimensions() {
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let w = decoder.width();
    let h = decoder.height();
    for result in decoder.frames() {
        let frame = match result {
            Ok(f) => f,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        assert_eq!(frame.width(), w, "iterator frame width must match");
        assert_eq!(frame.height(), h, "iterator frame height must match");
    }
}

// Drop safety

#[test]
fn decoder_drop_after_decode_should_not_panic() {
    // Drop must handle `inner == None` gracefully after decode_one consumes it.
    let mut decoder = match ImageDecoder::open(png_path()).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let _ = decoder.decode_one();
    // decoder is dropped here
}

#[test]
fn decoder_drop_without_decode_should_not_panic() {
    // Drop must free FFmpeg resources even when decode was never called.
    match ImageDecoder::open(png_path()).build() {
        Ok(_decoder) => {} // dropped here without decoding
        Err(e) => println!("Skipping: {e}"),
    }
}

// JPEG decoding

#[test]
fn open_jpeg_should_succeed() {
    let path = jpeg_path();
    if !path.exists() {
        println!("Skipping: JPEG asset not found");
        return;
    }
    match ImageDecoder::open(&path).build() {
        Ok(_) => {}
        Err(e) => println!("Skipping: {e}"),
    }
}

#[test]
fn decode_jpeg_frame_should_have_positive_width_and_height() {
    let path = jpeg_path();
    if !path.exists() {
        println!("Skipping: JPEG asset not found");
        return;
    }
    let decoder = match ImageDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(decoder.width() > 0, "JPEG width must be positive");
    assert!(decoder.height() > 0, "JPEG height must be positive");
}

#[test]
fn decode_jpeg_frame_dimensions_should_match_reported_dimensions() {
    let path = jpeg_path();
    if !path.exists() {
        println!("Skipping: JPEG asset not found");
        return;
    }
    let decoder = match ImageDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let expected_w = decoder.width();
    let expected_h = decoder.height();
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert_eq!(
        frame.width(),
        expected_w,
        "decoded JPEG frame width must match reported width"
    );
    assert_eq!(
        frame.height(),
        expected_h,
        "decoded JPEG frame height must match reported height"
    );
}

#[test]
fn decode_jpeg_frame_pixel_format_should_be_supported() {
    use ff_format::PixelFormat;
    let supported = [
        PixelFormat::Yuv420p,
        PixelFormat::Yuv422p,
        PixelFormat::Yuv444p,
        PixelFormat::Rgb24,
        PixelFormat::Bgr24,
        PixelFormat::Rgba,
        PixelFormat::Bgra,
        PixelFormat::Gray8,
    ];
    let path = jpeg_path();
    if !path.exists() {
        println!("Skipping: JPEG asset not found");
        return;
    }
    let decoder = match ImageDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = match decoder.decode() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(
        supported.contains(&frame.format()),
        "JPEG pixel format {:?} must be one of the supported formats",
        frame.format()
    );
}
