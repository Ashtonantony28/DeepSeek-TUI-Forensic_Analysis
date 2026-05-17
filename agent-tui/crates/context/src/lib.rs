//! Three-tier context management.
//!
//! From the build plan (locked-in stack, ANALYSIS.md §4):
//!
//! - `SeamManager` triggers at 192K / 384K / 576K tokens. Append-only.
//!   Preserves the prefix cache; uses a cheap LLM call to summarise.
//! - `CycleManager` triggers at 768K. Hard reset; archives the session
//!   into a `Briefing` prompt.
//! - `Compaction` — destructive fallback. Used only when seam slots
//!   are exhausted.
//! - `CapacityController` — computes `RiskBand` from `context_used_ratio`
//!   and tool volatility. `High` (the upstream's max band) triggers
//!   `VerifyAndReplan`.

pub mod capacity;
pub mod compaction;
pub mod cycle;
pub mod seam;
pub mod tokens;

pub use capacity::{CapacityConfig, CapacityController, CapacityObservation};
pub use compaction::{CompactionConfig, CompactionResult, Compactor};
pub use cycle::{CycleConfig, CycleManager, CycleOutcome};
pub use seam::{SeamConfig, SeamLevel, SeamManager, SeamOutcome};
pub use tokens::estimate_tokens;
