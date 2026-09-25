//! Running child processes and capturing what they print.
//!
//! Two things here are less obvious than they look.
//!
//! **Interleaving.** Several commands need cargo's stdout and stderr in one
//! stream, in the order the child wrote them, the way `2>&1` gives it. Reading
//! two pipes from one thread deadlocks as soon as one of them fills, so both
//! handles are pointed at the same file instead and the file is read afterwards.
//!
//! **Timeouts.** `test` has to kill a target that stops making progress, because
//! the livelock it guards against (#1718) never returns on its own. `std` has no
//! wait-with-deadline, and the usual command-line answer (`timeout`) is not
//! available on every platform this runs on, so the wait is written out here:
//! poll the child, kill it when the budget is spent.

use std::fs::File;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What a captured run produced.
pub struct Captured {
    /// stdout and stderr, interleaved as the child wrote them.
    pub output: String,
    /// The child's exit code, or `None` if it was killed or reported no code.
    pub code: Option<i32>,
    /// True when the run was killed for exceeding its time budget.
    pub timed_out: bool,
}

impl Captured {
    pub fn success(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }
}

/// A private scratch file per capture, so two runs cannot share one.
fn scratch_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("avio-xtask-{}-{n}.log", std::process::id()))
}

/// Runs `program args...`, capturing stdout and stderr together.
///
/// `timeout` of `None` waits as long as the child takes. The command inherits
/// the environment, which is what lets `CARGO_TARGET_DIR` and the rest of the
/// caller's cargo configuration apply.
pub fn capture(program: &str, args: &[String], timeout: Option<Duration>) -> Captured {
    let path = scratch_path();
    let sink = match File::create(&path) {
        Ok(file) => file,
        Err(err) => return failed_to_run(program, &format!("cannot create a capture file: {err}")),
    };
    let sink_err = match sink.try_clone() {
        Ok(file) => file,
        Err(err) => {
            return failed_to_run(
                program,
                &format!("cannot duplicate the capture file: {err}"),
            );
        }
    };

    let spawned = Command::new(program)
        .args(args)
        .stdout(Stdio::from(sink))
        .stderr(Stdio::from(sink_err))
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            let _ = std::fs::remove_file(&path);
            return failed_to_run(program, &err.to_string());
        }
    };

    let mut timed_out = false;
    let status = match timeout {
        None => child.wait().ok(),
        Some(budget) => {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {
                        if start.elapsed() >= budget {
                            timed_out = true;
                            let _ = child.kill();
                            break child.wait().ok();
                        }
                        // Long enough that polling costs nothing, short enough
                        // that the reported duration is not mostly sleep.
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    Err(_) => break None,
                }
            }
        }
    };

    let mut output = String::new();
    if let Ok(mut file) = File::open(&path) {
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_ok() {
            output = String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    let _ = std::fs::remove_file(&path);

    Captured {
        output,
        code: status.and_then(|status| status.code()),
        timed_out,
    }
}

/// The result for a child that never started, shaped like one that failed.
///
/// A missing `cargo` or `git` is reported through the same JSON the caller would
/// print anyway, rather than as a panic, so the message reaches whoever ran the
/// command.
fn failed_to_run(program: &str, reason: &str) -> Captured {
    Captured {
        output: format!("error: could not run `{program}`: {reason}"),
        code: None,
        timed_out: false,
    }
}

/// Runs a command purely for its stdout, trimmed. `None` if it failed to run or
/// exited non-zero.
///
/// This is the `$(...)` of the shell scripts, and it is used the same way: for
/// short `git` queries whose failure means "not applicable here".
pub fn stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The lines of a command's stdout, dropping empty ones.
pub fn stdout_lines(program: &str, args: &[&str]) -> Vec<String> {
    stdout(program, args)
        .map(|text| {
            text.lines()
                .map(str::trim_end)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
