//! Tool trait + built-in tools.
//!
//! Hard rule (Phase 2): every edit/write/apply_patch tool runs a
//! tree-sitter syntax check on the resulting file before returning
//! success. If the language is recognised and parsing fails, the tool
//! returns an error to the model with the parse error position.

pub mod checkpoint;
pub mod memory;
pub mod registry;
pub mod schema;
pub mod syntax;
pub mod tools;

pub use checkpoint::{CheckpointError, CheckpointMeta, CheckpointStore};
pub use memory::{Lesson, MemoryError, MemoryStore};
pub use registry::{ToolContext, ToolRegistry};
pub use schema::ToolSchema;
pub use tools::{Tool, ToolError, ToolResult};
