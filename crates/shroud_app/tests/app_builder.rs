//! Smoke tests for the `App` builder.
//!
//! The builder itself never spins up the event loop — `run()` consumes
//! `self` and drives winit, which can't be exercised in a unit test. So
//! these tests only verify the fluent surface (`Default`, chainability,
//! closure signature shape) to catch accidental API regressions.

use std::time::Duration;

use shroud_app::{App, AppScope};
use shroud_platform::SystemTheme;
use shroud_widgets::shortcut::Shortcut;
use shroud_widgets::tree::WidgetTree;

#[test]
fn builder_default_constructs() {
    let _app = App::new();
    let _app2: App = App::default();
}

#[test]
fn builder_methods_chain() {
    let _app = App::new()
        .title("test")
        .size(1024, 768)
        .capture_prevention(true)
        .disable_core_dumps(false)
        .ptrace_protection(false)
        .exploit_mitigation(false)
        .tick_interval(Duration::from_millis(100));
}

#[test]
fn builder_title_accepts_string_and_str() {
    let _a = App::new().title("literal");
    let _b = App::new().title(String::from("owned"));
}

/// Compile-check: the `run` closure accepts an `&mut AppScope`, and the
/// scope exposes `handle()` + `on_frame()`. Never actually called
/// because `run()` drives winit.
#[allow(dead_code)]
fn _scope_surface(scope: &mut AppScope) -> WidgetTree {
    let _handle = scope.handle().clone();
    scope.on_frame(|| {});
    // Second call must be accepted (replaces the first hook).
    scope.on_frame(|| {});
    WidgetTree::new()
}

/// Compile-check: `AppScope::on_shortcut` accepts a `Shortcut` and a
/// `FnMut(&mut ShortcutContext)`, and returns a `ShortcutId`. Doesn't
/// run (needs a live event loop) — drift in the signature breaks the
/// build.
#[allow(dead_code)]
fn _scope_shortcut_surface(scope: &mut AppScope) -> WidgetTree {
    let _id = scope.on_shortcut(Shortcut::ctrl('l'), |_ctx| {});
    WidgetTree::new()
}

/// Compile-check: `AppScope::system_theme` returns a clonable
/// `Signal<Option<SystemTheme>>`, and `system_locale` returns
/// `Option<String>`. Catches accidental signature drift on the
/// detection APIs added in the A-13/A-14 phase. The memoization
/// contract (two calls return the same `Signal` id) is unit-tested in
/// the event_loop module where `AppScope::new` is reachable without
/// spinning up winit.
#[allow(dead_code)]
fn _scope_system_surface(scope: &mut AppScope) -> WidgetTree {
    let sig: shroud_reactive::Signal<Option<SystemTheme>> = scope.system_theme();
    let _copy: shroud_reactive::Signal<Option<SystemTheme>> = sig;
    let _loc: Option<String> = scope.system_locale();
    WidgetTree::new()
}

#[test]
fn run_accepts_scope_closure() {
    // Type-level check: `run` must take a closure with the scope shape.
    // Cast is the verification — if the signature drifts, this fails
    // to compile.
    let _f: fn(App, fn(&mut AppScope) -> WidgetTree) = |app, build| app.run(build);
}
