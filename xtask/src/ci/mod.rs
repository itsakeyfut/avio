//! Tasks the GitHub workflows run.
//!
//! Their exit code is the gate, so each one reports every problem it found
//! before returning rather than stopping at the first, and prints the reasons to
//! stderr where a failed job shows them.

pub mod changelog;
pub mod publish_readiness;
