//! The full local gate, in one command.
//!
//! Not a convenience. It is the definition of "done" from `CLAUDE.md` in one
//! place, so that a claim that the gate passed is a claim about **this command's
//! exit code** rather than about which five commands somebody remembered to run.
//!
//! Every step runs even after one fails, because the cheap steps are the ones
//! whose output is easiest to act on and stopping at the first would hide them.
//! The order is cheapest-to-fail first so that a broken build is reported before
//! the test run spends ten minutes reaching the same conclusion.
//!
//! **Scoped to crates, deliberately.** A workspace-wide `cargo test` saturates
//! this machine, which is why `CLAUDE.md` forbids it locally; the gate therefore
//! takes the same `-p` arguments as `xtask test` and leaves the full sweep to CI.

use std::time::Duration;

use crate::proc;

/// Matches the CI docs job, which fails on a doc warning or a broken intra-doc
/// link (`.github/workflows/ci.yml`).
const DOC_FLAGS: &str = "-D warnings";
/// `rust-version` in `[workspace.package]`, which the CI MSRV job pins.
const MSRV: &str = "1.93.0";

struct Step {
    name: &'static str,
    status: Status,
    detail: String,
}

enum Status {
    Ok,
    Failed,
    Skipped,
}

pub fn run(args: &[String]) -> u8 {
    let mut crates: Vec<String> = Vec::new();
    let mut timeout: Option<String> = None;
    let mut tests = true;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" => {
                let Some(name) = args.get(i + 1) else {
                    eprintln!("error: {} needs a crate name", args[i]);
                    return 2;
                };
                crates.push(name.clone());
                i += 2;
            }
            // `commits` runs this gate once per commit of a branch, where the test
            // step would dominate the wall clock and answer a question the tip's
            // own run already answers. What is left is still the whole
            // compile-and-lint gate.
            "--no-tests" => {
                tests = false;
                i += 1;
            }
            "--timeout" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: --timeout needs a number of seconds");
                    return 2;
                };
                timeout = Some(value.clone());
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument `{other}`");
                return 2;
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

    let packages: Vec<String> = crates
        .iter()
        .flat_map(|name| ["-p".to_string(), name.clone()])
        .collect();

    let mut steps = Vec::new();
    let mut log = String::new();

    // Instant, and the one step that is workspace-wide: a formatting failure in
    // a crate nobody named is still a failure CI will report.
    steps.push(cargo(
        "fmt",
        &owned(&["fmt", "--all", "--", "--check"]),
        &mut log,
    ));

    // The CI shape: `--all-features` without `--all-targets`, so test-only lints
    // do not gate here either (RK-006).
    let mut clippy = owned(&["clippy"]);
    clippy.extend(packages.iter().cloned());
    clippy.extend(owned(&["--all-features", "--", "-D", "warnings"]));
    steps.push(cargo("clippy", &clippy, &mut log));

    // `--all-targets` where clippy does not have it: `cargo test` compiles only
    // `src/` and `tests/`, so a struct literal in `examples/` breaks in CI and
    // nowhere else (`CLAUDE.md`, Struct Field Changes).
    let mut check = owned(&["check"]);
    check.extend(packages.iter().cloned());
    check.extend(owned(&["--all-targets", "--all-features"]));
    steps.push(cargo("check", &check, &mut log));

    let mut doc = owned(&["doc"]);
    doc.extend(packages.iter().cloned());
    doc.extend(owned(&["--all-features", "--no-deps"]));
    steps.push(cargo_with_env(
        "doc",
        &doc,
        &[("RUSTDOCFLAGS", DOC_FLAGS)],
        &mut log,
    ));

    steps.push(msrv(&packages, &mut log));

    // Re-executes this binary rather than reimplementing `xtask test`: the GPU
    // targets have to run one at a time or a wgpu-heavy target livelocks (#1718),
    // and that logic should exist once.
    if tests {
        steps.push(test(&crates, timeout.as_deref(), &mut log));
    }

    report(&steps, &log)
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

fn cargo(name: &'static str, args: &[String], log: &mut String) -> Step {
    cargo_with_env(name, args, &[], log)
}

fn cargo_with_env(
    name: &'static str,
    args: &[String],
    env: &[(&str, &str)],
    log: &mut String,
) -> Step {
    record(name, "cargo", args, env, None, log)
}

/// The MSRV check, skipped rather than failed when the pinned toolchain is absent.
///
/// A developer without `1.93.0` installed would otherwise see a failure that says
/// nothing about their change, and would learn to ignore the gate.
fn msrv(packages: &[String], log: &mut String) -> Step {
    let probe = proc::capture(
        "cargo",
        &owned(&[&format!("+{MSRV}"), "--version"]),
        Some(Duration::from_secs(60)),
    );
    if !probe.success() {
        return Step {
            name: "msrv",
            status: Status::Skipped,
            detail: format!("toolchain {MSRV} is not installed (rustup toolchain install {MSRV})"),
        };
    }
    let mut args = owned(&[&format!("+{MSRV}"), "check"]);
    args.extend(packages.iter().cloned());
    args.push("--all-targets".to_string());
    record("msrv", "cargo", &args, &[], None, log)
}

fn test(crates: &[String], timeout: Option<&str>, log: &mut String) -> Step {
    let Ok(exe) = std::env::current_exe() else {
        return Step {
            name: "test",
            status: Status::Skipped,
            detail: "cannot locate this executable to re-run it".to_string(),
        };
    };
    let mut args = vec!["test".to_string()];
    for name in crates {
        args.push("-p".to_string());
        args.push(name.clone());
    }
    if let Some(secs) = timeout {
        args.push("--timeout".to_string());
        args.push(secs.to_string());
    }
    record(
        "test",
        &exe.to_string_lossy(),
        &args,
        &[],
        // The test task enforces its own per-target budget and reports a hang as a
        // `timeout` row, so a budget here would only cut that reporting short.
        None,
        log,
    )
}

fn record(
    name: &'static str,
    program: &str,
    args: &[String],
    env: &[(&str, &str)],
    timeout: Option<Duration>,
    log: &mut String,
) -> Step {
    let captured = proc::capture_with_env(program, args, timeout, env);
    let code = captured
        .code
        .map_or_else(|| "killed".to_string(), |code| code.to_string());
    log.push_str(&format!("\n===== {name} exit={code} =====\n"));
    log.push_str(&captured.output);

    let detail = tail(&captured.output, 20);
    Step {
        name,
        status: if captured.success() {
            Status::Ok
        } else {
            Status::Failed
        },
        detail,
    }
}

/// The last `lines` lines, which is where a cargo failure says what went wrong.
fn tail(output: &str, lines: usize) -> String {
    let collected: Vec<&str> = output.lines().collect();
    let start = collected.len().saturating_sub(lines);
    collected[start..].join("\n")
}

/// Prints one line per step and the failing steps' output, and returns the exit
/// code: non-zero when any step failed.
///
/// Plain text rather than the JSON the review aids print, because this one is read
/// by a person deciding whether they are done.
fn report(steps: &[Step], log: &str) -> u8 {
    println!("gate:");
    for step in steps {
        match step.status {
            Status::Ok => println!("  ok    {}", step.name),
            Status::Skipped => println!("  skip  {}  ({})", step.name, step.detail),
            Status::Failed => println!("  FAIL  {}", step.name),
        }
    }

    let failed: Vec<&Step> = steps
        .iter()
        .filter(|step| matches!(step.status, Status::Failed))
        .collect();
    for step in &failed {
        println!("\n----- {} -----", step.name);
        for line in step.detail.lines() {
            println!("      {line}");
        }
    }

    if failed.is_empty() {
        println!("\ngate: passed");
        0
    } else {
        let names: Vec<&str> = failed.iter().map(|step| step.name).collect();
        println!("\ngate: FAILED ({})", names.join(", "));
        let _ = log;
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_should_return_the_last_lines() {
        assert_eq!(tail("a\nb\nc\nd", 2), "c\nd");
    }

    #[test]
    fn tail_should_return_everything_when_there_are_fewer_lines_than_asked_for() {
        assert_eq!(tail("a\nb", 10), "a\nb");
    }

    #[test]
    fn owned_should_preserve_order() {
        assert_eq!(owned(&["a", "b"]), vec!["a".to_string(), "b".to_string()]);
    }
}
