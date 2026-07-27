//! sindon_layout — Taffy-based flexbox layout engine.
//!
//! Provides `LayoutEngine` wrapping `TaffyTree`, plus a `FlexStyle` builder
//! for common flexbox patterns. After `compute()`, each node has a computed
//! `Rect` in absolute coordinates.

mod engine;
mod style;

pub use engine::{LayoutEngine, MeasureQuery};
pub use style::{Align, FlexStyle, Justify};

// Re-export the Taffy NodeId so widgets can hold their layout node handle.
pub use taffy::NodeId as LayoutNodeId;
