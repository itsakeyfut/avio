//! `cargo xtask publish-readiness`: the gate that says the workspace could be
//! released today.
//!
//! It fails if any library crate is not ready to publish, or if a member that is
//! meant to stay unpublished becomes publishable. Over the root workspace
//! members reported by `cargo metadata --no-deps`, it checks:
//!
//! 1. Every member expected to stay unpublished still has `publish = false`.
//!    This catches a non-library member accidentally becoming publishable.
//! 2. Every publishable member declares the full publish metadata set:
//!    description, license (or license-file), repository, readme, keywords,
//!    categories. `cargo publish` only errors on a missing description or
//!    license, so keywords and categories are enforced here.
//! 3. `cargo publish --dry-run --no-verify --workspace` packages every library
//!    crate. `--workspace` resolves the internal `ff-*` dependencies against the
//!    local members, so this passes before those versions exist on crates.io (a
//!    per-crate dry-run would fail there), and it skips the `publish = false`
//!    members automatically. `--no-verify` skips the compile step, which the
//!    test, clippy and features jobs already cover and which matches
//!    release-plz's `publish_no_verify = true`, so this needs no FFmpeg.
//!
//! The `tools` and `fuzz` crates live in separate workspaces, so they never
//! appear in this workspace's `--no-deps` metadata and are excluded
//! automatically.

use std::process::Command;

use crate::json::{self, Value};
use crate::proc;

/// Root-workspace members that are intentionally never published.
///
/// Each must keep `publish = false`; removing it fails the gate. `xtask` is on
/// this list for the same reason as the example harness: it is repository
/// tooling, not part of the published API.
const EXPECTED_NON_LIBRARY: &[&str] = &["avio-examples", "xtask"];

/// Publish metadata every library crate must declare.
///
/// `license` is satisfied by either `license` or `license-file`; the rest must
/// be present and non-empty.
const REQUIRED_FIELDS: &[&str] = &[
    "description",
    "license",
    "repository",
    "readme",
    "keywords",
    "categories",
];

pub fn run(_args: &[String]) -> u8 {
    let Some(metadata) = proc::stdout("cargo", &["metadata", "--format-version", "1", "--no-deps"])
    else {
        eprintln!("error: `cargo metadata --no-deps` failed");
        return 1;
    };
    let parsed = match json::parse(&metadata) {
        Ok(value) => value,
        Err(reason) => {
            eprintln!("error: could not read `cargo metadata` output: {reason}");
            return 1;
        }
    };
    let Some(packages) = parsed.get("packages").and_then(Value::as_array) else {
        eprintln!("error: `cargo metadata` output has no `packages` array");
        return 1;
    };

    let mut errors: Vec<String> = Vec::new();

    // 1. Known non-library members must stay unpublished.
    for name in EXPECTED_NON_LIBRARY {
        match packages.iter().find(|pkg| package_name(pkg) == Some(name)) {
            None => errors.push(format!(
                "{name}: expected a non-library workspace member, but it was not found"
            )),
            Some(pkg) if is_publishable(pkg) => {
                errors.push(format!(
                    "{name}: expected `publish = false`, but it is publishable"
                ));
            }
            Some(_) => {}
        }
    }

    // 2. Every library crate needs complete metadata.
    let mut libraries: Vec<&str> = packages
        .iter()
        .filter(|pkg| is_publishable(pkg))
        .filter_map(package_name)
        .filter(|name| !EXPECTED_NON_LIBRARY.contains(name))
        .collect();
    libraries.sort_unstable();
    for name in &libraries {
        let Some(pkg) = packages.iter().find(|pkg| package_name(pkg) == Some(name)) else {
            continue;
        };
        let missing = missing_fields(pkg);
        if !missing.is_empty() {
            errors.push(format!(
                "{name}: incomplete publish metadata, missing {}",
                missing.join(", ")
            ));
        }
    }

    // 3. The dry run, with its output inherited so a packaging failure is
    //    readable in the job log rather than summarised away.
    println!("cargo publish --dry-run --no-verify --workspace");
    let dry_run = Command::new("cargo")
        .args(["publish", "--dry-run", "--no-verify", "--workspace"])
        .status();
    match dry_run {
        Ok(status) if status.success() => {}
        Ok(_) => {
            errors.push("`cargo publish --dry-run --no-verify --workspace` failed".to_string())
        }
        Err(err) => errors.push(format!("could not run `cargo publish --dry-run`: {err}")),
    }

    if !errors.is_empty() {
        eprintln!("\nPublishing-readiness gate FAILED:");
        for error in &errors {
            eprintln!("  - {error}");
        }
        return 1;
    }
    println!(
        "\nPublishing-readiness gate passed: {} library crates ready to publish.",
        libraries.len()
    );
    0
}

fn package_name(package: &Value) -> Option<&str> {
    package.get("name").and_then(Value::as_str)
}

/// A package is publishable unless its manifest sets `publish = false`.
///
/// `cargo metadata` encodes `publish` as null (unrestricted) or as the list of
/// allowed registries; `publish = false` becomes the empty list. So the test is
/// specifically for null, not merely for "no registries".
fn is_publishable(package: &Value) -> bool {
    package.get("publish").is_none_or(Value::is_null)
}

/// The required publish-metadata fields this package lacks.
fn missing_fields(package: &Value) -> Vec<String> {
    let mut missing = Vec::new();
    for field in REQUIRED_FIELDS {
        if *field == "license" {
            let has_license = !blank(package, "license") || !blank(package, "license_file");
            if !has_license {
                missing.push("license/license-file".to_string());
            }
        } else if blank(package, field) {
            missing.push((*field).to_string());
        }
    }
    missing
}

/// True when the field is absent, null, empty, or an empty list.
fn blank(package: &Value, field: &str) -> bool {
    package.get(field).is_none_or(Value::is_blank)
}
