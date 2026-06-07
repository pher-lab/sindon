//! Transient, dismissable status banner for user-facing failures.
//!
//! Knot's background work (the auto-save tick, deletes, imports, exports,
//! image inserts) used to fail silently to `stderr` — fine for a developer
//! at a terminal, useless to the user. This module is a single
//! app-wide slot for "something the user should see went wrong": call
//! [`show`] with a short message and the vault screen's banner (built in
//! `vault_screen`) renders it with a dismiss button; [`dismiss`] (or the next
//! [`show`]) clears it.
//!
//! Backed by a thread-local [`Signal`], mirroring `settings::signals` and
//! `shroud::app::system_theme_signal` — the UI runs single-threaded on the
//! event-loop thread, so a thread-local is the natural shared slot and lets
//! any module raise a notice without threading a handle through every builder.
//! Messages are non-secret (error summaries, never note contents), so a plain
//! `String` is fine here — unlike the vault, this never holds key material.

use std::cell::OnceCell;

use shroud::reactive::Signal;

thread_local! {
    /// The current banner message, or `None` when nothing is showing. Lazily
    /// initialized on first access (after the reactive runtime exists, same as
    /// `settings::signals`).
    static NOTICE: OnceCell<Signal<Option<String>>> = const { OnceCell::new() };
}

/// The shared banner signal. `Copy` (a `Signal` is a cheap id), so callers can
/// read it into reactive closures freely.
pub fn signal() -> Signal<Option<String>> {
    NOTICE.with(|c| *c.get_or_init(|| Signal::new(None)))
}

/// Raise a banner with `msg`, replacing any message already showing. Also
/// echoes to `stderr` so the failure still lands in logs for a developer.
pub fn show(msg: impl Into<String>) {
    let msg = msg.into();
    eprintln!("knot: {msg}");
    signal().set(Some(msg));
}

/// Clear the banner. Called by the banner's dismiss button and on entering the
/// vault screen (so a stale message from a previous session doesn't linger).
pub fn dismiss() {
    signal().set(None);
}
