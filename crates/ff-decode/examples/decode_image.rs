//! Decode a still image and inspect its properties.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example decode_image -- \
//!   --input photo.jpg \
//!   [--dump raw.yuv]
//! ```

use std::{io::Write as _, path::Path, process};

use ff_decode::ImageDecoder;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut dump = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--dump" => dump = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: decode_image --input <file> [--dump <raw.yuv>]");
        process::exit(1);
    });

    // Check supported extension
    let ext = Path::new(&input)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "bmp" | "webp" | "tiff" | "tif" => {}
        other => {
            eprintln!("Error: unsupported image format '.{other}' (try jpg, png, bmp, webp, tiff)");
            process::exit(1);
        }
    }

    let format_name = match ext.as_str() {
        "jpg" | "jpeg" => "JPEG",
        "png" => "PNG",
        "bmp" => "BMP",
        "webp" => "WebP",
        "tiff" | "tif" => "TIFF",
        _ => "Unknown",
    };

    let file_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    let dec = match ImageDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let frame = match dec.decode() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let w = frame.width();
    let h = frame.height();
    let pix_fmt = frame.format();
    let num_planes = frame.num_planes();
    let total_size = frame.total_size();

    println!("File:         {file_name}");
    println!("Format:       {format_name}");
    println!("Dimensions:   {w}×{h}");
    println!("Pixel format: {pix_fmt}");
    println!("Planes:       {num_planes}");

    for i in 0..num_planes {
        let stride = frame.stride(i).unwrap_or(0);
        let plane_len = frame.plane(i).map_or(0, |p| p.len());
        let label = match i {
            0 => " (Y)",
            1 => " (U)",
            2 => " (V)",
            _ => "",
        };
        println!("  plane {i}{label}  stride={stride}  size={plane_len} bytes");
    }

    println!("Total size:   {total_size} bytes");

    if let Some(dump_path) = dump {
        let data = frame.data();
        match std::fs::File::create(&dump_path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(&data) {
                    eprintln!("Error writing dump: {e}");
                    process::exit(1);
                }
                println!("\nRaw pixels written to: {dump_path}");
            }
            Err(e) => {
                eprintln!("Error creating dump file: {e}");
                process::exit(1);
            }
        }
    }
}
