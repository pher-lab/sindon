//! Editor formatting toolbar — a row of buttons that insert Markdown syntax
//! into the body at the caret.
//!
//! This is the feature-parity port of Knot v0.7.0's `Editor/Toolbar`. Each
//! button performs one of two kinds of edit:
//!
//! * **Inline** (Bold / Italic / Code) inserts an *empty* pair of the matching
//!   Markdown markers (`****`, `**`, `` `` ``) and drops the caret *between*
//!   them, so the user types the content directly and it comes out wrapped.
//!   `Input` has no selection, so inserting a placeholder we could "select"
//!   isn't possible — empty markers sidestep that and leave nothing to delete.
//! * **Link** inserts `[](https://)` with the caret inside the `[]`, so the
//!   user types the link text directly; the `https://` is a prefix to continue
//!   the URL from (arrow right past `](`).
//! * **Line prefix** (Heading / Quote / List) inserts `# ` / `> ` / `- ` at the
//!   start of the caret's line.
//!
//! ## How it reaches the caret without a selection model
//!
//! `Input` exposes only a caret (no selection range), and a toolbar button is a
//! *sibling* of the body input — it can't reach into it directly. The bridge is
//! two signals shared with the body input ([`crate::editor`] builds them):
//!
//! * `body_sig: Signal<String>` — the body text (already used for note save).
//! * `cursor_sig: Signal<usize>` — the body's caret byte offset, mirrored by
//!   [`shroud::widgets::Input::cursor_signal`].
//!
//! When a button is clicked the body input first loses focus, and `Input`
//! mirrors its final caret into `cursor_sig` on that `FocusLost` — *before* this
//! button's click handler runs. So [`apply`] reads the correct pre-blur caret,
//! rewrites `body_sig`, writes the new caret back into `cursor_sig`, and
//! re-focuses the body (`ctx.focus`) so the caret it just set becomes live and
//! the user keeps typing where the snippet landed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::reactive::Signal;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, EventContext};

use crate::i18n::{self, Key};
use crate::state::AppState;

/// Build the toolbar row into `parent` (the editor's column). `body_sig` and
/// `cursor_sig` are the body input's bound text/caret signals; `body_idx` is a
/// cell the caller fills with the body input's node index after it's built, so
/// the handlers can re-focus it (the toolbar is built *before* the body so it
/// renders above it, hence the deferred index).
pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    body_sig: Signal<String>,
    cursor_sig: Signal<usize>,
    body_idx: Rc<Cell<usize>>,
) {
    // `flex_wrap` so a narrow editor pane (e.g. while the preview is open)
    // wraps the buttons onto a second row instead of overflowing.
    let row = tree.add_child(parent, Container::row().flex_wrap(true).gap(6.0));

    for (label, fmt) in [
        (Key::ToolbarHeading, Fmt::Heading),
        (Key::ToolbarBold, Fmt::Bold),
        (Key::ToolbarItalic, Fmt::Italic),
        (Key::ToolbarCode, Fmt::Code),
        (Key::ToolbarQuote, Fmt::Quote),
        (Key::ToolbarList, Fmt::List),
        (Key::ToolbarLink, Fmt::Link),
    ] {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            row,
            Button::reactive_label(move || i18n::tr(label).to_string())
                .radius(6.0)
                .font_size(13.0)
                .on_click(move |ctx| {
                    apply(&state, body_sig, cursor_sig, &body_idx, ctx, fmt);
                }),
        );
    }
}

/// Apply a formatting action: read the caret, rewrite the body around it, push
/// the new caret back, persist the note, and re-focus the body. No-op on the
/// note save when nothing is selected (the toolbar is hidden then anyway, since
/// it lives inside the selection-gated editor area).
fn apply(
    state: &Rc<RefCell<AppState>>,
    body_sig: Signal<String>,
    cursor_sig: Signal<usize>,
    body_idx: &Rc<Cell<usize>>,
    ctx: &mut EventContext,
    fmt: Fmt,
) {
    let old = body_sig.get_clone();
    let caret = cursor_sig.get();
    let (new_body, new_caret) = fmt.transform(&old, caret);

    body_sig.set(new_body.clone());
    cursor_sig.set(new_caret);
    // Setting `body_sig` only rebases the input's buffer on the next paint; it
    // does not fire the input's `on_change`, so persist the note ourselves —
    // exactly as `editor::insert_image_from_bytes` does.
    crate::editor::write_selected(state, move |note| note.body = new_body);

    // Return focus to the body so the caret we set is live and typing continues
    // where the snippet landed (otherwise the next click into the body would
    // reset the caret to the end). `body_idx` is populated by the time any
    // click happens.
    ctx.focus(body_idx.get());
}

/// The formatting actions the toolbar offers. Split by mechanism: inline
/// markers wrap a placeholder, line prefixes prepend to the caret's line.
#[derive(Clone, Copy)]
enum Fmt {
    Heading,
    Bold,
    Italic,
    Code,
    Quote,
    List,
    Link,
}

impl Fmt {
    /// Produce the edited body and the new caret byte offset.
    fn transform(self, body: &str, caret: usize) -> (String, usize) {
        match self {
            Fmt::Heading => insert_line_prefix(body, caret, "# "),
            Fmt::Quote => insert_line_prefix(body, caret, "> "),
            Fmt::List => insert_line_prefix(body, caret, "- "),
            Fmt::Bold => insert_inline(body, caret, "**"),
            Fmt::Italic => insert_inline(body, caret, "*"),
            Fmt::Code => insert_inline(body, caret, "`"),
            Fmt::Link => insert_link(body, caret),
        }
    }
}

/// Snap `i` down to the nearest char boundary at or before it (and clamp to the
/// string length). The caret offset arrives from the input via a signal, so it
/// is already a valid boundary in practice — this is belt-and-suspenders so a
/// future change can't make us split a multi-byte codepoint and panic.
fn char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Insert an empty `<marker><marker>` pair at `caret`, returning the new body
/// and the caret placed *between* the two markers, so the user types the content
/// directly and it comes out wrapped (no placeholder to delete).
fn insert_inline(body: &str, caret: usize, marker: &str) -> (String, usize) {
    let caret = char_boundary(body, caret);
    let mut out = String::with_capacity(body.len() + marker.len() * 2);
    out.push_str(&body[..caret]);
    out.push_str(marker);
    out.push_str(marker);
    out.push_str(&body[caret..]);
    (out, caret + marker.len())
}

/// Insert `[](https://)` at `caret`, returning the new body and the caret inside
/// the empty `[]` (just past the opening `[`), so the user types the link text
/// directly; `https://` is left as a prefix for the URL.
fn insert_link(body: &str, caret: usize) -> (String, usize) {
    const TEMPLATE: &str = "[](https://)";
    let caret = char_boundary(body, caret);
    let mut out = String::with_capacity(body.len() + TEMPLATE.len());
    out.push_str(&body[..caret]);
    out.push_str(TEMPLATE);
    out.push_str(&body[caret..]);
    (out, caret + 1)
}

/// Insert `prefix` at the start of the line containing `caret` (the byte after
/// the previous `\n`, or 0). Returns the new body and the caret shifted right by
/// the prefix length so it keeps its position within the line's text.
fn insert_line_prefix(body: &str, caret: usize, prefix: &str) -> (String, usize) {
    let caret = char_boundary(body, caret);
    let line_start = body[..caret].rfind('\n').map_or(0, |nl| nl + 1);
    let mut out = String::with_capacity(body.len() + prefix.len());
    out.push_str(&body[..line_start]);
    out.push_str(prefix);
    out.push_str(&body[line_start..]);
    (out, caret + prefix.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_inserts_empty_markers_with_caret_between() {
        // Caret in the middle of "hello" (byte 2, after "he"). Bold inserts an
        // empty "****" and lands the caret between the pairs, so the next char
        // typed lands inside the markers.
        let (out, caret) = insert_inline("hello", 2, "**");
        assert_eq!(out, "he****llo");
        assert_eq!(caret, 4);
        assert_eq!(&out[..caret], "he**");
        assert_eq!(&out[caret..], "**llo");
    }

    #[test]
    fn inline_into_empty_body() {
        let (out, caret) = insert_inline("", 0, "*");
        assert_eq!(out, "**");
        // Caret sits between the two '*' so typing produces "*x*".
        assert_eq!(caret, 1);
    }

    #[test]
    fn link_inserts_template_with_caret_in_brackets() {
        let (out, caret) = insert_link("x", 1);
        assert_eq!(out, "x[](https://)");
        // Caret inside the empty "[]" — typing fills the link text directly.
        assert_eq!(caret, 2);
        assert_eq!(&out[..caret], "x[");
    }

    #[test]
    fn line_prefix_prepends_to_the_caret_line() {
        // "a\nbc", caret at byte 3 (after 'b' on the second line). The prefix
        // goes at the start of that line (byte 2), and the caret shifts right
        // by the prefix length so it stays after 'b'.
        let (out, caret) = insert_line_prefix("a\nbc", 3, "# ");
        assert_eq!(out, "a\n# bc");
        assert_eq!(caret, 5);
        assert_eq!(&out[..caret], "a\n# b");
    }

    #[test]
    fn line_prefix_on_first_line() {
        let (out, caret) = insert_line_prefix("abc", 0, "- ");
        assert_eq!(out, "- abc");
        assert_eq!(caret, 2);
    }

    #[test]
    fn caret_past_a_multibyte_codepoint_snaps_down() {
        // "あ" is 3 bytes; an offset of 2 falls inside it and must snap to 0,
        // so the slice never splits the codepoint.
        let (out, caret) = insert_inline("あ", 2, "**");
        assert_eq!(out, "****あ");
        assert_eq!(caret, 2);
    }
}
