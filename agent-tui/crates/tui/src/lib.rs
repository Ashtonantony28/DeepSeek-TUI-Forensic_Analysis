//! Terminal UI built on ratatui + crossterm.
//!
//! Five panels per the plan:
//!   - Composer (bottom)
//!   - Transcript (centre, scrollable)
//!   - Plan/reflection side panel (right; empty until Phase 3.7)
//!   - Status bar (top)
//!   - Command palette (Ctrl+K modal)
//!
//! Keys: F1 help, Esc backs out, Tab/Shift+Tab cycles Plan/Agent/YOLO,
//! Ctrl+K palette, Ctrl+C cancels current turn.

pub mod app;
pub mod render;

pub use app::{run_tui, App};
