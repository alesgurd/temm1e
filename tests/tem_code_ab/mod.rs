//! Tem-Code A/B Testing Infrastructure
//!
//! Compares OLD toolset (file_read + file_write + shell) vs NEW toolset
//! (code_edit + code_glob + code_grep + code_patch + code_snapshot)
//! across three metrics: token usage, token efficiency, harmful behavior.

pub mod benchmark;
pub mod metrics;
pub mod scenarios;
