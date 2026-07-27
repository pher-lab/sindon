//! In-editor find / replace bar (B-1 ④).
//!
//! Toggled with Ctrl+H (the global shortcut is registered in [`crate::main`]),
//! this bar searches the body for a case-insensitive substring, steps through
//! the matches, and replaces the current one or all of them.
//!
//! ## How it reaches the body without touching it
//!
//! Exactly like the formatting [`crate::toolbar`], the bar is a *sibling* of the
//! body input and can't reach into it. It drives the body through the three
//! signals the editor shares with it:
//!
//! * `body_sig: Signal<String>` — the body text,
//! * `cursor_sig: Signal<usize>` — the body's caret byte offset, and
//! * `selection_sig: Signal<Option<(usize, usize)>>` — the body's selection.
//!
//! To highlight a match, the bar writes the match range into `selection_sig`
//! (and the caret into `cursor_sig`), then calls [`EventContext::focus`] on the
//! body: the framework only paints a selection / caret — and only runs
//! scroll-to-caret — for the *focused* input, so a match isn't visible until the
//! body has focus. The programmatic caret move scrolls the match into view via
//! the `sync_from_source` reveal the framework gained for B-1 ④. A replacement
//! additionally rewrites `body_sig` and persists the note through
//! [`crate::editor::write_selected`] (setting `body_sig` only rebases the
//! input's buffer; it does not fire the input's `on_change`).
//!
//! ## State
//!
//! The toggle, the query, and the replacement text live in a thread-local
//! [`signals`] singleton (mirroring [`crate::settings::signals`]) so the bar UI
//! and the Ctrl+H handler in `main` share the same handles without threading
//! them through every screen builder. The query / replacement survive a
//! hide-and-reopen, so reopening the bar keeps the last search.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use sindon::reactive::{Reactive, Signal};
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, EventContext, Input, TextWidget};

use crate::editor::write_selected;
use crate::i18n::{self, Key};
use crate::settings;
use crate::state::AppState;

/// The find-replace bar's live state. `Copy` (the fields are `Signal`s, which
/// are cheap `Copy` ids) so it drops into as many closures as needed.
#[derive(Clone, Copy)]
pub struct FindReplaceSignals {
    /// Whether the bar is shown. Toggled by Ctrl+H and the ✕ button; drives the
    /// bar container's `visible`.
    pub visible: Signal<bool>,
    /// The search text (bound to the Find input, so typing updates it live).
    pub query: Signal<String>,
    /// The replacement text (bound to the Replace input).
    pub replace: Signal<String>,
}

thread_local! {
    static SIGNALS: OnceCell<FindReplaceSignals> = const { OnceCell::new() };
}

/// Thread-local find-replace signals, lazily created on first access. Both the
/// bar (built per vault screen) and the Ctrl+H handler (registered once in
/// `main`) call this and get the same handles, so a toggle from either side is
/// seen by the other. Thread-local for the same reason as
/// [`crate::settings::signals`] — the UI runs single-threaded on the event-loop
/// thread.
pub fn signals() -> FindReplaceSignals {
    SIGNALS.with(|c| {
        *c.get_or_init(|| FindReplaceSignals {
            visible: Signal::new(false),
            query: Signal::new(String::new()),
            replace: Signal::new(String::new()),
        })
    })
}

/// Build the find-replace bar into `parent` (the editor column). `body_sig` /
/// `cursor_sig` / `selection_sig` are the body input's bound signals;
/// `body_idx` is the cell the editor fills with the body input's node index, so
/// the handlers can focus it (the bar is built before the body, hence the
/// deferred index — same as the toolbar).
#[allow(clippy::too_many_arguments)]
pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    body_sig: Signal<String>,
    cursor_sig: Signal<usize>,
    selection_sig: Signal<Option<(usize, usize)>>,
    body_idx: Rc<Cell<usize>>,
) {
    let sigs = signals();

    // The whole bar collapses (`display: none`) when toggled off, so it takes no
    // space and its inputs neither paint nor capture focus.
    let bar = tree.add_child(
        parent,
        Container::column()
            .gap(6.0)
            .padding(8.0)
            .radius(8.0)
            .background(settings::surface())
            .visible(Reactive::derive(move || sigs.visible.get())),
    );

    // Row 1: Find field, match counter, prev / next / close.
    let find_row = tree.add_child(bar, Container::row().gap(6.0).align_center());

    let find_input = {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            find_row,
            Input::new()
                .reactive_placeholder(|| i18n::tr(Key::FindReplaceFindPlaceholder).to_string())
                .value(sigs.query)
                .grow(1.0)
                // Enter in the Find field jumps to the next match.
                .on_submit(move |_q, ctx| {
                    navigate(
                        &state,
                        body_sig,
                        cursor_sig,
                        selection_sig,
                        &body_idx,
                        ctx,
                        Dir::Next,
                    );
                }),
        )
    };
    // Record for the Ctrl+H focus (mirrors `search_input_idx` for Ctrl+F).
    state.borrow_mut().find_input_idx = Some(find_input);

    // Live "current / total" (or "no matches") counter.
    tree.add_child(
        find_row,
        TextWidget::reactive(move || {
            counter_label(
                &body_sig.get_clone(),
                &sigs.query.get_clone(),
                selection_sig.get(),
            )
        })
        .color(settings::on_surface_variant()),
    );

    // Prev / Next use fixed glyph labels (no localization needed).
    {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            find_row,
            Button::new("\u{2191}") // ↑
                .radius(6.0)
                .font_size(13.0)
                .on_click(move |ctx| {
                    navigate(
                        &state,
                        body_sig,
                        cursor_sig,
                        selection_sig,
                        &body_idx,
                        ctx,
                        Dir::Prev,
                    );
                }),
        );
    }
    {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            find_row,
            Button::new("\u{2193}") // ↓
                .radius(6.0)
                .font_size(13.0)
                .on_click(move |ctx| {
                    navigate(
                        &state,
                        body_sig,
                        cursor_sig,
                        selection_sig,
                        &body_idx,
                        ctx,
                        Dir::Next,
                    );
                }),
        );
    }
    {
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            find_row,
            Button::new("\u{2715}") // ✕
                .radius(6.0)
                .font_size(13.0)
                .on_click(move |ctx| {
                    sigs.visible.set(false);
                    ctx.focus(body_idx.get());
                }),
        );
    }

    // Row 2: Replace field + Replace / Replace all.
    let replace_row = tree.add_child(bar, Container::row().gap(6.0).align_center());

    {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            replace_row,
            Input::new()
                .reactive_placeholder(|| i18n::tr(Key::FindReplaceReplacePlaceholder).to_string())
                .value(sigs.replace)
                .grow(1.0)
                // Enter in the Replace field replaces the current match.
                .on_submit(move |_v, ctx| {
                    replace_current(&state, body_sig, cursor_sig, selection_sig, &body_idx, ctx);
                }),
        );
    }
    {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            replace_row,
            Button::reactive_label(|| i18n::tr(Key::FindReplaceReplaceBtn).to_string())
                .radius(6.0)
                .font_size(13.0)
                .on_click(move |ctx| {
                    replace_current(&state, body_sig, cursor_sig, selection_sig, &body_idx, ctx);
                }),
        );
    }
    {
        let state = Rc::clone(&state);
        let body_idx = Rc::clone(&body_idx);
        tree.add_child(
            replace_row,
            Button::reactive_label(|| i18n::tr(Key::FindReplaceReplaceAllBtn).to_string())
                .radius(6.0)
                .font_size(13.0)
                .on_click(move |ctx| {
                    replace_all_in_body(
                        &state,
                        body_sig,
                        cursor_sig,
                        selection_sig,
                        &body_idx,
                        ctx,
                    );
                }),
        );
    }
}

/// Step direction for [`navigate`].
#[derive(Clone, Copy)]
enum Dir {
    Next,
    Prev,
}

/// Jump to the next / previous match of the current query, relative to the
/// body's caret (or the active end of its selection). No-op when the query is
/// empty or has no match.
fn navigate(
    state: &Rc<RefCell<AppState>>,
    body_sig: Signal<String>,
    cursor_sig: Signal<usize>,
    selection_sig: Signal<Option<(usize, usize)>>,
    body_idx: &Rc<Cell<usize>>,
    ctx: &mut EventContext,
    dir: Dir,
) {
    let _ = state; // navigation doesn't mutate the note; kept for a uniform signature
    let query = signals().query.get_clone();
    let body = body_sig.get_clone();
    let matches = find_matches(&body, &query);
    if matches.is_empty() {
        return;
    }
    let caret = cursor_sig.get();
    let selection = selection_sig.get();
    let target = match dir {
        // From the end of the current match / caret, so repeated Next walks
        // forward (and wraps).
        Dir::Next => next_match(&matches, selection.map_or(caret, |(_, hi)| hi)),
        // From the start of the current match / caret, so Prev walks back.
        Dir::Prev => prev_match(&matches, selection.map_or(caret, |(lo, _)| lo)),
    };
    if let Some((lo, hi)) = target {
        select_in_body(cursor_sig, selection_sig, body_idx, ctx, lo, hi);
    }
}

/// Replace the currently selected match with the replacement text, then step to
/// the next match. When the selection isn't sitting exactly on a match (e.g. the
/// user just opened the bar), this behaves as Find Next instead — so the first
/// press selects a match and the second replaces it, the usual two-step feel.
fn replace_current(
    state: &Rc<RefCell<AppState>>,
    body_sig: Signal<String>,
    cursor_sig: Signal<usize>,
    selection_sig: Signal<Option<(usize, usize)>>,
    body_idx: &Rc<Cell<usize>>,
    ctx: &mut EventContext,
) {
    let query = signals().query.get_clone();
    let repl = signals().replace.get_clone();
    let body = body_sig.get_clone();
    let matches = find_matches(&body, &query);
    if matches.is_empty() {
        return;
    }

    // Only replace when the selection is exactly on a match; otherwise fall back
    // to Find Next.
    if let Some(sel) = selection_sig.get() {
        if matches.contains(&sel) {
            let (lo, hi) = sel;
            let (new_body, caret) = replace_span(&body, lo, hi, &repl);
            commit_body(state, body_sig, new_body);
            // Step to the next match *after* the caret (which sits past the
            // inserted text, so a replacement that itself contains the query
            // isn't immediately re-matched). If none remain, just place the
            // caret there.
            let after = find_matches(&body_sig.get_clone(), &query);
            match next_match(&after, caret) {
                Some((nlo, nhi)) => {
                    select_in_body(cursor_sig, selection_sig, body_idx, ctx, nlo, nhi)
                }
                None => {
                    selection_sig.set(None);
                    cursor_sig.set(caret);
                    ctx.focus(body_idx.get());
                }
            }
            return;
        }
    }

    // No match selected yet → behave as Find Next.
    navigate(
        state,
        body_sig,
        cursor_sig,
        selection_sig,
        body_idx,
        ctx,
        Dir::Next,
    );
}

/// Replace every match of the current query in one edit. No-op when the query is
/// empty or has no match.
fn replace_all_in_body(
    state: &Rc<RefCell<AppState>>,
    body_sig: Signal<String>,
    cursor_sig: Signal<usize>,
    selection_sig: Signal<Option<(usize, usize)>>,
    body_idx: &Rc<Cell<usize>>,
    ctx: &mut EventContext,
) {
    let query = signals().query.get_clone();
    let repl = signals().replace.get_clone();
    let body = body_sig.get_clone();
    let (new_body, count) = replace_all(&body, &query, &repl);
    if count == 0 {
        return;
    }
    commit_body(state, body_sig, new_body);
    // The old offsets are gone; drop the selection and park the caret at the
    // top, then focus the body so the rewrite is visible.
    selection_sig.set(None);
    cursor_sig.set(0);
    ctx.focus(body_idx.get());
}

/// Point the body's caret + selection at `[lo, hi)` and focus it, so the
/// framework renders the highlight and scrolls the match into view. Does not
/// change the body text.
fn select_in_body(
    cursor_sig: Signal<usize>,
    selection_sig: Signal<Option<(usize, usize)>>,
    body_idx: &Rc<Cell<usize>>,
    ctx: &mut EventContext,
    lo: usize,
    hi: usize,
) {
    selection_sig.set(Some((lo, hi)));
    cursor_sig.set(hi);
    ctx.focus(body_idx.get());
}

/// Write `new_body` into the shared body signal and persist it to the selected
/// note. Mirrors the toolbar: `body_sig.set` only rebases the input's buffer on
/// the next paint and does *not* fire its `on_change`, so the note is persisted
/// here explicitly.
fn commit_body(state: &Rc<RefCell<AppState>>, body_sig: Signal<String>, new_body: String) {
    body_sig.set(new_body.clone());
    write_selected(state, move |note| note.body = new_body);
}

/// The counter shown between the Find field and the nav buttons: empty while the
/// query is empty, a localized "no matches" when there are none, `"i/total"`
/// when the current selection sits on the i-th match, else `"total"`.
fn counter_label(body: &str, query: &str, selection: Option<(usize, usize)>) -> String {
    if query.is_empty() {
        return String::new();
    }
    let matches = find_matches(body, query);
    if matches.is_empty() {
        return i18n::tr(Key::FindReplaceNoMatches).to_string();
    }
    match current_index(&matches, selection) {
        Some(i) => format!("{i}/{}", matches.len()),
        None => matches.len().to_string(),
    }
}

// ── Pure search / replace core (unit-tested below) ──────────────────────────

/// Case-insensitive, non-overlapping matches of `needle` in `haystack`, as
/// `[lo, hi)` byte ranges into `haystack` (always on `char` boundaries). An
/// empty needle yields no matches. The offsets are real byte positions in the
/// *original* (un-lowercased) text, so a replacement lands exactly on the
/// matched span — see [`match_at`] for the case-folding details.
pub fn find_matches(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < haystack.len() {
        if let Some(len) = match_at(&haystack[start..], needle) {
            out.push((start, start + len));
            // Non-overlapping: skip past this match (`len > 0` for a non-empty
            // needle, so this always advances).
            start += len;
        } else {
            // Advance to the next `char` boundary and try again.
            start += haystack[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// If `hay` starts with `needle` compared case-insensitively, return the byte
/// length the match spans *in `hay`* (which can differ from `needle.len()`,
/// e.g. `İ` vs `i`), else `None`. Both sides are lowercased per `char`
/// (`char::to_lowercase`), so this handles ASCII and the common 1:1 Unicode
/// case mappings; the match must end on a `hay` `char` boundary, so a needle
/// whose lowercase only partially covers a hay char's lowercase expansion does
/// not match.
fn match_at(hay: &str, needle: &str) -> Option<usize> {
    let mut needle_lc = needle.chars().flat_map(char::to_lowercase).peekable();
    let mut hay_chars = hay.chars();
    let mut consumed = 0usize;
    while needle_lc.peek().is_some() {
        let hc = hay_chars.next()?; // ran out of hay before needle → no match
        for lc in hc.to_lowercase() {
            match needle_lc.next() {
                Some(nc) if nc == lc => {}
                // Mismatch, or the needle ended part-way through this hay char's
                // lowercase expansion (a partial match isn't a match).
                _ => return None,
            }
        }
        consumed += hc.len_utf8();
    }
    Some(consumed)
}

/// First match starting at or after `from`, wrapping to the first match when
/// none is at/after `from`. `None` only when there are no matches.
fn next_match(matches: &[(usize, usize)], from: usize) -> Option<(usize, usize)> {
    matches
        .iter()
        .find(|&&(lo, _)| lo >= from)
        .or_else(|| matches.first())
        .copied()
}

/// Last match ending at or before `from`, wrapping to the last match when none
/// is at/before `from`. `None` only when there are no matches.
fn prev_match(matches: &[(usize, usize)], from: usize) -> Option<(usize, usize)> {
    matches
        .iter()
        .rev()
        .find(|&&(_, hi)| hi <= from)
        .or_else(|| matches.last())
        .copied()
}

/// 1-based index of the match exactly equal to `selection` within `matches`, or
/// `None` when the selection isn't on a match.
fn current_index(matches: &[(usize, usize)], selection: Option<(usize, usize)>) -> Option<usize> {
    let sel = selection?;
    matches.iter().position(|&m| m == sel).map(|i| i + 1)
}

/// Replace the `[lo, hi)` span of `body` with `repl`, returning the new body and
/// the caret placed just after the inserted replacement. `lo` / `hi` are snapped
/// to `char` boundaries (belt-and-suspenders against a stale offset).
fn replace_span(body: &str, lo: usize, hi: usize, repl: &str) -> (String, usize) {
    let lo = char_boundary(body, lo);
    let hi = char_boundary(body, hi).max(lo);
    let mut out = String::with_capacity(body.len() - (hi - lo) + repl.len());
    out.push_str(&body[..lo]);
    out.push_str(repl);
    let caret = out.len();
    out.push_str(&body[hi..]);
    (out, caret)
}

/// Replace every case-insensitive match of `query` in `body` with `repl`,
/// returning the new body and the number of replacements. Returns the body
/// unchanged with count 0 when the query is empty or has no match. Matches are
/// the non-overlapping ones from [`find_matches`], so replacement text is never
/// itself re-scanned.
pub fn replace_all(body: &str, query: &str, repl: &str) -> (String, usize) {
    let matches = find_matches(body, query);
    if matches.is_empty() {
        return (body.to_string(), 0);
    }
    let mut out = String::with_capacity(body.len());
    let mut prev = 0usize;
    for &(lo, hi) in &matches {
        out.push_str(&body[prev..lo]);
        out.push_str(repl);
        prev = hi;
    }
    out.push_str(&body[prev..]);
    (out, matches.len())
}

/// Snap `i` down to the nearest `char` boundary at or before it (clamped to the
/// string length).
fn char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matches_is_case_insensitive_and_non_overlapping() {
        // Three case-insensitive matches of "ab" in "ABabAb"; the byte ranges
        // index the *original* text.
        let m = find_matches("ABabAb", "ab");
        assert_eq!(m, vec![(0, 2), (2, 4), (4, 6)]);
        // Non-overlapping: "aa" in "aaaa" → [0,2), [2,4), not [1,3).
        assert_eq!(find_matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn find_matches_empty_needle_and_no_match() {
        assert!(find_matches("anything", "").is_empty());
        assert!(find_matches("", "x").is_empty());
        assert!(find_matches("hello", "zzz").is_empty());
    }

    #[test]
    fn find_matches_preserves_byte_offsets_across_multibyte_text() {
        // "café" — the 'é' is two bytes, so "fé" starts at byte 3 (after "caf").
        let body = "café au lait";
        let m = find_matches(body, "FÉ");
        assert_eq!(m.len(), 1);
        let (lo, hi) = m[0];
        assert_eq!(&body[lo..hi], "fé");
    }

    #[test]
    fn find_matches_does_not_split_a_codepoint() {
        // Searching for "a" must never report an offset inside the 3-byte 'あ'.
        let body = "あaあ";
        let m = find_matches(body, "a");
        assert_eq!(m, vec![(3, 4)]);
        assert_eq!(&body[3..4], "a");
    }

    #[test]
    fn next_and_prev_match_wrap() {
        let m = vec![(0, 2), (5, 7), (10, 12)];
        // Next from before the first → first; from the first's end → second.
        assert_eq!(next_match(&m, 0), Some((0, 2)));
        assert_eq!(next_match(&m, 2), Some((5, 7)));
        // Past the last → wraps to the first.
        assert_eq!(next_match(&m, 12), Some((0, 2)));
        // Prev from the second's start → first; from the first's start → wraps
        // to the last.
        assert_eq!(prev_match(&m, 5), Some((0, 2)));
        assert_eq!(prev_match(&m, 0), Some((10, 12)));
        // No matches → None either way.
        assert_eq!(next_match(&[], 0), None);
        assert_eq!(prev_match(&[], 0), None);
    }

    #[test]
    fn current_index_locates_the_selected_match() {
        let m = vec![(0, 2), (5, 7), (10, 12)];
        assert_eq!(current_index(&m, Some((5, 7))), Some(2));
        assert_eq!(current_index(&m, Some((0, 2))), Some(1));
        // A selection that isn't a match (or no selection) → None.
        assert_eq!(current_index(&m, Some((1, 3))), None);
        assert_eq!(current_index(&m, None), None);
    }

    #[test]
    fn replace_span_swaps_the_range_and_returns_the_caret() {
        // Replace "lo" in "hello" with "XYZ"; caret lands just past "XYZ".
        let (out, caret) = replace_span("hello", 3, 5, "XYZ");
        assert_eq!(out, "helXYZ");
        assert_eq!(caret, "helXYZ".len());
        assert_eq!(&out[..caret], "helXYZ");
    }

    #[test]
    fn replace_all_replaces_every_match_case_insensitively() {
        let (out, n) = replace_all("Foo foo FOO", "foo", "bar");
        assert_eq!(out, "bar bar bar");
        assert_eq!(n, 3);
        // No match → unchanged, count 0.
        let (out, n) = replace_all("nothing here", "xyz", "!");
        assert_eq!(out, "nothing here");
        assert_eq!(n, 0);
    }

    #[test]
    fn replace_all_with_longer_replacement_does_not_rescan() {
        // Replacing "a" with "aa" must yield exactly one "aa" per original "a"
        // (3 → "aaaaaa"), not loop on the inserted text.
        let (out, n) = replace_all("aaa", "a", "aa");
        assert_eq!(out, "aaaaaa");
        assert_eq!(n, 3);
    }

    #[test]
    fn match_at_rejects_partial_and_overruns() {
        // Exact prefix match returns the byte length spanned in the haystack.
        assert_eq!(match_at("hello", "he"), Some(2));
        // A needle longer than the haystack can't match.
        assert_eq!(match_at("he", "hello"), None);
        // Different content doesn't match.
        assert_eq!(match_at("hello", "xy"), None);
    }
}
