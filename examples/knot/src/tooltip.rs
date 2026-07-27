//! Hover tooltips (FW-13) — the app-side timing + bubble over the framework's
//! two primitives: [`sindon::widgets::Container::on_hover_enter`] /
//! `on_hover_exit` and [`sindon::widgets::layer::LayerOptions::tooltip`] (a
//! click-through overlay).
//!
//! The framework deliberately stops at "tell me when a container is hovered"
//! and "give me a paint-only overlay". The *delay* — so a tip appears on a
//! deliberate rest, not on every cursor pass-through — lives here, polled by
//! the existing per-frame tick in [`crate::main`] (the same `on_frame` that
//! drives auto-save / auto-lock). State is a thread-local singleton, mirroring
//! [`crate::find_replace::signals`]; the UI runs single-threaded on the
//! event-loop thread.
//!
//! ## Why a click-through layer
//!
//! A tooltip pushed as an ordinary layer would capture all pointer input, so
//! the trigger would never see the `MouseLeave` that dismisses the tip — it
//! would stick or flicker. `LayerOptions::tooltip()` is non-interactive: it
//! paints on top but event routing skips it, so the trigger keeps receiving
//! hover events and [`on_exit`] fires normally.
//!
//! ## Usage
//!
//! Wrap the widget that should show a tip in a [`trigger`] container:
//!
//! ```ignore
//! let cell = tree.add_child(row, tooltip::trigger(i18n::tr(Key::TooltipBold)));
//! tree.add_child(cell, icons::icon_button(Icon::Bold, 18.0).on_click(..));
//! ```
//!
//! The trigger reports its own layout rect on hover, which anchors the bubble
//! directly beneath it.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use sindon::app::FrameContext;
use sindon::core::Rect;
use sindon::widgets::layer::{HAlign, LayerAnchor, LayerOptions, Placement};
use sindon::widgets::{Container, EventContext, TextWidget};

use crate::settings;

/// How long the cursor must rest on a trigger before its tip appears. The
/// per-frame tick (≈`tick_interval`, 500 ms) is the polling granularity, so
/// the *visible* delay is this rounded up to the next tick — a normal tooltip
/// feel rather than an instant flash as the cursor brushes past an icon.
const SHOW_DELAY: Duration = Duration::from_millis(400);

/// Font size of the tip text, a touch smaller than body text.
const TIP_FONT_SIZE: f32 = 13.0;

/// A trigger the cursor is currently resting on, awaiting its delay.
struct Pending {
    /// The trigger's layout rect (viewport coords), to anchor the bubble.
    rect: Rect,
    /// The (already localized) tip text.
    text: String,
    /// When the cursor entered — the tip shows once this is `SHOW_DELAY` old.
    since: Instant,
}

#[derive(Default)]
struct State {
    /// The trigger under the cursor, if any. Cleared on exit.
    pending: Option<Pending>,
    /// Whether a tip layer is currently up. We pop it by "top" rather than by
    /// root, which is sound because a shown tip is always the topmost layer:
    /// any interactive layer (menu, modal, dialog) is opened by a click, and a
    /// click is preceded by the cursor movement that already fired `on_exit`.
    shown: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Build a tooltip trigger: a container that reports hover enter/leave to the
/// controller, with `text` as the tip. Add the widget that should carry the
/// tip as this container's single child — the container hugs it and uses its
/// own layout rect to anchor the bubble.
pub fn trigger(text: impl Into<String>) -> Container {
    let text = text.into();
    Container::row()
        .on_hover_enter(move |rect, ctx| on_enter(rect, text.clone(), ctx))
        .on_hover_exit(on_exit)
}

/// Record the hovered trigger, starting its show-delay clock. If a tip is
/// somehow still up (a stray enter with no matching exit), dismiss it first so
/// we never strand a bubble for a trigger the cursor has left.
fn on_enter(rect: Rect, text: String, ctx: &mut EventContext) {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        if st.shown {
            ctx.pop_top_layer();
            st.shown = false;
        }
        st.pending = Some(Pending {
            rect,
            text,
            since: Instant::now(),
        });
    });
}

/// The cursor left the trigger: drop the pending clock and pop any shown tip.
fn on_exit(ctx: &mut EventContext) {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.pending = None;
        if st.shown {
            ctx.pop_top_layer();
            st.shown = false;
        }
    });
}

/// Per-frame poll, called from the `on_frame` tick in `main`. Once the hovered
/// trigger has rested for `SHOW_DELAY`, push the bubble as a click-through
/// tooltip layer anchored beneath it. No-op when nothing is hovered, when a
/// tip is already up, or before the delay elapses — so it costs ~one
/// `borrow()` on an idle frame.
pub fn tick(frame: &mut FrameContext) {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        if st.shown {
            return;
        }
        let Some(pending) = &st.pending else {
            return;
        };
        if pending.since.elapsed() < SHOW_DELAY {
            return;
        }
        let rect = pending.rect;
        let text = pending.text.clone();
        frame.event_ctx.push_layer(
            LayerOptions::tooltip().anchor(LayerAnchor::AnchorRect {
                rect,
                // Below the toolbar normally; flips above near the viewport
                // bottom so a tip on a low trigger stays on screen.
                prefer: Placement::Auto,
                align: HAlign::Start,
            }),
            bubble(),
            move |tree, root| {
                tree.add_child(
                    root,
                    TextWidget::new(text)
                        .font_size(TIP_FONT_SIZE)
                        .color(settings::on_surface()),
                );
            },
        );
        st.shown = true;
    });
}

/// Drop all tooltip state without popping (the layer, if any, is already gone).
/// Call when the screen is rebuilt out from under the controller — e.g. an
/// auto-lock `replace_screen` fired while a tip was up, tearing down every
/// layer. Without this the stale `shown` flag would suppress all future tips.
pub fn reset() {
    STATE.with(|s| *s.borrow_mut() = State::default());
}

/// The bubble root: a small, raised, rounded surface that holds the tip text.
fn bubble() -> Container {
    Container::row()
        .padding(6.0)
        .radius(6.0)
        .background(settings::surface())
}
