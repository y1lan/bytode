//! Replay-to-context rendering. No disk IO and no compact policy live here.

pub mod render;

pub use render::{RenderedEntry, render_context_entries};
