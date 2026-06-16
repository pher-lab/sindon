//! Markdown smart-keymap policy for the body editor (B-1 ③).
//!
//! The framework `Input` owns the *mechanism* — it applies the [`KeyEdit`] a
//! hook returns as one discrete undo step (see `Input::on_enter` /
//! `Input::on_backspace`). This module owns the *policy*: what a markdown list /
//! quote marker looks like, and how Enter / Backspace should treat it. Keeping
//! the markdown knowledge here (not in the generic text widget) mirrors how the
//! live syntax highlighter splits work — framework hook, app classifier.
//!
//! Behavior:
//!   * **Enter** on a list / quote item continues it on the next line, carrying
//!     the indentation, incrementing an ordered number, and resetting a task
//!     checkbox to unchecked. On an *empty* item (just the marker) Enter instead
//!     clears the marker, so a second Enter ends the list.
//!   * **Backspace** with the caret right after a complete marker (nothing else
//!     typed on the line yet) deletes the whole marker in one stroke, keeping
//!     any indentation.

use shroud::widgets::KeyEdit;

/// Byte range `[start, end)` of the hard (`\n`-separated) line containing
/// `cursor`. `start` is just after the preceding newline (or 0); `end` is the
/// next newline at/after the caret (or the end of the text).
fn line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(text.len());
    (start, end)
}

/// A list / quote marker parsed off the start of a line.
struct Marker {
    /// Leading whitespace (spaces / tabs) before the marker.
    indent: String,
    /// The marker exactly as it appears, including its trailing space — e.g.
    /// `"- "`, `"1. "`, `"- [x] "`, `"> "`. Used to locate where the item's
    /// content begins.
    prefix: String,
    /// The marker to begin the *next* line with: same as `prefix`, except an
    /// ordered list increments its number and a task checkbox resets to `[ ]`.
    next: String,
}

/// Recognize a markdown list / blockquote marker at the start of `line`.
/// Returns `None` for an ordinary paragraph line. Markers recognized:
///
///   * unordered: `- `, `* `, `+ ` (optionally a GFM task box `[ ] ` / `[x] `),
///   * ordered: `N. ` or `N) `,
///   * blockquote: `> ` (or a bare `>`).
fn parse_marker(line: &str) -> Option<Marker> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = line[..indent_len].to_string();
    let after = &line[indent_len..];

    // Blockquote: '>' with an optional single trailing space. Continuation
    // always uses "> " so a bare ">" line still continues with a tidy marker.
    if after.starts_with('>') {
        let prefix = if after.starts_with("> ") { "> " } else { ">" };
        return Some(Marker {
            indent,
            prefix: prefix.to_string(),
            next: "> ".to_string(),
        });
    }

    // Unordered list: a bullet char then a space, optionally a task checkbox.
    for bullet in ['-', '*', '+'] {
        let mut chars = after.chars();
        if chars.next() == Some(bullet) && chars.next() == Some(' ') {
            let body = &after[2..]; // past the "<bullet> "
            for box_mark in ["[ ] ", "[x] ", "[X] "] {
                if body.starts_with(box_mark) {
                    return Some(Marker {
                        indent,
                        prefix: format!("{bullet} {box_mark}"),
                        // A continued task item starts unchecked.
                        next: format!("{bullet} [ ] "),
                    });
                }
            }
            let prefix = format!("{bullet} ");
            return Some(Marker {
                indent,
                next: prefix.clone(),
                prefix,
            });
        }
    }

    // Ordered list: one or more digits, then '.' or ')', then a space.
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after_digits = &after[digits.len()..];
        let delim = after_digits.chars().next();
        // `after_digits[1..]` is safe: the delim, when matched, is a 1-byte
        // ASCII char so the slice lands on a boundary.
        if matches!(delim, Some('.') | Some(')')) && after_digits[1..].starts_with(' ') {
            let delim = delim.unwrap();
            // Saturating so a pathologically long number can't panic; the
            // continuation just stops incrementing at the ceiling.
            let n: u64 = digits.parse().unwrap_or(0);
            return Some(Marker {
                indent,
                prefix: format!("{digits}{delim} "),
                next: format!("{}{delim} ", n.saturating_add(1)),
            });
        }
    }

    None
}

/// Enter handler for the body editor: continue or exit a markdown list / quote.
/// Returns `None` for a non-list line so the editor inserts a plain newline.
pub fn smart_enter(text: &str, cursor: usize) -> Option<KeyEdit> {
    let (line_start, line_end) = line_bounds(text, cursor);
    let line = &text[line_start..line_end];
    let m = parse_marker(line)?;
    let content = &line[m.indent.len() + m.prefix.len()..];
    if content.trim().is_empty() {
        // Empty item: Enter exits the list. Clear the whole marker line, leaving
        // a blank line with the caret at its start (a second Enter then just
        // adds a newline). Matches the list-exit behavior of common editors.
        Some(KeyEdit {
            replace: line_start..line_end,
            insert: String::new(),
            caret: line_start,
        })
    } else {
        // Continue: open a new line carrying the indentation and the (possibly
        // incremented / reset) marker. Inserting at the caret splits the line,
        // so any text after the caret moves onto the new bulleted line.
        let insert = format!("\n{}{}", m.indent, m.next);
        let caret = cursor + insert.len();
        Some(KeyEdit {
            replace: cursor..cursor,
            insert,
            caret,
        })
    }
}

/// Backspace handler for the body editor: delete a whole list / quote marker
/// when the caret sits right at its end. Returns `None` otherwise so the editor
/// falls back to a normal single-character delete.
pub fn smart_backspace(text: &str, cursor: usize) -> Option<KeyEdit> {
    let (line_start, _) = line_bounds(text, cursor);
    let before = &text[line_start..cursor];
    let m = parse_marker(before)?;
    // Only fire when everything before the caret on this line is exactly the
    // marker — i.e. the caret is at the marker's trailing edge with no content
    // typed yet. Delete just the marker, keeping any indentation, so "  - |"
    // becomes "  |".
    if before == format!("{}{}", m.indent, m.prefix) {
        let marker_start = line_start + m.indent.len();
        Some(KeyEdit {
            replace: marker_start..cursor,
            insert: String::new(),
            caret: marker_start,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_continues_unordered_and_ordered_lists() {
        assert_eq!(
            smart_enter("- a", 3),
            Some(KeyEdit {
                replace: 3..3,
                insert: "\n- ".to_string(),
                caret: 6,
            })
        );
        // Ordered lists increment.
        assert_eq!(
            smart_enter("1. a", 4),
            Some(KeyEdit {
                replace: 4..4,
                insert: "\n2. ".to_string(),
                caret: 8,
            })
        );
        // The `)` delimiter is honored too.
        assert_eq!(
            smart_enter("9) x", 4),
            Some(KeyEdit {
                replace: 4..4,
                insert: "\n10) ".to_string(),
                caret: 9,
            })
        );
    }

    #[test]
    fn enter_carries_indentation() {
        assert_eq!(
            smart_enter("  - a", 5),
            Some(KeyEdit {
                replace: 5..5,
                insert: "\n  - ".to_string(),
                caret: 10,
            })
        );
    }

    #[test]
    fn enter_continues_blockquote_and_resets_task_box() {
        assert_eq!(
            smart_enter("> quote", 7),
            Some(KeyEdit {
                replace: 7..7,
                insert: "\n> ".to_string(),
                caret: 10,
            })
        );
        // A checked task continues as an unchecked one.
        assert_eq!(
            smart_enter("- [x] done", 10),
            Some(KeyEdit {
                replace: 10..10,
                insert: "\n- [ ] ".to_string(),
                caret: 17,
            })
        );
    }

    #[test]
    fn enter_on_empty_item_clears_the_marker() {
        // An empty bullet: Enter removes the marker rather than continuing it.
        assert_eq!(
            smart_enter("- ", 2),
            Some(KeyEdit {
                replace: 0..2,
                insert: String::new(),
                caret: 0,
            })
        );
        // Same for an empty ordered item, on the relevant line of a multi-line
        // buffer (offsets must be absolute).
        assert_eq!(
            smart_enter("x\n1. ", 5),
            Some(KeyEdit {
                replace: 2..5,
                insert: String::new(),
                caret: 2,
            })
        );
    }

    #[test]
    fn enter_returns_none_for_plain_text() {
        assert_eq!(smart_enter("plain text", 10), None);
        // A lone "-" without the trailing space is not a list.
        assert_eq!(smart_enter("-no space", 9), None);
        // Empty buffer.
        assert_eq!(smart_enter("", 0), None);
    }

    #[test]
    fn enter_uses_the_line_the_caret_is_on() {
        // Multi-line buffer: continuation offsets are absolute into the buffer,
        // and the marker is read from the caret's line.
        let text = "intro\n- item";
        assert_eq!(
            smart_enter(text, text.len()),
            Some(KeyEdit {
                replace: 12..12,
                insert: "\n- ".to_string(),
                caret: 15,
            })
        );
    }

    #[test]
    fn backspace_deletes_a_whole_marker_keeping_indentation() {
        assert_eq!(
            smart_backspace("- ", 2),
            Some(KeyEdit {
                replace: 0..2,
                insert: String::new(),
                caret: 0,
            })
        );
        // Indentation is preserved: "  - |" -> "  |".
        assert_eq!(
            smart_backspace("  - ", 4),
            Some(KeyEdit {
                replace: 2..4,
                insert: String::new(),
                caret: 2,
            })
        );
        // Task marker deletes as a unit.
        assert_eq!(
            smart_backspace("- [ ] ", 6),
            Some(KeyEdit {
                replace: 0..6,
                insert: String::new(),
                caret: 0,
            })
        );
    }

    #[test]
    fn backspace_returns_none_once_content_is_typed() {
        // The caret is past content, not at the marker's edge — normal delete.
        assert_eq!(smart_backspace("- a", 3), None);
        // Caret inside the marker (between "-" and " ") — not a complete marker.
        assert_eq!(smart_backspace("- ", 1), None);
        // Plain text.
        assert_eq!(smart_backspace("hello", 5), None);
    }
}
