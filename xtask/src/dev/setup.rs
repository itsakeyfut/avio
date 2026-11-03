//! Prerequisite check for avio development.
//!
//! Run this after cloning to find out what is still needed before `cargo build`
//! will succeed. Each check is listed with its result; exits non-zero when
//! anything is missing.
//!
//! The checks are read-only: they inspect the environment and print what is
//! needed, but never run an installer. The install commands are printed so the
//! contributor can copy-paste them; the choice of when to run a long package
//! installation is theirs.

use std::path::Path;

use crate::proc;

pub fn run(args: &[String]) -> u8 {
    if !args.is_empty() {
        eprintln!("error: setup takes no arguments");
        return 2;
    }

    println!("setup: checking prerequisites for avio development\n");

    let checks = collect();
    let missing = print_report(&checks);

    if missing == 0 {
        println!("setup: all prerequisites satisfied");
        0
    } else {
        println!("setup: {missing} prerequisite(s) missing");
        1
    }
}

struct Check {
    name: &'static str,
    status: Status,
    /// One-line detail shown on the status line (found path, or what was not found).
    detail: String,
    /// Command(s) to run to satisfy this prerequisite, printed when missing.
    install: Option<String>,
}

enum Status {
    Ok,
    Missing,
}

fn collect() -> Vec<Check> {
    match std::env::consts::OS {
        "windows" => vec![check_ffmpeg_windows(), check_clang_windows()],
        "macos" => vec![check_ffmpeg_macos()],
        "linux" => vec![check_ffmpeg_linux()],
        os => {
            eprintln!("warning: setup checks are not implemented for {os}");
            vec![]
        }
    }
}

// ── Windows ──────────────────────────────────────────────────────────────────

fn vcpkg_lib_path() -> std::path::PathBuf {
    let root = std::env::var("VCPKG_ROOT").unwrap_or_else(|_| "C:\\vcpkg".to_string());
    Path::new(&root)
        .join("installed")
        .join("x64-windows")
        .join("lib")
}

fn check_ffmpeg_windows() -> Check {
    let lib_path = vcpkg_lib_path();
    let marker = lib_path.join("avformat.lib");

    if marker.exists() {
        return Check {
            name: "ffmpeg",
            status: Status::Ok,
            detail: lib_path.display().to_string(),
            install: None,
        };
    }

    let root = std::env::var("VCPKG_ROOT").unwrap_or_else(|_| "C:\\vcpkg".to_string());
    Check {
        name: "ffmpeg",
        status: Status::Missing,
        detail: format!("vcpkg x64-windows package not found (VCPKG_ROOT={root})"),
        install: Some(
            "vcpkg install \"ffmpeg[avcodec,avformat,avfilter,swscale,swresample,drawtext]:x64-windows\"\n\
             \n\
             If vcpkg is not at C:\\vcpkg, set VCPKG_ROOT first:\n\
             \n\
             set VCPKG_ROOT=<path to vcpkg>\n\
             vcpkg install \"ffmpeg[avcodec,avformat,avfilter,swscale,swresample,drawtext]:x64-windows\""
                .to_string(),
        ),
    }
}

fn check_clang_windows() -> Check {
    // bindgen requires libclang.dll. Check the same paths as build.rs.
    let candidates: &[&str] = &[
        // LIBCLANG_PATH env var (user override or set by a previous build)
        // handled separately below
        "C:\\Program Files\\LLVM\\bin\\libclang.dll",
        "C:\\Program Files (x86)\\LLVM\\bin\\libclang.dll",
    ];

    // Env var override: check it first if set.
    if let Ok(path) = std::env::var("LIBCLANG_PATH") {
        let dll = Path::new(&path).join("libclang.dll");
        if dll.exists() {
            return Check {
                name: "LLVM / clang  (bindgen)",
                status: Status::Ok,
                detail: dll.display().to_string(),
                install: None,
            };
        }
    }

    for candidate in candidates {
        let dll = Path::new(candidate);
        if dll.exists() {
            return Check {
                name: "LLVM / clang  (bindgen)",
                status: Status::Ok,
                detail: dll.display().to_string(),
                install: None,
            };
        }
    }

    Check {
        name: "LLVM / clang  (bindgen)",
        status: Status::Missing,
        detail: "libclang.dll not found".to_string(),
        install: Some(
            "Download and install LLVM from https://github.com/llvm/llvm-project/releases\n\
             (choose LLVM-<version>-win64.exe, check \"Add LLVM to the system PATH\")\n\
             \n\
             Or set LIBCLANG_PATH to the directory containing libclang.dll:\n\
             set LIBCLANG_PATH=C:\\path\\to\\LLVM\\bin"
                .to_string(),
        ),
    }
}

// ── macOS ─────────────────────────────────────────────────────────────────────

fn check_ffmpeg_macos() -> Check {
    // Mirror the detection order in build.rs: HOMEBREW_PREFIX env var, then
    // Apple Silicon default, then Intel default.
    let prefixes: &[&str] = &["/opt/homebrew", "/usr/local"];

    // Env var override.
    if let Ok(prefix) = std::env::var("HOMEBREW_PREFIX") {
        let lib = Path::new(&prefix).join("lib").join("libavformat.dylib");
        if lib.exists() {
            return Check {
                name: "ffmpeg",
                status: Status::Ok,
                detail: format!("{prefix}/lib"),
                install: None,
            };
        }
    }

    for prefix in prefixes {
        let lib = Path::new(prefix).join("lib").join("libavformat.dylib");
        if lib.exists() {
            return Check {
                name: "ffmpeg",
                status: Status::Ok,
                detail: format!("{prefix}/lib"),
                install: None,
            };
        }
    }

    Check {
        name: "ffmpeg",
        status: Status::Missing,
        detail: "libavformat.dylib not found via Homebrew".to_string(),
        install: Some("brew install ffmpeg".to_string()),
    }
}

// ── Linux ─────────────────────────────────────────────────────────────────────

fn check_ffmpeg_linux() -> Check {
    // Use pkg-config, which is what build.rs uses.
    let found = proc::stdout("pkg-config", &["--exists", "libavformat"]).is_some();

    if found {
        let version = proc::stdout("pkg-config", &["--modversion", "libavformat"])
            .unwrap_or_else(|| "unknown version".to_string());
        return Check {
            name: "ffmpeg",
            status: Status::Ok,
            detail: format!("libavformat {version}"),
            install: None,
        };
    }

    Check {
        name: "ffmpeg",
        status: Status::Missing,
        detail: "libavformat not found via pkg-config".to_string(),
        install: Some(
            "# Ubuntu / Debian:\n\
             sudo apt install \\\n\
               libavcodec-dev libavformat-dev libavutil-dev \\\n\
               libswscale-dev libswresample-dev libavfilter-dev\n\
             \n\
             # Fedora:\n\
             sudo dnf install ffmpeg-devel\n\
             \n\
             # Arch:\n\
             sudo pacman -S ffmpeg"
                .to_string(),
        ),
    }
}

// ── Report ────────────────────────────────────────────────────────────────────

/// Prints the status table and, for missing checks, the install instructions.
/// Returns the number of missing prerequisites.
fn print_report(checks: &[Check]) -> usize {
    for check in checks {
        match check.status {
            Status::Ok => println!("  ok    {}  ({})", check.name, check.detail),
            Status::Missing => println!("  MISS  {}  {}", check.name, check.detail),
        }
    }

    let missing: Vec<&Check> = checks
        .iter()
        .filter(|c| matches!(c.status, Status::Missing))
        .collect();

    if !missing.is_empty() {
        println!("\nTo install what is missing:\n");
        for (i, check) in missing.iter().enumerate() {
            println!("  {}. {}", i + 1, check.name);
            if let Some(install) = &check.install {
                for line in install.lines() {
                    println!("     {line}");
                }
            }
            println!();
        }
    }

    missing.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_accepted() {
        // setup takes no arguments; any argument should return exit code 2.
        let result = run(&["--unexpected".to_string()]);
        assert_eq!(result, 2);
    }
}
