//! Build script for ff-sys crate.
//!
//! This script handles:
//! - Platform-specific `FFmpeg` library detection
//! - bindgen code generation for FFI bindings
//!
//! # Windows (VCPKG)
//!
//! Requires FFmpeg installed via VCPKG:
//! ```bash
//! vcpkg install ffmpeg:x64-windows
//! ```
//!
//! Environment variables:
//! - `VCPKG_ROOT`: Path to VCPKG installation (default: `C:\vcpkg`)
//! - `LIBCLANG_PATH`: Path to LLVM/clang bin directory (for bindgen)
//!
//! # macOS (Homebrew)
//!
//! Requires FFmpeg installed via Homebrew:
//! ```bash
//! brew install ffmpeg
//! ```
//!
//! Environment variables:
//! - `HOMEBREW_PREFIX`: Path to Homebrew installation (auto-detected if not set)
//!   - Apple Silicon: `/opt/homebrew`
//!   - Intel: `/usr/local`
//!
//! # Linux (pkg-config)
//!
//! Requires FFmpeg development packages:
//! - Ubuntu/Debian: `apt install libavcodec-dev libavformat-dev libswscale-dev libswresample-dev`
//! - Fedora: `dnf install ffmpeg-devel`
//! - Arch: `pacman -S ffmpeg`

// Build scripts are allowed to use panic/expect for fatal configuration errors
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    // Declare the custom cfgs this build script may emit. Without this, the
    // `unexpected_cfgs` lint fires on the hand-written wrapper modules now that
    // the crate no longer carries a blanket `#![allow(warnings)]`.
    println!("cargo::rustc-check-cfg=cfg(docsrs)");
    println!("cargo::rustc-check-cfg=cfg(ffmpeg8)");
    println!("cargo::rustc-check-cfg=cfg(ffmpeg_buffersrc_flag_u32)");

    // docs.rs does not have FFmpeg installed.  Emit empty bindings so that the
    // crate compiles; docsrs_stubs.rs (included by lib.rs) provides stub types.
    if env::var("DOCS_RS").is_ok() {
        let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
        std::fs::write(out_path.join("bindings.rs"), b"")
            .expect("Couldn't write stub bindings for docs.rs");
        println!("cargo:rustc-cfg=docsrs");
        return;
    }

    // Detect target platform
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Configure platform-specific linking and collect include paths
    let include_paths = match target_os.as_str() {
        "windows" => configure_windows(),
        "macos" => configure_macos(),
        "linux" => configure_linux(),
        other => panic!("Unsupported platform: {other}"),
    };

    // Emit cfg flags based on detected FFmpeg/library API variants
    emit_api_cfg_flags(&include_paths);

    // Generate FFI bindings
    generate_bindings(&include_paths);
}

/// FFmpeg libraries required for linking
const FFMPEG_LIBS: &[&str] = &[
    "avformat",
    "avcodec",
    "avutil",
    "swscale",
    "swresample",
    "avfilter",
];

/// Configure `FFmpeg` linking for Windows via VCPKG.
///
/// Returns include paths for bindgen.
fn configure_windows() -> Vec<String> {
    // Rebuild if environment variables change
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=LIBCLANG_PATH");

    let vcpkg_root = env::var("VCPKG_ROOT").unwrap_or_else(|_| "C:\\vcpkg".to_string());
    let installed_path = Path::new(&vcpkg_root).join("installed").join("x64-windows");

    // Verify VCPKG FFmpeg installation exists
    let lib_path = installed_path.join("lib");
    let include_path = installed_path.join("include");
    let bin_path = installed_path.join("bin");

    if !lib_path.exists() {
        panic!(
            "VCPKG FFmpeg not found at: {}\n\
            Please install FFmpeg via VCPKG:\n\
            vcpkg install ffmpeg:x64-windows",
            lib_path.display()
        );
    }

    // Verify required libraries exist
    for lib in FFMPEG_LIBS {
        let lib_file = lib_path.join(format!("{lib}.lib"));
        if !lib_file.exists() {
            panic!(
                "FFmpeg library not found: {}\n\
                Please reinstall FFmpeg via VCPKG:\n\
                vcpkg install ffmpeg:x64-windows",
                lib_file.display()
            );
        }
    }

    // Set library search path
    println!("cargo:rustc-link-search=native={}", lib_path.display());

    // Link FFmpeg libraries (dynamic linking)
    for lib in FFMPEG_LIBS {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }

    // Set DLL search path for runtime
    // This helps locate FFmpeg DLLs when running the application
    if bin_path.exists() {
        println!("cargo:rustc-env=FFMPEG_DLL_PATH={}", bin_path.display());
    }

    // Configure LLVM/clang path for bindgen
    configure_llvm_for_bindgen();

    vec![include_path.to_string_lossy().into_owned()]
}

/// Configure LLVM/clang path for bindgen on Windows.
///
/// bindgen requires libclang to parse C headers. On Windows, this is typically
/// provided by an LLVM installation.
fn configure_llvm_for_bindgen() {
    // Common LLVM installation paths on Windows
    let llvm_paths = [
        env::var("LIBCLANG_PATH").ok(),
        Some("C:\\Program Files\\LLVM\\bin".to_string()),
        Some("C:\\Program Files (x86)\\LLVM\\bin".to_string()),
        env::var("LLVM_HOME").ok().map(|p| format!("{p}\\bin")),
    ];

    for path in llvm_paths.into_iter().flatten() {
        let clang_dll = Path::new(&path).join("libclang.dll");
        if clang_dll.exists() {
            // Set LIBCLANG_PATH for bindgen
            // SAFETY: This is a build script running in a single-threaded context.
            // Setting environment variables is safe here as no other threads are
            // accessing environment variables concurrently.
            unsafe {
                env::set_var("LIBCLANG_PATH", &path);
            }
            return;
        }
    }

    // If LIBCLANG_PATH is already set, assume it's valid
    if env::var("LIBCLANG_PATH").is_ok() {
        return;
    }

    // Warn but don't fail - bindgen might find it through other means
    println!(
        "cargo:warning=LLVM/clang not found. Set LIBCLANG_PATH environment variable \
         to the LLVM bin directory containing libclang.dll"
    );
}

/// Configure `FFmpeg` linking for macOS via Homebrew.
///
/// This function tries the following detection methods in order:
/// 1. Homebrew installation (Apple Silicon: `/opt/homebrew`, Intel: `/usr/local`)
/// 2. pkg-config as a fallback
///
/// Returns include paths for bindgen.
fn configure_macos() -> Vec<String> {
    println!("cargo:rerun-if-env-changed=HOMEBREW_PREFIX");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    // Try Homebrew first
    if let Some(paths) = try_homebrew() {
        return paths;
    }

    // Fall back to pkg-config
    if let Some(paths) = try_pkgconfig_unix() {
        return paths;
    }

    panic!(
        "FFmpeg not found on macOS.\n\
        Please install FFmpeg via Homebrew:\n\
        brew install ffmpeg\n\n\
        Or ensure pkg-config can find FFmpeg:\n\
        export PKG_CONFIG_PATH=\"/path/to/ffmpeg/lib/pkgconfig\""
    );
}

/// Try to configure FFmpeg via Homebrew.
///
/// Returns include paths if successful, None if FFmpeg is not found.
fn try_homebrew() -> Option<Vec<String>> {
    // Detect Homebrew prefix
    // - Apple Silicon (arm64): /opt/homebrew
    // - Intel (x86_64): /usr/local
    let homebrew_prefix = env::var("HOMEBREW_PREFIX").unwrap_or_else(|_| {
        // Auto-detect based on architecture
        let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        if arch == "aarch64" {
            "/opt/homebrew".to_string()
        } else {
            "/usr/local".to_string()
        }
    });

    let homebrew_path = Path::new(&homebrew_prefix);
    let lib_path = homebrew_path.join("lib");
    let include_path = homebrew_path.join("include");

    // Verify Homebrew installation exists
    if !lib_path.exists() {
        return None;
    }

    // Verify FFmpeg libraries exist
    let mut all_found = true;
    for lib in FFMPEG_LIBS {
        // Check for .dylib files (macOS dynamic libraries)
        let dylib_file = lib_path.join(format!("lib{lib}.dylib"));
        if !dylib_file.exists() {
            // Also check for .a files (static libraries)
            let static_file = lib_path.join(format!("lib{lib}.a"));
            if !static_file.exists() {
                all_found = false;
                break;
            }
        }
    }

    if !all_found {
        return None;
    }

    // Verify include path contains FFmpeg headers
    let avcodec_header = include_path.join("libavcodec").join("avcodec.h");
    if !avcodec_header.exists() {
        return None;
    }

    // Set library search path
    println!("cargo:rustc-link-search=native={}", lib_path.display());

    // Link FFmpeg libraries (dynamic linking)
    for lib in FFMPEG_LIBS {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }

    Some(vec![include_path.to_string_lossy().into_owned()])
}

/// Configure `FFmpeg` linking for Linux via pkg-config.
///
/// Returns the include paths detected by pkg-config.
fn configure_linux() -> Vec<String> {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    if let Some(paths) = try_pkgconfig_unix() {
        return paths;
    }

    panic!(
        "FFmpeg not found on Linux.\n\
        Please install FFmpeg development packages:\n\n\
        Ubuntu/Debian:\n\
        sudo apt install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev\n\n\
        Fedora:\n\
        sudo dnf install ffmpeg-devel\n\n\
        Arch Linux:\n\
        sudo pacman -S ffmpeg\n\n\
        If FFmpeg is installed in a non-standard location, set PKG_CONFIG_PATH:\n\
        export PKG_CONFIG_PATH=\"/path/to/ffmpeg/lib/pkgconfig\""
    );
}

/// Minimum versions per library required for FFmpeg 7.x.
///
/// Each libav* library has its own version number independent of the FFmpeg
/// suite version. These values correspond to the library versions shipped
/// with FFmpeg 7.0.
///
/// | Library        | FFmpeg 6.x | FFmpeg 7.x |
/// |----------------|-----------|-----------|
/// | libavformat    | 60.x      | 61.x      |
/// | libavcodec     | 60.x      | 61.x      |
/// | libavutil      | 58.x      | 59.x      |
/// | libswscale     | 7.x       | 8.x       |
/// | libswresample  | 4.x       | 5.x       |
const PKGCONFIG_LIBS: &[(&str, &str)] = &[
    ("libavformat", "61.0"),
    ("libavcodec", "61.0"),
    ("libavutil", "59.0"),
    ("libswscale", "8.0"),
    ("libswresample", "5.0"),
    ("libavfilter", "10.0"),
];

/// Try to configure FFmpeg via pkg-config (Unix systems).
///
/// Returns include paths if successful, None if FFmpeg is not found.
fn try_pkgconfig_unix() -> Option<Vec<String>> {
    let mut include_paths = Vec::new();
    let mut all_found = true;

    for (lib, min_version) in PKGCONFIG_LIBS {
        match pkg_config::Config::new()
            .atleast_version(min_version)
            .probe(lib)
        {
            Ok(library) => {
                // Collect include paths from pkg-config
                for path in &library.include_paths {
                    let path_str = path.to_string_lossy().to_string();
                    if !include_paths.contains(&path_str) {
                        include_paths.push(path_str);
                    }
                }
            }
            Err(e) => {
                // Log the error but continue checking other libraries
                println!("cargo:warning=pkg-config: {lib} not found: {e}");
                all_found = false;
                break;
            }
        }
    }

    if all_found { Some(include_paths) } else { None }
}

/// Emit Cargo cfg flags for FFmpeg API variants based on library versions.
///
/// Different FFmpeg major versions ship different API shapes for the same
/// functionality. We detect the installed version from the headers and emit
/// cfg flags so that Rust source code can conditionally compile the correct
/// constant/type names without relying on platform assumptions.
///
/// # libswscale SWS flags
///
/// | FFmpeg suite | libswscale | SWS_* constants |
/// |-------------|------------|-----------------|
/// | 7.x         | 8.x        | `#define` macros → `SWS_FAST_BILINEAR` etc. |
/// | 8.x         | 9.x        | C enum `SwsFlags` → `SwsFlags_SWS_FAST_BILINEAR` etc. |
///
/// Emits `ffmpeg_sws_flags_enum` when libswscale major version ≥ 9.
///
/// # `AV_BUFFERSRC_FLAG_KEEP_REF` type
///
/// bindgen generates `AV_BUFFERSRC_FLAG_KEEP_REF` as `u32` on Linux/macOS
/// (pkg-config / Homebrew FFmpeg) but as `i32` on Windows (VCPKG FFmpeg).
///
/// Emits `ffmpeg_buffersrc_flag_u32` on platforms where the type is `u32`,
/// so callers can use the normalized `BUFFERSRC_FLAG_KEEP_REF_I32` constant
/// instead of casting at every call site.
fn emit_api_cfg_flags(include_paths: &[String]) {
    let swscale_major = read_version_major(include_paths, "libswscale");

    if let Some(major) = swscale_major {
        if major >= 9 {
            // FFmpeg 8.x: SWS_* flags are a C enum, bindgen generates SwsFlags_SWS_*
            println!("cargo:rustc-cfg=ffmpeg8");
        }
    } else {
        println!(
            "cargo:warning=Could not detect libswscale version; \
             assuming FFmpeg 7.x (#define SWS_* constants)"
        );
    }

    // On Linux and macOS, bindgen generates AV_BUFFERSRC_FLAG_KEEP_REF as u32.
    // On Windows (VCPKG), it is generated as i32.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if matches!(target_os.as_str(), "linux" | "macos") {
        println!("cargo:rustc-cfg=ffmpeg_buffersrc_flag_u32");
    }
}

/// Read the major version number from a libav*/libsw* version header.
///
/// Searches `include_paths` for `<lib>/version_major.h` (preferred) or
/// `<lib>/version.h` and returns the value of `LIB*_VERSION_MAJOR`.
fn read_version_major(include_paths: &[String], lib: &str) -> Option<u32> {
    for base in include_paths {
        let base = Path::new(base).join(lib);
        let candidates = [base.join("version_major.h"), base.join("version.h")];

        for path in &candidates {
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };

            // Look for a line like:  #define LIBSWSCALE_VERSION_MAJOR  9
            let needle = format!(
                "LIB{}_VERSION_MAJOR",
                lib.trim_start_matches("lib").to_ascii_uppercase()
            );
            for line in content.lines() {
                if line.contains(&needle) {
                    if let Some(val) = line.split_whitespace().last() {
                        if let Ok(n) = val.parse::<u32>() {
                            return Some(n);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Generate FFI bindings using bindgen.
///
/// # Arguments
/// * `include_paths` - Include paths collected from platform-specific configuration
fn generate_bindings(include_paths: &[String]) {
    // Build bindgen with include paths
    let mut builder = bindgen::Builder::default().header("wrapper.h");

    // Add all include paths
    for path in include_paths {
        builder = builder.clang_arg(format!("-I{path}"));
    }

    let bindings = builder
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Allowlist FFmpeg functions
        .allowlist_function("av_.*")
        .allowlist_function("avformat_.*")
        .allowlist_function("avcodec_.*")
        .allowlist_function("sws_.*")
        .allowlist_function("swr_.*")
        // Allowlist FFmpeg types
        .allowlist_type("AV.*")
        .allowlist_type("Sws.*")
        .allowlist_type("Swr.*")
        // Allowlist FFmpeg constants
        .allowlist_var("AV_.*")
        .allowlist_var("AVERROR.*")
        .allowlist_var("AVSEEK_.*")
        .allowlist_var("AVIO_.*")
        .allowlist_var("SWS_.*")
        .allowlist_var("SWR_.*")
        // Allowlist libavfilter functions and types
        .allowlist_function("avfilter_.*")
        .allowlist_function("av_buffersrc_.*")
        .allowlist_function("av_buffersink_.*")
        .allowlist_type("AVFilter.*")
        .allowlist_var("AV_BUFFERSRC_.*")
        .allowlist_var("AV_BUFFERSINK_.*")
        // Derive traits for safety and convenience
        .derive_debug(true)
        .derive_default(true)
        // Disable doc comments - FFmpeg C comments contain invalid Rust code
        .generate_comments(false)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");

    // Rerun build script if wrapper.h changes
    println!("cargo:rerun-if-changed=wrapper.h");
}
