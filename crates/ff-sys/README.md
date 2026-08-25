# ff-sys

## Why ff-sys?

Crates like [`ffmpeg-sys-next`](https://crates.io/crates/ffmpeg-sys-next), [`ffmpeg-next`](https://crates.io/crates/ffmpeg-next), and [`rsmpeg`](https://crates.io/crates/rsmpeg) already wrap FFmpeg well. `ff-sys` does not try to replace them; it is the purpose-built FFI base for the `ff-*` / `avio` family, optimised for that one job:

- **Version and ABI control**: it targets FFmpeg 7.x / 8.x directly and owns exactly which versions and ABI quirks it supports (for example the `SWS_*` flag change between libswscale 8 and 9, handled via the `ffmpeg8` cfg).
- **Self-contained build detection**: the build script locates FFmpeg per platform on its own (vcpkg via `VCPKG_ROOT` on Windows, pkg-config on Linux, Homebrew on macOS) and drives bindgen through `LIBCLANG_PATH`, so the family is not bound to another crate's build-script assumptions.
- **An implementation detail, not a public API**: application code uses the safe `ff-*` crates, never `ff-sys` directly, so "mature vs. new" matters less for a base you are not meant to depend on.

Building your own project on FFmpeg directly? The crates above are excellent choices. Building on `avio`? You already get `ff-sys` transitively; reach for the safe `ff-*` crates.

## Installation Prerequisites

**FFmpeg 7.x or 8.x** development libraries must be available on your system before building any crate in this workspace. FFmpeg 6.x is not supported (the `SWS_*` flags differ in API shape); 8.x is detected automatically via the `SwsFlags` enum (the `ffmpeg8` cfg).

### Windows

Install FFmpeg via [vcpkg](https://github.com/microsoft/vcpkg):

```sh
vcpkg install ffmpeg:x64-windows
```

The build script reads `VCPKG_ROOT` to locate the installation (defaulting to `C:\vcpkg`) and expects FFmpeg under `<VCPKG_ROOT>\installed\x64-windows`. bindgen also requires libclang: set `LIBCLANG_PATH` to your LLVM `bin` directory (containing `libclang.dll`) if it is not in a standard location such as `C:\Program Files\LLVM\bin`.

### Linux

Detected via pkg-config:

```sh
sudo apt install libavcodec-dev libavformat-dev libavutil-dev \
                 libavfilter-dev libswscale-dev libswresample-dev
```

If FFmpeg is installed in a non-standard location, set `PKG_CONFIG_PATH` to its `lib/pkgconfig` directory.

### macOS

```sh
brew install ffmpeg
```

## Platform Support

| Platform | Detection            | Notes                                            |
|----------|----------------------|--------------------------------------------------|
| Windows  | vcpkg (`VCPKG_ROOT`) | `ffmpeg:x64-windows` triplet; `LIBCLANG_PATH` for bindgen |
| Linux    | pkg-config           | Dev packages (`-dev`) must be installed          |
| macOS    | Homebrew, pkg-config | Auto-detects `/opt/homebrew` or `/usr/local`, falls back to pkg-config |

## Wrapper Modules

Alongside the raw bindgen output, `ff-sys` ships thin safe-wrapper modules that isolate the most error-prone FFmpeg call sequences:

| Module       | Wraps                                      |
|--------------|--------------------------------------------|
| `avcodec`    | `AVCodecContext`, codec open/close         |
| `avformat`   | `AVFormatContext`, demux/mux lifecycle     |
| `swscale`    | `SwsContext`, pixel format conversion      |
| `swresample` | `SwrContext`, sample format / channel layout conversion |

These modules are public (`pub mod`) but are meant for use by the higher-level `ff-*` crates rather than for direct consumption.

## Usage

`ff-sys` is normally consumed transitively through the safe `ff-*` crates. A few of the thin wrappers are themselves safe, however, and can be called directly:

```rust
use ff_sys::{av_error_string, avformat, error_codes};

fn main() {
    // Convert an FFmpeg error code into a readable message.
    let msg = av_error_string(error_codes::ENOMEM);
    println!("ENOMEM: {msg}");

    // Query the linked FFmpeg build for optional protocol support.
    let has_srt = avformat::srt_available();
    println!("libsrt available: {has_srt}");
}
```

Most of the surface is raw FFI and therefore `unsafe`; the safe wrappers above and the `ff-*` crates exist so that application code never has to touch it directly.

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
