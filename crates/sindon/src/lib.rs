//! sindon — Secret-aware Rust UI framework.
//!
//! A zeroize-first, GPU-rendered UI framework (no WebView) designed for
//! applications that handle sensitive data (passwords, keys, messages).
//!
//! This crate is a facade — each submodule re-exports one of the workspace
//! crates. Most applications only need [`app`] and [`widgets`].

pub use sindon_app as app;
pub use sindon_core as core;
pub use sindon_layout as layout;
pub use sindon_platform as platform;
pub use sindon_reactive as reactive;
pub use sindon_render as render;
pub use sindon_security as security;
pub use sindon_text as text;
pub use sindon_widgets as widgets;
