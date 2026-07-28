//! sindon_app — Application entry point and winit event loop integration.
//!
//! Provides [`App`], a fluent builder that drives the winit event loop and
//! wires the widget tree to the renderer. Process-level hardening
//! (core dump disable, ptrace protection, exploit mitigation) is applied
//! on `run()` by default.
//!
//! The build closure receives an [`AppScope`] with access to the
//! thread-safe [`AppHandle`] (whose `wake()` kicks redraws from external
//! timers or async tasks) and [`AppScope::on_frame`] for per-frame tick
//! callbacks that run on the UI thread.

pub mod a11y;
pub mod event_loop;
pub mod perf;

pub use event_loop::{
    App, AppError, AppErrorKind, AppHandle, AppScope, FrameContext, system_theme_signal,
    theme_color, theme_value,
};
pub use perf::{FRAME_BUDGET, FrameTimings, PerfSnapshot};
