//! Trait-bound sanity checks for `AppHandle`.
//!
//! `AppHandle` must be `Clone + Send` so it can be handed to background
//! threads that drive the UI via `wake()`. These are compile-time
//! assertions — they don't instantiate a real handle (which requires a
//! live event loop and can only exist on the main thread).

use shroud_app::AppHandle;

fn assert_send<T: Send>() {}
fn assert_clone<T: Clone>() {}

#[test]
fn app_handle_is_send() {
    assert_send::<AppHandle>();
}

#[test]
fn app_handle_is_clone() {
    assert_clone::<AppHandle>();
}
