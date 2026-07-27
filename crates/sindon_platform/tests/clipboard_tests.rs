use sindon_platform::SecureClipboard;
use std::time::Duration;

#[test]
fn clipboard_default_not_active() {
    let clip = SecureClipboard::new();
    assert!(!clip.is_timer_active());
}

#[test]
fn clipboard_timer_management() {
    let mut clip = SecureClipboard::new().with_auto_clear(Duration::from_millis(50));

    assert!(!clip.is_timer_active());

    // No timer active — tick should return false
    assert!(!clip.tick());
}

#[test]
fn clipboard_time_remaining_none_when_inactive() {
    let clip = SecureClipboard::new();
    assert!(clip.time_remaining().is_none());
}
