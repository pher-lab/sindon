//! Tracks the IME's hidden character caret during conversion.
//!
//! While a Japanese IME converts (space pressed, clauses underlined), the
//! platform reports only the *target clause* as a byte range — but the IME
//! also keeps an internal character-level caret the user steers with ←/→,
//! and TSF-aware apps (Notepad) draw it. That caret never crosses the IMM
//! bridge winit consumes, so it cannot be read directly; it can, however, be
//! reconstructed from what *does* arrive, with no guessing:
//!
//! - A preedit update with the **same clause range** right after a plain
//!   arrow is the hidden caret moving one character inside the clause (the
//!   IME notifies every such step; verified for MS-IME and Copilot Keyboard,
//!   whose projections behave identically here).
//! - A preedit update with a **different clause range** re-anchors the caret
//!   exactly: entering a clause from the right puts it one character before
//!   the clause end, from the left one character after the start.
//! - Caret mode *engages* on the wrap the IME performs when ← is pressed on
//!   the leftmost clause: the range jumps first → last clause and the caret
//!   lands one character before the composition end.
//! - **Silence** (arrow consumed, no update) means nothing moved — edge
//!   no-ops. MS-IME's "caret escapes the composition" behaviour never
//!   reaches the IMM bridge at all.
//!
//! Anything unexplained — text changed, range collapsed or gone, a range
//! jump with no arrow hint — drops tracking back to `None` (clause-select
//! display, the pre-tracking behaviour), so a mismatched IME degrades to the
//! status quo instead of drawing a caret that lies.

use crate::event::ImeNav;

/// Advance the tracked caret (a byte offset into the preedit string, or
/// `None` while in clause-select mode) across one `ImePreedit` update.
///
/// `prev_text` / `prev_range` are the widget's stored preedit state from
/// *before* this update; `text` / `range` / `nav` come from the event.
pub(crate) fn track(
    state: Option<usize>,
    prev_text: &str,
    prev_range: Option<(usize, usize)>,
    text: &str,
    range: Option<(usize, usize)>,
    nav: Option<ImeNav>,
) -> Option<usize> {
    // No clause on screen (typing, cancelled, partially reverted): the
    // collapsed-cursor path already draws a real caret — nothing to track.
    let (cs, ce) = match range {
        Some((cs, ce)) if cs < ce => (cs, ce),
        _ => return None,
    };
    // Text changed: candidate swap, clause resize, more typing, partial Esc
    // revert. None of these are caret walks; re-enter clause-select mode.
    if text != prev_text {
        return None;
    }
    let prev = prev_range?;

    if (cs, ce) == prev {
        // Same clause re-reported: one caret step inside it, in the arrow's
        // direction, clamped to the clause's closed interval [cs, ce] (both
        // boundaries are legal caret stops — measured hysteresis model).
        let c = state?.clamp(cs, ce);
        match nav {
            Some(ImeNav::Left) => Some(if c <= cs { cs } else { prev_char(text, c) }),
            Some(ImeNav::Right) => Some(if c >= ce { ce } else { next_char(text, c) }),
            None => Some(c),
        }
    } else {
        match (state, nav) {
            // Wrap: ← on the leftmost clause jumps the range to the last
            // clause and engages caret mode one character before the end
            // ([天I気], not [天気I] — the wrap and the step are one press).
            (None, Some(ImeNav::Left)) if prev.0 == 0 && ce == text.len() => {
                Some(prev_char(text, ce))
            }
            // Ordinary clause-select move: underline follows, no caret.
            (None, _) => None,
            // Caret mode crossing into a neighbour clause: re-anchor one
            // character in from the entry edge (this is the self-correction
            // point — whatever drift a missed step caused ends here).
            (Some(_), Some(ImeNav::Left)) => Some(prev_char(text, ce).max(cs)),
            (Some(_), Some(ImeNav::Right)) => Some(next_char(text, cs).min(ce)),
            // Range moved without an arrow — not a walk we understand.
            (Some(_), None) => None,
        }
    }
}

/// Largest char boundary strictly before `i` (0 when there is none).
fn prev_char(text: &str, i: usize) -> usize {
    text[..i]
        .chars()
        .next_back()
        .map_or(0, |c| i - c.len_utf8())
}

/// Smallest char boundary strictly after `i` (`text.len()` when at the end).
fn next_char(text: &str, i: usize) -> usize {
    text[i..]
        .chars()
        .next()
        .map_or(text.len(), |c| i + c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 今日はいい天気 = 今日は(0..9) いい(9..15) 天気(15..21), all offsets in
    // bytes (3 per char). This is the exact trace measured from MS-IME and
    // Copilot Keyboard on 2026-07-22 (ime_probe2): both project the same
    // machine over IMM.
    const T: &str = "今日はいい天気";

    #[test]
    fn typing_and_space_stay_in_clause_select() {
        // Collapsed cursor while typing kana → no tracking.
        assert_eq!(
            track(None, "きょう", Some((9, 9)), "きょう", Some((9, 9)), None),
            None
        );
        // Space: text changes kana→converted, target clause appears.
        assert_eq!(
            track(
                None,
                "きょうはいいてんき",
                Some((0, 9)),
                T,
                Some((0, 9)),
                None
            ),
            None
        );
        // Plain clause-select → / ← moves keep the caret hidden.
        assert_eq!(
            track(None, T, Some((0, 9)), T, Some((9, 15)), Some(ImeNav::Right)),
            None
        );
        assert_eq!(
            track(
                None,
                T,
                Some((15, 21)),
                T,
                Some((9, 15)),
                Some(ImeNav::Left)
            ),
            None
        );
    }

    #[test]
    fn left_edge_wrap_engages_caret_mode_before_last_char() {
        // ← on 今日は wraps to 天気 with the caret at [天I気] = byte 18.
        assert_eq!(
            track(None, T, Some((0, 9)), T, Some((15, 21)), Some(ImeNav::Left)),
            Some(18)
        );
    }

    #[test]
    fn same_range_fire_steps_one_char_with_boundary_clamp() {
        // [天I気] → ← → [I天気] (clause-start boundary is a legal stop) …
        assert_eq!(
            track(
                Some(18),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Left)
            ),
            Some(15)
        );
        // … and a further same-range ← at the boundary clamps in place.
        assert_eq!(
            track(
                Some(15),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Left)
            ),
            Some(15)
        );
        // Rightward mirror: [天I気] → → → [天気I], clamped thereafter.
        assert_eq!(
            track(
                Some(18),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Right)
            ),
            Some(21)
        );
        assert_eq!(
            track(
                Some(21),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Right)
            ),
            Some(21)
        );
    }

    #[test]
    fn clause_crossing_reanchors_one_char_in_from_entry_edge() {
        // Crossing left out of 天気 into いい lands [いIい] = byte 12.
        assert_eq!(
            track(
                Some(15),
                T,
                Some((15, 21)),
                T,
                Some((9, 15)),
                Some(ImeNav::Left)
            ),
            Some(12)
        );
        // Crossing right out of 今日は into いい lands [いIい] too (start+1).
        assert_eq!(
            track(
                Some(9),
                T,
                Some((0, 9)),
                T,
                Some((9, 15)),
                Some(ImeNav::Right)
            ),
            Some(12)
        );
    }

    #[test]
    fn full_measured_leftward_walk() {
        // The ①/② probe walk: wrap into 天気, one step, cross into いい, one
        // step, cross into 今日は, two steps to its head. Byte positions:
        // 18 → 15 → 12 → 9 → 6 → 3 → 0.
        let mut c = track(None, T, Some((0, 9)), T, Some((15, 21)), Some(ImeNav::Left));
        assert_eq!(c, Some(18));
        let steps: [(Option<(usize, usize)>, usize); 6] = [
            (Some((15, 21)), 15),
            (Some((9, 15)), 12),
            (Some((9, 15)), 9),
            (Some((0, 9)), 6),
            (Some((0, 9)), 3),
            (Some((0, 9)), 0),
        ];
        let mut prev_range = Some((15, 21));
        for (range, want) in steps {
            c = track(c, T, prev_range, T, range, Some(ImeNav::Left));
            assert_eq!(c, Some(want));
            prev_range = range;
        }
        // Edge silence sends no event at all — state simply persists.
    }

    #[test]
    fn unexplained_updates_drop_tracking() {
        // Esc partial revert: text changes (きょうは back to kana) → None.
        assert_eq!(
            track(
                Some(0),
                T,
                Some((0, 9)),
                "きょうはいい天気",
                Some((0, 0)),
                None
            ),
            None
        );
        // Range collapses or disappears → None.
        assert_eq!(
            track(Some(18), T, Some((15, 21)), T, Some((21, 21)), None),
            None
        );
        assert_eq!(track(Some(18), T, Some((15, 21)), T, None, None), None);
        // Range jumps with no arrow hint (candidate ops, modifier'd keys) →
        // bail out rather than guess.
        assert_eq!(
            track(Some(18), T, Some((15, 21)), T, Some((9, 15)), None),
            None
        );
    }
}
