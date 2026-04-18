//! Smoke tests for the `App` builder.
//!
//! The builder itself never spins up the event loop — `run()` consumes
//! `self` and drives winit, which can't be exercised in a unit test. So
//! these tests only verify the fluent surface (`Default`, chainability)
//! to catch accidental API regressions.

use shroud_app::App;

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
        .exploit_mitigation(false);
}

#[test]
fn builder_title_accepts_string_and_str() {
    let _a = App::new().title("literal");
    let _b = App::new().title(String::from("owned"));
}
