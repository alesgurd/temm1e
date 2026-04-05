//! # TemDOS — Tem Delegated Operating Subsystem
//!
//! Specialist sub-agents (Cores) that run as tools within TEMM1E's main agent loop.
//! Inspired by GLaDOS's personality cores from Portal — a central consciousness
//! with specialist modules that feed information back.
//!
//! ## Architecture
//!
//! - **Main Agent** is the sole decision-maker (the central consciousness)
//! - **Cores** are specialist sub-agents that inform but never steer
//! - Each Core runs its own LLM tool loop in isolation
//! - Cores share the main agent's budget (no separate allocation)
//! - Cores have full tool access EXCEPT `invoke_core` (no recursion)
//! - Multiple Cores can run in parallel
//!
//! ## The One Invariant
//!
//! > The Main Agent is the sole decision-maker. Cores inform. Cores never steer.

pub mod definition;
pub mod invoke_tool;
pub mod registry;
pub mod runtime;
pub mod types;

pub use definition::CoreDefinition;
pub use invoke_tool::InvokeCoreTool;
pub use registry::CoreRegistry;
pub use runtime::CoreRuntime;
pub use types::{CoreResult, CoreStats};
