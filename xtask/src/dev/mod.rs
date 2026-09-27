//! Tasks a contributor runs directly.
//!
//! These are the commands referenced from `CLAUDE.md` and from the contributor
//! documentation. Their output is read by a person, so they may be chatty, and
//! their exit code is the answer to "did it pass".

pub mod adr_check;
pub mod commits;
pub mod dep_graph;
pub mod gate;
pub mod test;
