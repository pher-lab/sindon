//! Clock — demonstrates `AppHandle::wake()` driving redraws from a
//! background thread.
//!
//! A worker thread wakes the UI twice a second. Each wake schedules a
//! redraw, which re-runs the `TextWidget::reactive` closure and pulls
//! the current `Instant`. No signals are touched off-thread; the handle
//! only carries "please refresh" across the boundary.

use std::thread;
use std::time::{Duration, Instant};

use shroud::app::App;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, TextWidget};

fn main() {
    App::new()
        .title("shroud — clock")
        .size(420, 240)
        .run(|handle| {
            // Spawn the waker on a detached thread. The handle is `Clone`, so
            // each producer gets its own proxy and they can all push toward
            // the same event loop.
            let waker = handle.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(500));
                    waker.wake();
                }
            });

            let start = Instant::now();

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .center()
                    .gap(12.0),
            );

            tree.add_child(root, TextWidget::new("shroud clock").font_size(24.0));

            // Reactive label — pulled on every paint, which happens after
            // every wake. The closure runs on the UI thread; `Instant` is
            // cheap enough to read here.
            tree.add_child(
                root,
                TextWidget::reactive(move || {
                    let secs = start.elapsed().as_secs();
                    format!("elapsed: {}s", secs)
                })
                .font_size(40.0),
            );

            tree
        });
}
