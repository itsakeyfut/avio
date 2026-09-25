//! Tasks that answer a question a reviewer asks about a change.
//!
//! What does this change touch, does it build, does it lint, how much `unsafe`
//! does it carry. Reading a diff answers all four eventually; these answer them
//! in one command, and the same way every time.
//!
//! Each prints one JSON object on stdout and nothing else. That suits a tool
//! reading the result, and it also suits a person: the fields are the summary,
//! and `log_tail` is the part worth reading when a field says something is
//! wrong. Each one exits non-zero when what it measured is not clean, so it
//! works as a gate too.

pub mod build;
pub mod clippy;
pub mod diff_scope;
pub mod unsafe_count;
