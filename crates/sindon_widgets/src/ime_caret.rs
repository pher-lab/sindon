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
//! - Tracking *arms* on the wrap the IME performs when ← is pressed on the
//!   leftmost clause: the range jumps first → last clause and the caret
//!   lands one character before the composition end.
//! - **Silence** (arrow consumed, no update) means nothing moved — edge
//!   no-ops. MS-IME's "caret escapes the composition" behaviour never
//!   reaches the IMM bridge at all.
//!
//! The wrap signature alone is ambiguous, though: Google 日本語入力 *cycles*
//! clause selection at both edges — its ← on the first clause also jumps the
//! range to the last clause, with no hidden caret behind it (measured
//! 2026-07-22, ime_probe3). What separates the families is that a cycling
//! IME never re-reports an unchanged range: every press moves the clause.
//! So the wrap only arms a **tentative, undrawn** track, and the caret
//! becomes visible on the first same-range step — the event that proves a
//! character caret exists. Under a cycling IME that proof never arrives and
//! nothing is ever drawn.
//!
//! Anything unexplained — text changed, range collapsed or gone, a range
//! jump with no arrow hint — drops tracking back to [`Track::Idle`]
//! (clause-select display, the pre-tracking behaviour), so a mismatched IME
//! degrades to the status quo instead of drawing a caret that lies.

use crate::event::ImeNav;

/// Hidden-caret tracking state across one IME composition.
///
/// Positions are byte offsets into the preedit string. Only [`Live`]
/// (Self::Live) is ever drawn; `Tentative` follows the same arithmetic
/// silently until a same-range step proves the caret is real.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Track {
    /// Clause-select display: thick underline only, no caret claimed.
    #[default]
    Idle,
    /// Armed by the left-edge wrap, not yet corroborated — not drawn.
    Tentative(usize),
    /// Corroborated by at least one same-range step — drawn.
    Live(usize),
}

impl Track {
    /// The caret position to draw, if tracking has been corroborated.
    pub(crate) fn caret(self) -> Option<usize> {
        match self {
            Track::Live(c) => Some(c),
            _ => None,
        }
    }

    fn pos(self) -> Option<usize> {
        match self {
            Track::Idle => None,
            Track::Tentative(c) | Track::Live(c) => Some(c),
        }
    }

    /// Keep the variant (tentative stays tentative) but move the position.
    fn moved(self, c: usize) -> Track {
        match self {
            Track::Idle => Track::Idle,
            Track::Tentative(_) => Track::Tentative(c),
            Track::Live(_) => Track::Live(c),
        }
    }
}

/// Advance the tracker across one `ImePreedit` update.
///
/// `prev_text` / `prev_range` are the widget's stored preedit state from
/// *before* this update; `text` / `range` / `nav` come from the event.
pub(crate) fn track(
    state: Track,
    prev_text: &str,
    prev_range: Option<(usize, usize)>,
    text: &str,
    range: Option<(usize, usize)>,
    nav: Option<ImeNav>,
) -> Track {
    // No clause on screen (typing, cancelled, partially reverted): the
    // collapsed-cursor path already draws a real caret — nothing to track.
    let (cs, ce) = match range {
        Some((cs, ce)) if cs < ce => (cs, ce),
        _ => return Track::Idle,
    };
    // Text changed: candidate swap, clause resize, more typing, partial Esc
    // revert. None of these are caret walks; re-enter clause-select mode.
    if text != prev_text {
        return Track::Idle;
    }
    let prev = match prev_range {
        Some(p) => p,
        None => return Track::Idle,
    };

    if (cs, ce) == prev {
        // Same clause re-reported: one caret step inside it, in the arrow's
        // direction, clamped to the clause's closed interval [cs, ce] (both
        // boundaries are legal caret stops — measured hysteresis model).
        // This event is also the proof a character caret exists at all —
        // cycling IMEs never produce it — so it promotes Tentative to Live.
        let c = match state.pos() {
            Some(c) => c.clamp(cs, ce),
            None => return Track::Idle,
        };
        match nav {
            Some(ImeNav::Left) => Track::Live(if c <= cs { cs } else { prev_char(text, c) }),
            Some(ImeNav::Right) => Track::Live(if c >= ce { ce } else { next_char(text, c) }),
            None => state.moved(c),
        }
    } else {
        match (state, nav) {
            // Wrap: ← on the leftmost clause jumps the range to the last
            // clause with the hidden caret one character before the end
            // ([天I気], not [天気I] — the wrap and the step are one press).
            // Arm only: Google-style clause cycling produces this exact
            // signature with no caret behind it, so nothing is drawn until
            // a same-range step corroborates.
            (Track::Idle, Some(ImeNav::Left)) if prev.0 == 0 && ce == text.len() => {
                Track::Tentative(prev_char(text, ce))
            }
            // Ordinary clause-select move: underline follows, no caret.
            (Track::Idle, _) => Track::Idle,
            // Crossing into a neighbour clause: re-anchor one character in
            // from the entry edge (this is the self-correction point —
            // whatever drift a missed step caused ends here).
            (s, Some(ImeNav::Left)) => s.moved(prev_char(text, ce).max(cs)),
            (s, Some(ImeNav::Right)) => s.moved(next_char(text, cs).min(ce)),
            // Range moved without an arrow — not a walk we understand.
            (_, None) => Track::Idle,
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
    // Copilot Keyboard on 2026-07-22 (ime_probe2/3): both project the same
    // machine over IMM.
    const T: &str = "今日はいい天気";

    #[test]
    fn typing_and_space_stay_idle() {
        // Collapsed cursor while typing kana → no tracking.
        assert_eq!(
            track(
                Track::Idle,
                "きょう",
                Some((9, 9)),
                "きょう",
                Some((9, 9)),
                None
            ),
            Track::Idle
        );
        // Space: text changes kana→converted, target clause appears.
        assert_eq!(
            track(
                Track::Idle,
                "きょうはいいてんき",
                Some((0, 9)),
                T,
                Some((0, 9)),
                None
            ),
            Track::Idle
        );
        // Plain clause-select → / ← moves keep the caret hidden.
        assert_eq!(
            track(
                Track::Idle,
                T,
                Some((0, 9)),
                T,
                Some((9, 15)),
                Some(ImeNav::Right)
            ),
            Track::Idle
        );
        assert_eq!(
            track(
                Track::Idle,
                T,
                Some((15, 21)),
                T,
                Some((9, 15)),
                Some(ImeNav::Left)
            ),
            Track::Idle
        );
    }

    #[test]
    fn wrap_arms_tentative_and_first_step_goes_live() {
        // ← on 今日は wraps to 天気: armed at [天I気] = byte 18, NOT drawn —
        // this exact signature is also Google-style cycling (see below).
        let s = track(
            Track::Idle,
            T,
            Some((0, 9)),
            T,
            Some((15, 21)),
            Some(ImeNav::Left),
        );
        assert_eq!(s, Track::Tentative(18));
        assert_eq!(s.caret(), None);
        // The next same-range step is the corroboration: [I天気], drawn.
        let s = track(s, T, Some((15, 21)), T, Some((15, 21)), Some(ImeNav::Left));
        assert_eq!(s, Track::Live(15));
        assert_eq!(s.caret(), Some(15));
    }

    #[test]
    fn same_range_fire_steps_one_char_with_boundary_clamp() {
        // Live at the clause-start boundary clamps in place …
        assert_eq!(
            track(
                Track::Live(15),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Left)
            ),
            Track::Live(15)
        );
        // … and the rightward mirror walks [天I気] → [天気I], then clamps.
        assert_eq!(
            track(
                Track::Live(18),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Right)
            ),
            Track::Live(21)
        );
        assert_eq!(
            track(
                Track::Live(21),
                T,
                Some((15, 21)),
                T,
                Some((15, 21)),
                Some(ImeNav::Right)
            ),
            Track::Live(21)
        );
    }

    #[test]
    fn clause_crossing_reanchors_one_char_in_from_entry_edge() {
        // Crossing left out of 天気 into いい lands [いIい] = byte 12.
        assert_eq!(
            track(
                Track::Live(15),
                T,
                Some((15, 21)),
                T,
                Some((9, 15)),
                Some(ImeNav::Left)
            ),
            Track::Live(12)
        );
        // Crossing right out of 今日は into いい lands [いIい] too (start+1).
        assert_eq!(
            track(
                Track::Live(9),
                T,
                Some((0, 9)),
                T,
                Some((9, 15)),
                Some(ImeNav::Right)
            ),
            Track::Live(12)
        );
    }

    #[test]
    fn full_measured_leftward_walk() {
        // The ①/② probe walk: wrap into 天気 (tentative), one step (live),
        // cross into いい, one step, cross into 今日は, two steps to its
        // head. Byte positions: (18) → 15 → 12 → 9 → 6 → 3 → 0.
        let mut s = track(
            Track::Idle,
            T,
            Some((0, 9)),
            T,
            Some((15, 21)),
            Some(ImeNav::Left),
        );
        assert_eq!(s, Track::Tentative(18));
        let steps: [(Option<(usize, usize)>, Track); 6] = [
            (Some((15, 21)), Track::Live(15)),
            (Some((9, 15)), Track::Live(12)),
            (Some((9, 15)), Track::Live(9)),
            (Some((0, 9)), Track::Live(6)),
            (Some((0, 9)), Track::Live(3)),
            (Some((0, 9)), Track::Live(0)),
        ];
        let mut prev_range = Some((15, 21));
        for (range, want) in steps {
            s = track(s, T, prev_range, T, range, Some(ImeNav::Left));
            assert_eq!(s, want);
            prev_range = range;
        }
        // Edge silence sends no event at all — state simply persists.
    }

    #[test]
    fn google_style_cycling_never_draws() {
        // Regression for the false caret under Google 日本語入力 (measured
        // 2026-07-22, ime_probe3): it splits 今日はいい天気 into two clauses
        // 今日は(0..9) / いい天気(9..21) and *cycles* selection at both
        // edges — every press changes the range, never a same-range fire.
        // The ← jump first→last matches the wrap signature, so it arms; the
        // corroborating step never comes, so no caret may ever be drawn.
        let full = Some((9, 21));
        let first = Some((0, 9));
        // → cycles last → first (clause-select, stays Idle).
        let mut s = track(Track::Idle, T, full, T, first, Some(ImeNav::Right));
        assert_eq!(s.caret(), None);
        // ← jumps first → last: the ambiguous signature. Arms, must not draw.
        s = track(s, T, first, T, full, Some(ImeNav::Left));
        assert_eq!(s.caret(), None);
        // Endless alternation keeps re-anchoring the tentative track; the
        // caret must stay invisible through all of it.
        let mut prev = full;
        for range in [first, full, first, full, first, full] {
            s = track(s, T, prev, T, range, Some(ImeNav::Left));
            assert_eq!(s.caret(), None, "cycling IME must never show a caret");
            prev = range;
        }
    }

    #[test]
    fn unexplained_updates_drop_tracking() {
        // Esc partial revert: text changes (きょうは back to kana) → Idle.
        assert_eq!(
            track(
                Track::Live(0),
                T,
                Some((0, 9)),
                "きょうはいい天気",
                Some((0, 0)),
                None
            ),
            Track::Idle
        );
        // Range collapses or disappears → Idle.
        assert_eq!(
            track(Track::Live(18), T, Some((15, 21)), T, Some((21, 21)), None),
            Track::Idle
        );
        assert_eq!(
            track(Track::Live(18), T, Some((15, 21)), T, None, None),
            Track::Idle
        );
        // Range jumps with no arrow hint (candidate ops, modifier'd keys) →
        // bail out rather than guess.
        assert_eq!(
            track(Track::Live(18), T, Some((15, 21)), T, Some((9, 15)), None),
            Track::Idle
        );
    }
}
