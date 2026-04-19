//! shroud — Secret-aware Rust UI framework.
//!
//! A zeroize-first, GPU-rendered UI framework (no WebView) designed for
//! applications that handle sensitive data (passwords, keys, messages).
//!
//! This crate is a facade — each submodule re-exports one of the workspace
//! crates. Most applications only need [`app`] and [`widgets`].

pub use shroud_app as app;
pub use shroud_core as core;
pub use shroud_layout as layout;
pub use shroud_platform as platform;
pub use shroud_reactive as reactive;
pub use shroud_render as render;
pub use shroud_security as security;
pub use shroud_text as text;
pub use shroud_widgets as widgets;
