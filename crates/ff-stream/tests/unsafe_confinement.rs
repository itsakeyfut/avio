//! Guards the `unsafe` confinement rule from `docs/rules/unsafe.md`.

use std::path::Path;

/// Every file opting out of the workspace `unsafe_code` lint must be an
/// `*_inner` module.
///
/// This is #1597's acceptance criterion as an executable check. The drift it
/// fixed accumulated silently precisely because nothing enforced it: each new
/// feature module added its own `#![allow(unsafe_code)]` and the count grew to
/// 17 files across three crates before anyone counted.
#[test]
fn no_non_inner_file_should_allow_unsafe_code() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    visit(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "these files opt out of `unsafe_code` but are not `*_inner` modules, so the \
         FFI they carry is not confined: {offenders:#?}"
    );
}

/// The opt-out check must survive the ways the attribute can legitimately be
/// written, not just the one spelling the `*_inner` modules happen to use today.
#[test]
fn opt_out_check_should_see_past_the_attribute_spelling() {
    assert!(opts_out_of_unsafe_code("#![allow(unsafe_code)]"));
    assert!(opts_out_of_unsafe_code(
        "#![allow(unsafe_code, unsafe_op_in_unsafe_fn)]"
    ));
    assert!(opts_out_of_unsafe_code(
        "#![allow(clippy::pedantic,unsafe_code)]"
    ));
    assert!(opts_out_of_unsafe_code("#![expect(unsafe_code)]"));
    assert!(opts_out_of_unsafe_code("#![allow(\n    unsafe_code,\n)]"));
    assert!(!opts_out_of_unsafe_code(
        "#![allow(unsafe_op_in_unsafe_fn)]"
    ));
    assert!(!opts_out_of_unsafe_code(r#"f().expect("unsafe_code");"#));
}

/// Whether `src` opts out of the `unsafe_code` lint.
///
/// Matches the attribute's argument list rather than the literal text
/// `allow(unsafe_code)`: a combined `#![allow(unsafe_code,
/// unsafe_op_in_unsafe_fn)]` opts out just as effectively, and every `*_inner`
/// module already carries both lints as separate lines, so merging them is a
/// natural edit that must not silently disable this guard. `expect` counts for
/// the same reason. Whitespace is stripped first so a wrapped attribute is
/// matched too.
fn opts_out_of_unsafe_code(src: &str) -> bool {
    let flat: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    for keyword in ["allow(", "expect("] {
        let mut rest = flat.as_str();
        while let Some(start) = rest.find(keyword) {
            rest = &rest[start + keyword.len()..];
            let args = rest.find(')').map_or(rest, |end| &rest[..end]);
            if args.split(',').any(|arg| arg == "unsafe_code") {
                return true;
            }
        }
    }
    false
}

fn visit(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("source directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            visit(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path.to_string_lossy().contains("_inner")
            && opts_out_of_unsafe_code(
                &std::fs::read_to_string(&path).expect("source file should be readable"),
            )
        {
            out.push(path.display().to_string());
        }
    }
}
