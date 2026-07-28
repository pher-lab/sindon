//! sindon — Secret-aware Rust UI framework.
//!
//! A zeroize-first, GPU-rendered UI framework (no WebView) designed for
//! applications that handle sensitive data (passwords, keys, messages).
//!
//! This crate is the facade: it names the surface an *application* builds
//! against. Most applications only need [`app`] and [`widgets`].
//!
//! # Quickstart
//!
//! A window with centered text — the whole program:
//!
//! ```no_run
//! use sindon::app::App;
//! use sindon::core::Color;
//! use sindon::widgets::tree::WidgetTree;
//! use sindon::widgets::{Container, TextWidget};
//!
//! App::new()
//!     .title("hello")
//!     .size(800, 600)
//!     .run(|_scope| {
//!         let mut tree = WidgetTree::new();
//!         let root = tree.set_root(Container::column().width(800.0).height(600.0).center());
//!         tree.add_child(
//!             root,
//!             TextWidget::new("Hello, sindon!")
//!                 .font_size(32.0)
//!                 .color(Color::rgb(0.2, 0.8, 0.7)),
//!         );
//!         tree
//!     });
//! ```
//!
//! [`App::run`](app::App::run) takes the window over and returns when it
//! closes. From there, [`widgets`] holds the widget set and [`reactive`] the
//! signals that drive it; secrets live in [`security`]'s types and reach the
//! screen through `SecureInput` / `SecureText`.
//!
//! One note for reading the rest of these docs: the per-item examples live on
//! the crates re-exported below, so they are written as
//! `sindon_widgets::Container` rather than `sindon::widgets::Container`. Those
//! are the same item — a crate depending on `sindon` spells it the second way.
//!
//! # What this facade promises
//!
//! Under the hood sindon is nine crates, and several of them carry
//! third-party types in their public API — `wgpu` in the renderer, `winit` in
//! the window, `taffy` in the layout engine, `accesskit` in the a11y bridge,
//! and the vendored cosmic-text fork in the shaper. Those types are load
//! bearing for someone *integrating* sindon into an existing renderer or
//! event loop, and irrelevant to someone *writing an app* with it.
//!
//! So the rule this module follows is:
//!
//! > **No third-party type an application would have to name or construct
//! > appears in `sindon::*`.**
//!
//! Crates whose entire public API is sindon's own vocabulary are re-exported
//! whole. The remaining five are re-exported item by item, leaving the
//! integrator-level surface out. Nothing is deleted by this: depend on
//! `sindon_render`, `sindon_platform`, `sindon_text`, `sindon_layout` or
//! `sindon_app` directly and the full surface is there.
//!
//! The practical payoff is version freedom. A `wgpu` or `winit` bump is a
//! breaking change for whoever names those types; because no path through
//! this facade does, it is not a breaking change for applications.
//!
//! Two deliberate exceptions, both third-party types an application never
//! names or constructs:
//!
//! - [`security`] re-exports `zeroize` **on purpose** — `SecureSignal<T>`
//!   bounds `T: Zeroize`, so downstream code has to reach the exact copy
//!   sindon links against. See [`sindon_security::zeroize`].
//! - [`reactive`]'s `ReactiveId` is a `slotmap` key. It is an opaque handle
//!   returned by `Signal::id()`; the `slotmap` trait it implements is never
//!   named by app code, and there is no way to construct one.

// ── Re-exported whole: no third-party type in their public API ─────────

pub use sindon_core as core;
pub use sindon_reactive as reactive;
pub use sindon_security as security;
pub use sindon_widgets as widgets;

// ── Curated: the crate's own surface, minus the integrator-level part ──

/// Flexbox layout vocabulary.
///
/// The engine itself (`LayoutEngine`, `MeasureQuery`, and the `LayoutNodeId`
/// that is a `taffy::NodeId`) stays in `sindon_layout`: a widget describes
/// itself with a [`FlexStyle`](layout::FlexStyle) and never touches the
/// engine, which is what keeps `taffy` out of this facade.
pub mod layout {
    pub use sindon_layout::{Align, FlexStyle, Justify};
}

/// Images and the paint-side value types a widget hands back.
///
/// The renderer proper (`Renderer`, `TextureAtlas`, `SecureTextureAtlas` and
/// the draw commands) stays in `sindon_render`, because its constructors take
/// `wgpu::Device` / `wgpu::Queue`. Painting from a widget goes through
/// [`widgets::PaintContext`], whose methods speak only in sindon types.
pub mod render {
    pub use sindon_render::{
        DecodedImage, GlyphRotation, ImageError, ImageId, LayerSnapshot, encode_png,
    };
}

/// Text shaping vocabulary: attributes, spans, and the shared engine.
///
/// The cosmic-text re-exports (`Attrs`, `CacheKey`, `FontSystem`, `Metrics`,
/// `Shaping`) stay in `sindon_text`. A widget reaches shaped output through
/// [`TextEngine`](text::TextEngine) and passes `ShapedGlyph::cache_key`
/// straight back to `rasterize`, so it never has to name a cosmic-text type.
pub mod text {
    pub use sindon_text::{
        ComposedBlock, DecorationLine, EditBuffer, FontStyle, FontWeight, GlyphImage, ShapedGlyph,
        ShapedText, SpanBox, TextAttrs, TextDecoration, TextEngine, TextFamily, TextSpan,
    };
}

/// OS integration: clipboard, dialogs, config storage, locale and theme.
///
/// `PlatformWindow` and `DisplayProtection` stay in `sindon_platform` — both
/// are built from an `Arc<winit::Window>`. An app turns capture prevention on
/// with [`App::capture_prevention`](app::App::capture_prevention) instead.
pub mod platform {
    pub use sindon_platform::storage;
    pub use sindon_platform::{
        ClipboardError, ClipboardImage, FileDialog, SecureClipboard, SystemTheme, caret_blink_time,
        config_dir, read_json, system_locale, write_json_atomic,
    };
}

/// Application entry point: the builder, its scope, and frame timing.
///
/// The `a11y` module stays in `sindon_app`; it is the one place `accesskit`
/// types reach a signature, and it is a translation layer rather than app
/// API. Screen-reader support is turned on with
/// [`App::accessibility`](app::App::accessibility).
pub mod app {
    pub use sindon_app::{
        App, AppHandle, AppScope, FRAME_BUDGET, FrameContext, FrameTimings, PerfSnapshot,
        system_theme_signal, theme_color, theme_value,
    };
}
