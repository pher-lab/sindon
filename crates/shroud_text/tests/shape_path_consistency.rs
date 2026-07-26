//! Regression: the plain and rich shape paths must agree on line structure.
//!
//! An `Input` measures its content height (and places its caret) through one of
//! three entry points depending on state: `shape_text_attrs` (plain, unfocused),
//! `shape_edit_rich` (rich, focused + highlighter), and `shape_composing`
//! (plain, IME preedit). They must all report the *same* geometry for the same
//! string — otherwise a field's scrollable height and caret position jump as it
//! gains focus or begins composing.
//!
//! This broke for text ending in a line break: upstream cosmic-text's
//! `set_rich_text` splits paragraphs with `BidiParagraphs`, which drops the
//! trailing empty paragraph, and (unlike `set_text`) never re-adds the final
//! empty line. So a highlighted, focused editor measured one line short and
//! could not put the caret on the trailing blank line, while the same note
//! unfocused (plain path) reserved that line. The vendored fork re-adds it in
//! `set_rich_text`; these tests pin the invariant.

use shroud_core::Color;
use shroud_text::{TextAttrs, TextEngine, TextSpan};

const FS: f32 = 16.0;
const LH: f32 = 19.2;
const WRAP: Option<f32> = Some(300.0);

/// One base-attrs span covering the whole text — what `build_highlight_spans`
/// produces for a note with no highlighted ranges (knot's common case).
fn one_span(text: &str, attrs: &TextAttrs) -> Vec<TextSpan> {
    vec![TextSpan::new(text.to_string()).attrs(attrs.clone())]
}

/// A color-only tiling (first half base, second half tinted) — a highlighted
/// note. Color must not change layout, so this must measure like the plain path.
fn split_spans(text: &str, attrs: &TextAttrs) -> Vec<TextSpan> {
    let mid = (0..=text.len() / 2)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap();
    let mut spans = Vec::new();
    if mid > 0 {
        spans.push(TextSpan::new(text[..mid].to_string()).attrs(attrs.clone()));
    }
    spans.push(
        TextSpan::new(text[mid..].to_string())
            .attrs(attrs.clone())
            .color(Color::rgb(1.0, 0.0, 0.0)),
    );
    spans
}

#[test]
fn content_height_agrees_across_shape_paths() {
    let mut e = TextEngine::new();
    let attrs = TextAttrs::default();

    // Trailing / interior newlines are the interesting cases; plain (no break)
    // and CJK are controls.
    let cases = [
        "line one\nline two\n",         // trailing newline — the original bug
        "line one\n",                   // single line + trailing newline
        "a\n\nb\n",                     // blank interior line + trailing newline
        "trailing blank\n\n",           // two trailing blank lines
        "no trailing newline",          // control: no break
        "abc\ndef",                     // control: interior break, no trailing
        "\u{3044}\u{308D}\n\u{306F}\n", // CJK + trailing newline
    ];

    for text in cases {
        let plain = e.shape_text_attrs(text, FS, LH, WRAP, &attrs).height;
        let edit_plain = e
            .shape_edit_plain(text, FS, LH, WRAP, &attrs)
            .shaped()
            .height;
        let rich_1 = e
            .shape_edit_rich(&one_span(text, &attrs), FS, LH, WRAP)
            .shaped()
            .height;
        let rich_n = e
            .shape_edit_rich(&split_spans(text, &attrs), FS, LH, WRAP)
            .shaped()
            .height;

        assert_eq!(plain, edit_plain, "plain vs edit-plain for {text:?}");
        assert_eq!(plain, rich_1, "plain vs single-span rich for {text:?}");
        assert_eq!(plain, rich_n, "plain vs multi-span rich for {text:?}");
    }
}

#[test]
fn rich_caret_reaches_trailing_empty_line() {
    // With the caret at the very end of a note ending in `\n`, it must sit on
    // the empty line *below* the last text — in both the plain path (unfocused)
    // and the rich path (focused + highlighter), not jump back up to the end of
    // the previous line in the rich path.
    let mut e = TextEngine::new();
    let attrs = TextAttrs::default();
    let text = "line one\n";
    let end = text.len();

    let (_, plain_y) = e.caret_at_offset_attrs(text, end, FS, LH, WRAP, &attrs);
    // Shaping parks the buffer in the engine's edit slot, which is what
    // `edit_caret` then reads.
    let _edit = e.shape_edit_rich(&one_span(text, &attrs), FS, LH, WRAP);
    let (_, rich_y) = e.edit_caret(text, end, FS, LH, WRAP, &attrs);

    assert_eq!(
        plain_y, LH,
        "plain caret should sit on the trailing empty line"
    );
    assert_eq!(rich_y, plain_y, "rich caret must match the plain path");
}
