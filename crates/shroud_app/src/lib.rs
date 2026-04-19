//! shroud_app — Application entry point and winit event loop integration.
//!
//! Provides [`App`], a fluent builder that drives the winit event loop and
//! wires the widget tree to the renderer. Process-level hardening
//! (core dump disable, ptrace protection, exploit mitigation) is applied
//! on `run()` by default.
//!
//! [`AppHandle`] exposes `wake()` for external timers / async tasks that
//! need to trigger a redraw.

pub mod event_loop;

pub use event_loop::{App, AppHandle};
