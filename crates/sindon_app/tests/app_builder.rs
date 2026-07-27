//! Smoke tests for the `App` builder.
//!
//! The builder itself never spins up the event loop — `run()` consumes
//! `self` and drives winit, which can't be exercised in a unit test. So
//! these tests only verify the fluent surface (`Default`, chainability,
//! closure signature shape) to catch accidental API regressions.

use std::time::Duration;

use sindon_app::{App, AppScope, system_theme_signal};
use sindon_core::Theme;
use sindon_platform::SystemTheme;
use sindon_reactive::{Reactive, Signal};
use sindon_widgets::shortcut::Shortcut;
use sindon_widgets::tree::WidgetTree;

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
        .image_load_hardening(false)
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
    scope.on_frame(|_ctx| {});
    // Second call must be accepted (replaces the first hook).
    scope.on_frame(|_ctx| {});
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
/// `system_theme_signal_is_singleton` below.
#[allow(dead_code)]
fn _scope_system_surface(scope: &AppScope) -> WidgetTree {
    let sig: Signal<Option<SystemTheme>> = scope.system_theme();
    let _copy: Signal<Option<SystemTheme>> = sig;
    let _loc: Option<String> = scope.system_locale();
    WidgetTree::new()
}

/// Compile-check: `App::theme` accepts every reactive shape that
/// `Reactive::<Theme>::From` provides — a static `Theme`, a
/// `Signal<Theme>` (relies on the relaxed `T: Clone` bound from Phase
/// 30), and a `Reactive::derive` closure that folds the OS theme
/// signal in. Catches accidental tightening of the bound, which would
/// silently break the live-theme-swap call sites in apps.
#[allow(dead_code)]
fn _app_theme_accepts_all_reactive_shapes() {
    let _static = App::new().theme(Theme::light());

    let sig: Signal<Theme> = Signal::new(Theme::dark());
    let _signal_driven = App::new().theme(sig);

    let os_theme = system_theme_signal();
    let derived: Reactive<Theme> = Reactive::derive(move || match os_theme.get() {
        Some(SystemTheme::Light) => Theme::light(),
        _ => Theme::dark(),
    });
    let _derived = App::new().theme(derived);
}

#[test]
fn run_accepts_scope_closure() {
    // Type-level check: `run` must take a closure with the scope shape.
    // Cast is the verification — if the signature drifts, this fails
    // to compile.
    let _f: fn(App, fn(&mut AppScope) -> WidgetTree) = |app, build| app.run(build);
}

#[test]
fn system_theme_signal_is_singleton() {
    // The whole point of moving the signal into a `thread_local!` was
    // to make pre-`run` callers and in-build callers see the same
    // handle. Two calls in any order — even mixed across modules —
    // must hand back identical reactive ids; otherwise an OS theme
    // change update lands on a signal nobody is reading.
    let a = system_theme_signal();
    let b = system_theme_signal();
    assert_eq!(
        a.id(),
        b.id(),
        "system_theme_signal must reuse the thread-local"
    );

    // Writes through one handle must surface through the other, which
    // is what the event-loop ThemeChanged update relies on.
    a.set(Some(SystemTheme::Light));
    assert_eq!(b.get(), Some(SystemTheme::Light));
    a.set(Some(SystemTheme::Dark));
    assert_eq!(b.get(), Some(SystemTheme::Dark));
}
