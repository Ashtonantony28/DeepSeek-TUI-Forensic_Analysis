//! Agent engine.
//!
//! Runs as a tokio task driven by an mpsc channel of `Op` values; emits
//! `Event` values out. Implements the upstream turn lifecycle:
//!   1. Pre-turn snapshot (deferred to Phase 3.1 — checkpoint extension)
//!   2. Pre-turn prep: context refresh / cycle / compaction
//!   3. LLM streaming
//!   4. Tool execution (parallel for read-only; serial for destructive)
//!   5. Post-tool LSP hook (deferred to Phase 3 — Phase 2 stubs)
//!   6. Post-turn snapshot (Phase 3.1)
//!
//! Tool deferral and Plan-mode narrowing are handled when building the
//! tool catalog for each turn.

pub mod auto_test;
pub mod compactor;
pub mod engine;
pub mod parser;
pub mod routing;
pub mod session;

pub use compactor::FlashCompactor;
pub use engine::{Engine, EngineHandle};
pub use parser::parse_tool_input;
pub use routing::{classify as classify_prompt, Router, Tier};
pub use session::Session;
