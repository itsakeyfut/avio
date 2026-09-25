//! `cargo xtask build`: a build, summarised as JSON.
//!
//! Emits `{ok, exit_code, error_count, log_tail}`. Any arguments are passed
//! through to cargo, which is how the build gets scoped: `cargo xtask build -p
//! ff-render`. With no arguments it builds every target in the current package
//! selection, which over a whole workspace is heavy, so scoping is the norm.

use crate::json;
use crate::proc;

pub fn run(args: &[String]) -> u8 {
    let mut cargo_args = vec![
        "build".to_string(),
        "--all-targets".to_string(),
        // The short format keeps one diagnostic on one line, which is what makes
        // the counts below meaningful and the tail worth reading.
        "--message-format=short".to_string(),
    ];
    cargo_args.extend(args.iter().cloned());

    let captured = proc::capture("cargo", &cargo_args, None);
    let errors = captured
        .output
        .lines()
        .filter(|line| line.starts_with("error"))
        .count();

    println!(
        "{{\"ok\":{},\"exit_code\":{},\"error_count\":{errors},\"log_tail\":{}}}",
        captured.success(),
        captured.code.unwrap_or(-1),
        json::quote(&json::log_tail(&captured.output, 60)),
    );
    u8::from(!captured.success())
}
