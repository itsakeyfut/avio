//! `cargo xtask test`: run a crate's tests with the GPU targets serialised.
//!
//! Why this exists (#1718): a test binary that builds many wgpu contexts
//! concurrently livelocks in `RenderContext::init_blocking`. libtest defaults to
//! one thread per logical CPU, so a suite with around thirty `GpuCompositor::new()`
//! call sites intermittently stops forever and has to be killed.
//! `--test-threads=1` is stable for those.
//!
//! Why not just pass `--test-threads=1` everywhere: serialising the whole run
//! hides defects that only appear under parallelism. One parity suite shared an
//! output path between two tests and failed three times out of three at default
//! parallelism while passing green under `--test-threads=1`, and its failure
//! message read as "the formula is wrong". So: GPU targets serialised, everything
//! else at default parallelism. Cargo already runs test *binaries* one at a time,
//! which makes the target the right granularity.
//!
//! Usage:
//!
//! ```text
//! cargo xtask test -p avio [-p ff-preview ...] [extra cargo args]
//! cargo xtask test -p avio --timeout 300       # per-target kill after 300s (default 600)
//! cargo xtask test -p avio --features preview  # any feature flag suppresses --all-features
//! ```
//!
//! At least one `-p` is required: a workspace-wide local run is what freezes a
//! development machine. `CARGO_TARGET_DIR` is honoured if it is set; this command
//! does not choose one.
//!
//! It prints `{ok, targets, totals, log_tail}` as JSON, where each target carries
//! its `mode` (`parallel` or `serial(gpu)`), so which targets were serialised is
//! visible rather than assumed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::json;
use crate::proc;
use crate::repo;

/// Anything that ends up building a wgpu adapter and device.
///
/// Detected rather than listed: a hardcoded suite list goes stale the moment
/// someone adds a GPU test. Both `RenderContext::new` and `RenderContext::init`
/// are here because `ff-render`'s own `gpu_nodes.rs` reaches the adapter through
/// `block_on(RenderContext::init())`, which a narrower marker missed.
/// Over-matching only costs wall clock; under-matching costs an indefinite hang.
const GPU_MARKERS: &[&str] = &[
    "GpuCompositor::new",
    "GpuPreviewCompositor::new",
    "RenderContext::new",
    "RenderContext::init",
];

const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// One cargo invocation's result, as it appears in the JSON.
struct Row {
    name: String,
    mode: &'static str,
    passed: u64,
    failed: u64,
    ignored: u64,
    status: &'static str,
}

pub fn run(args: &[String]) -> u8 {
    let mut crates: Vec<String> = Vec::new();
    let mut cargo_args: Vec<String> = Vec::new();
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut has_feature_flag = false;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-p" | "--package" => {
                let Some(name) = args.get(i + 1) else {
                    eprintln!("error: {arg} needs a crate name");
                    return 2;
                };
                crates.push(name.clone());
                i += 2;
            }
            "--timeout" => {
                let Some(value) = args.get(i + 1).and_then(|v| v.parse::<u64>().ok()) else {
                    eprintln!("error: --timeout needs a number of seconds");
                    return 2;
                };
                timeout_secs = value;
                i += 2;
            }
            "--all-features" | "--no-default-features" => {
                has_feature_flag = true;
                cargo_args.push(arg.to_string());
                i += 1;
            }
            "--features" => {
                has_feature_flag = true;
                cargo_args.push(arg.to_string());
                if let Some(value) = args.get(i + 1) {
                    cargo_args.push(value.clone());
                }
                i += 2;
            }
            _ => {
                if arg.starts_with("--features=") {
                    has_feature_flag = true;
                }
                cargo_args.push(arg.to_string());
                i += 1;
            }
        }
    }

    if crates.is_empty() {
        eprintln!(
            "error: pass at least one -p <crate>. A workspace-wide local run freezes this \
             machine; scope to the changed crates and let CI do the full sweep."
        );
        return 2;
    }
    if !has_feature_flag {
        cargo_args.push("--all-features".to_string());
    }

    let root = repo::root();
    let timeout = Duration::from_secs(timeout_secs);
    let mut rows: Vec<Row> = Vec::new();
    let mut log = String::new();

    for krate in &crates {
        let Some(dir) = crate_dir(&root, krate) else {
            rows.push(Row {
                name: krate.clone(),
                mode: "-",
                passed: 0,
                failed: 0,
                ignored: 0,
                status: "no-such-crate",
            });
            continue;
        };

        // The lib target is classified by whether anything in its `src` builds a
        // GPU context, because the lib's unit tests run in that same binary.
        let lib_is_gpu = repo::rust_files(&dir.join("src"))
            .iter()
            .any(|path| has_gpu_marker(&repo::read(path)));

        // Integration targets are `tests/<name>.rs`. `tests/fixtures/` and
        // friends are modules, not targets, so only regular files at that depth
        // count.
        let mut parallel_targets: Vec<String> = Vec::new();
        let mut gpu_targets: Vec<String> = Vec::new();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir.join("tests"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "rs"))
            .collect();
        entries.sort();
        for path in entries {
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if has_gpu_marker(&repo::read(&path)) {
                gpu_targets.push(name.to_string());
            } else {
                parallel_targets.push(name.to_string());
            }
        }

        // Pass 1: the lib and every non-GPU integration target, at DEFAULT parallelism.
        let mut selection: Vec<String> = Vec::new();
        if !lib_is_gpu {
            selection.push("--lib".to_string());
        }
        for target in &parallel_targets {
            selection.push("--test".to_string());
            selection.push(target.clone());
        }
        if !selection.is_empty() {
            let mut invocation = vec!["-p".to_string(), krate.clone()];
            invocation.extend(selection);
            invocation.extend(cargo_args.iter().cloned());
            run_one(
                &format!("{krate}:parallel-suites"),
                "parallel",
                &invocation,
                timeout,
                &mut rows,
                &mut log,
            );
        }

        // Pass 2: doctests. Their own binary, and they do not build GPU contexts.
        let mut doc = vec!["-p".to_string(), krate.clone(), "--doc".to_string()];
        doc.extend(cargo_args.iter().cloned());
        run_one(
            &format!("{krate}:doc"),
            "parallel",
            &doc,
            timeout,
            &mut rows,
            &mut log,
        );

        // Pass 3: every GPU target on its own, serialised (#1718).
        if lib_is_gpu {
            let mut invocation = vec!["-p".to_string(), krate.clone(), "--lib".to_string()];
            invocation.extend(cargo_args.iter().cloned());
            invocation.push("--".to_string());
            invocation.push("--test-threads=1".to_string());
            run_one(
                &format!("{krate}:lib"),
                "serial(gpu)",
                &invocation,
                timeout,
                &mut rows,
                &mut log,
            );
        }
        for target in &gpu_targets {
            let mut invocation = vec![
                "-p".to_string(),
                krate.clone(),
                "--test".to_string(),
                target.clone(),
            ];
            invocation.extend(cargo_args.iter().cloned());
            invocation.push("--".to_string());
            invocation.push("--test-threads=1".to_string());
            run_one(
                &format!("{krate}:{target}"),
                "serial(gpu)",
                &invocation,
                timeout,
                &mut rows,
                &mut log,
            );
        }
    }

    emit(&rows, &log)
}

/// Runs one cargo invocation and records a row for it.
fn run_one(
    label: &str,
    mode: &'static str,
    cargo_args: &[String],
    timeout: Duration,
    rows: &mut Vec<Row>,
    log: &mut String,
) {
    let mut args = vec!["test".to_string()];
    args.extend(cargo_args.iter().cloned());
    let captured = proc::capture("cargo", &args, Some(timeout));

    let code = captured
        .code
        .map_or_else(|| "killed".to_string(), |code| code.to_string());
    log.push_str(&format!("\n===== {label} [{mode}] exit={code} =====\n"));
    log.push_str(&captured.output);

    // The kill is ours, so a timeout is known rather than inferred from an exit
    // code. That matters: a test binary killed by the budget and one that failed
    // on its own look the same from the outside.
    let status = if captured.timed_out {
        "timeout"
    } else if captured.success() {
        "ok"
    } else {
        "fail"
    };

    let (passed, failed, ignored) = summarise(&captured.output);
    rows.push(Row {
        name: label.to_string(),
        mode,
        passed,
        failed,
        ignored,
        status,
    });
}

/// Sums the counters across every `test result:` line in one cargo run.
///
/// A single invocation can report several of them (a lib target and its
/// doctests, say), so the last line alone would undercount.
fn summarise(output: &str) -> (u64, u64, u64) {
    let (mut passed, mut failed, mut ignored) = (0, 0, 0);
    for line in output.lines() {
        let Some(rest) = line.split_once("test result:") else {
            continue;
        };
        let tokens: Vec<&str> = rest.1.split_whitespace().collect();
        for pair in tokens.windows(2) {
            let Ok(count) = pair[0].parse::<u64>() else {
                continue;
            };
            // libtest writes `12 passed;`, so the label carries a separator.
            match pair[1].trim_end_matches(';') {
                "passed" => passed += count,
                "failed" => failed += count,
                "ignored" => ignored += count,
                _ => {}
            }
        }
    }
    (passed, failed, ignored)
}

/// True if `text` names a constructor that ends up building a wgpu context.
fn has_gpu_marker(text: &str) -> bool {
    GPU_MARKERS.iter().any(|marker| text.contains(marker))
}

/// Resolves a package name to its directory.
///
/// `crates/<name>` covers the library crates. A package whose directory is not
/// its name (`avio-examples` lives in `examples/`) is found by its manifest, so
/// a typo cannot silently report no-such-crate and leave the run vacuous.
fn crate_dir(root: &Path, name: &str) -> Option<PathBuf> {
    for candidate in [root.join("crates").join(name), root.join(name)] {
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let needle = format!("name = \"{name}\"");
    let mut manifests = vec![root.join("Cargo.toml")];
    for parent in [root.join("crates"), root.join("examples")] {
        if let Ok(entries) = std::fs::read_dir(&parent) {
            for entry in entries.flatten() {
                manifests.push(entry.path().join("Cargo.toml"));
            }
        }
        manifests.push(parent.join("Cargo.toml"));
    }
    manifests.sort();
    manifests.dedup();
    manifests
        .into_iter()
        .find(|manifest| {
            repo::read(manifest)
                .lines()
                .any(|line| line.trim_end() == needle)
        })
        .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
}

/// Prints the JSON result and returns the process exit code.
fn emit(rows: &[Row], log: &str) -> u8 {
    let ok = rows.iter().all(|row| row.status == "ok");
    let targets: Vec<String> = rows
        .iter()
        .map(|row| {
            format!(
                "{{\"name\":{},\"mode\":{},\"passed\":{},\"failed\":{},\"ignored\":{},\"status\":{}}}",
                json::quote(&row.name),
                json::quote(row.mode),
                row.passed,
                row.failed,
                row.ignored,
                json::quote(row.status),
            )
        })
        .collect();
    let passed: u64 = rows.iter().map(|row| row.passed).sum();
    let failed: u64 = rows.iter().map(|row| row.failed).sum();
    let ignored: u64 = rows.iter().map(|row| row.ignored).sum();

    println!(
        "{{\"ok\":{},\"targets\":[{}],\"totals\":{{\"passed\":{passed},\"failed\":{failed},\"ignored\":{ignored}}},\"log_tail\":{}}}",
        ok,
        targets.join(","),
        json::quote(&json::log_tail(log, 80)),
    );
    u8::from(!ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarise_should_sum_every_test_result_line() {
        let output = "\
test result: ok. 12 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(summarise(output), (13, 2, 3));
    }

    #[test]
    fn summarise_should_ignore_output_without_a_result_line() {
        assert_eq!(summarise("error: could not compile `avio`"), (0, 0, 0));
    }

    #[test]
    fn has_gpu_marker_should_match_the_indirect_render_context_entry_points() {
        assert!(has_gpu_marker(
            "let ctx = block_on(RenderContext::init())?;"
        ));
        assert!(has_gpu_marker("GpuPreviewCompositor::new(&ctx)"));
        assert!(!has_gpu_marker("let timeline = Timeline::builder();"));
    }
}
