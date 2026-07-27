//! Verification Plan #4 — SecureInput end-to-end zeroize.
//!
//! What we're proving: when an app uses `SecureInput` the way Knot's
//! lock flow does, the typed secret leaves no recoverable trace in
//! process memory once the buffer is cleared and dropped.
//!
//! Lifecycle exercised (mirrors the password_manager / Knot lock pattern):
//! 1. Mount `SecureInput` in a real `WidgetTree`, bound to a
//!    `ClearTrigger` the "lock" handler will bump.
//! 2. Focus via `MouseDown`, type a runtime-generated 32-byte ASCII
//!    canary as a sequence of `CharInput` events — the same path live
//!    keystrokes take through `event_loop`.
//! 3. Paint once. `SecureInput::paint` shapes the *mask* (●●●) through
//!    cosmic-text, not the secret — Phase 42's design guarantee. A
//!    counter-scan asserts this didn't change the canary count.
//! 4. "Lock" = `trigger.bump()`; the next paint runs `sync_clear`,
//!    which calls `SecureString::clear` (zeroize-then-truncate).
//! 5. Drop the tree, taking `SecureInput` and its `SecureString` with
//!    it. The `zeroize` crate also fires on drop as a second line.
//!
//! The hard assertion is `post_drop_residue == 0`: scanning every
//! committed, readable user-mode page in this process must not find the
//! canary anywhere except the `Vec<u8>` that holds the needle for the
//! scanner itself.
//!
//! Scanner is duplicated from `crates/sindon_text/tests/cosmic_residue.rs`.
//! Both tests walk this process's address space the same way; when a
//! third consumer appears, factor the walker into a shared dev-dep
//! helper. For two, duplication keeps the test binaries self-contained.
//!
//! Windows-only (`VirtualQueryEx` + `ReadProcessMemory`). Linux /
//! macOS equivalents would need their own walker; not in scope today.
//!
//! Both tests count a canary across the *whole process's* memory, so no two
//! scans may overlap in time: a live scanner's scratch holds a copy of the
//! last window it read, and this test's canary sitting in *someone else's*
//! scratch is indistinguishable from a real leak. The scratch is therefore
//! process-wide and every scan holds it for the whole walk — see `Scratch`.
//! Nothing here depends on `--test-threads=1`.

#![cfg(windows)]

use std::sync::{Mutex, OnceLock};

use sindon_core::{Point, Rect};
use sindon_widgets::{
    ClearTrigger, Container, EventContext, MouseButton, PaintContext, SecureInput, Widget,
    WidgetEvent, WidgetTree,
};

use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY, VirtualQueryEx,
};
use windows::Win32::System::Threading::GetCurrentProcess;

// ─── Scanner (duplicated from cosmic_residue.rs) ──────────────────────────

/// Build a 32-byte canary from runtime-only sources (process id + clock).
/// Bytes are ASCII A–Z so the pattern is easy to recognize in a dump and
/// will not collide with arbitrary binary content. Crucially, the canary
/// never appears as a string literal in the compiled binary — every byte
/// is computed at runtime.
fn build_canary() -> Vec<u8> {
    let pid = std::process::id() as u64;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = pid.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(nanos);
    let mut out = Vec::with_capacity(32);
    for _ in 0..32u32 {
        // splitmix64 step
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Map to A–Z range (avoid 0x00 / printable-only).
        out.push(b'A' + ((z as u8) % 26));
    }
    out
}

/// Returns true if a page protection value indicates the page is readable.
fn is_readable(protect: u32) -> bool {
    if protect & PAGE_GUARD.0 != 0 {
        return false;
    }
    if protect == PAGE_NOACCESS.0 {
        return false;
    }
    // Any of these read-capable flags.
    let read_flags = PAGE_READONLY.0
        | PAGE_READWRITE.0
        | PAGE_WRITECOPY.0
        | PAGE_EXECUTE_READ.0
        | PAGE_EXECUTE_READWRITE.0
        | PAGE_EXECUTE_WRITECOPY.0;
    (protect & read_flags) != 0
}

/// Where a single match was found: absolute VA plus its owning region's
/// geometry, for flake forensics.
#[derive(Debug)]
struct MatchDetail {
    addr: usize,
    region_base: usize,
    region_size: usize,
    protect: u32,
    mem_type: u32,
}

#[derive(Default, Debug)]
struct ScanReport {
    matches: usize,
    matches_in_private: usize,
    matches_in_image: usize,
    matches_in_mapped: usize,
    regions_scanned: usize,
    bytes_scanned: u64,
    /// Per-match location (capped at 64). A match whose address falls inside
    /// the scanner's own scratch buffer is by construction an artifact: the
    /// scratch only ever holds copies of memory that is already counted at
    /// its real address.
    match_details: Vec<MatchDetail>,
}

/// The one scratch buffer in this process, at a fixed address, behind a lock.
///
/// A match found inside a scratch is a phantom by construction: a scratch only
/// ever holds a copy of a window the walk already counted at its real address.
/// There are two ways a scan can end up reading one, and both produced flakes:
///
/// 1. **Its own** (fixed 2026-07, `2aba6f2`). `ReadProcessMemory` with
///    overlapping src/dst is a plain forward copy — NOT a no-op — so reading
///    the chunk that contains the scratch smears the sliver
///    `[window_start, scratch_start)` across the whole window with period
///    `scratch_start - window_start`, replicating any real canary in that
///    sliver into `floor(64K/period)` phantom matches. That was the CI-only
///    "residue after drop" flake: dropping the widget tree resized the
///    scratch's heap region, shifting the chunk grid so the needle Vec
///    (allocated just before the scratch, hence nearby on the heap) landed in
///    the sliver with a small period.
///
/// 2. **A sibling's** (fixed 2026-07-17). Both tests in this binary scan, and
///    libtest runs them on parallel threads, so their scratches were allocated
///    back-to-back on the heap. Each scanner hopped over its own and then read
///    its sibling's mid-churn, counting the sibling's snapshot of this test's
///    canary. Being a torn read of a moving buffer, the phantom count wobbled
///    with how the two walks interleaved: landing on `baseline` flaked the
///    `typing_copies >= 1` sanity check, while landing on a later phase fired
///    the paint/residue **security** asserts with a false "the secret leaked"
///    message. Only reproducible under load (`cargo test -p sindon_widgets`).
///
/// One process-wide scratch, held for the whole walk, closes both: there is a
/// single range to skip, and no other scan can be writing into it while we
/// read. Skipping it loses no detection power — the bytes it holds are counted
/// at their real address by the window that owns them.
struct Scratch {
    buf: Mutex<Box<[u8]>>,
    start: usize,
    end: usize,
}

fn scratch() -> &'static Scratch {
    static SCRATCH: OnceLock<Scratch> = OnceLock::new();
    SCRATCH.get_or_init(|| {
        let buf = vec![0u8; 1 << 16].into_boxed_slice();
        // Boxing pins the allocation, so this range is stable for the life of
        // the process even as the Box moves into the Mutex.
        let (start, len) = (buf.as_ptr() as usize, buf.len());
        Scratch {
            buf: Mutex::new(buf),
            start,
            end: start + len,
        }
    })
}

/// Handle to the shared scratch, held across a test's phases so no allocation
/// or free of the scratch (or of its containing heap region) can shift the
/// chunk grid between one scan and the next.
struct Scanner {
    scratch: &'static Scratch,
}

impl Scanner {
    fn new() -> Self {
        Self { scratch: scratch() }
    }
}

fn scan_self_for(scanner: &mut Scanner, needle: &[u8]) -> ScanReport {
    assert!(!needle.is_empty());
    let mut report = ScanReport::default();
    let hproc = unsafe { GetCurrentProcess() };

    let mut addr: usize = 0;
    // Conservative user-mode upper bound on x64 Windows.
    let max_addr: usize = 0x7FFF_FFFE_0000;
    // Hold the process-wide scratch for the entire walk. A concurrent scan
    // would be churning canary copies through memory we are counting.
    let mut guard = scanner
        .scratch
        .buf
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let buf = &mut *guard;
    let scratch_start = scanner.scratch.start;
    let scratch_end = scanner.scratch.end;

    while addr < max_addr {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let mbi_size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
        let queried = unsafe { VirtualQueryEx(hproc, Some(addr as *const _), &mut mbi, mbi_size) };
        if queried == 0 {
            break;
        }
        let region_base = mbi.BaseAddress as usize;
        let region_size = mbi.RegionSize;
        let next = region_base.saturating_add(region_size);
        if next <= addr {
            break;
        }

        let committed = mbi.State == MEM_COMMIT;
        if committed && is_readable(mbi.Protect.0) {
            report.regions_scanned += 1;
            let is_image = mbi.Type == MEM_IMAGE;
            let is_private = !is_image && (mbi.Type.0 & 0x0002_0000) != 0; // MEM_PRIVATE
            let mut offset = 0usize;
            // To catch a match that straddles a chunk boundary, the next
            // chunk overlaps the previous by (needle.len() - 1) bytes.
            let overlap = needle.len() - 1;
            while offset < region_size {
                let read_start = region_base + offset;
                // Never read a window that overlaps the scratch itself: an
                // overlapping ReadProcessMemory forward-copies over its own
                // source, smearing the sliver before the scratch into
                // phantom canary copies (see the Scratch doc). Hop over it.
                if read_start >= scratch_start && read_start < scratch_end {
                    offset = (scratch_end - region_base).min(region_size);
                    continue;
                }
                let mut want = (region_size - offset).min(buf.len());
                if read_start < scratch_start && read_start + want > scratch_start {
                    // Stop just short of the scratch; a match straddling
                    // into it could only be scratch bytes, not real data.
                    want = scratch_start - read_start;
                    if want < needle.len() {
                        offset = (scratch_end - region_base).min(region_size);
                        continue;
                    }
                }
                let mut nread: usize = 0;
                let ok = unsafe {
                    ReadProcessMemory(
                        hproc,
                        (region_base + offset) as *const _,
                        buf.as_mut_ptr() as *mut _,
                        want,
                        Some(&mut nread),
                    )
                };
                if ok.is_ok() && nread >= needle.len() {
                    report.bytes_scanned += nread as u64;
                    let mut i = 0;
                    while i + needle.len() <= nread {
                        if &buf[i..i + needle.len()] == needle {
                            report.matches += 1;
                            if report.match_details.len() < 64 {
                                report.match_details.push(MatchDetail {
                                    addr: region_base + offset + i,
                                    region_base,
                                    region_size,
                                    protect: mbi.Protect.0,
                                    mem_type: mbi.Type.0,
                                });
                            }
                            if is_image {
                                report.matches_in_image += 1;
                            } else if is_private {
                                report.matches_in_private += 1;
                            } else {
                                report.matches_in_mapped += 1;
                            }
                            // Non-overlapping count.
                            i += needle.len();
                        } else {
                            i += 1;
                        }
                    }
                }
                // Advance by chunk minus overlap so a straddling match is
                // still seen, but only if we actually progressed.
                if nread == 0 {
                    break;
                }
                offset += nread.saturating_sub(overlap).max(1);
            }
        }
        addr = next;
    }
    report
}

/// Print every recorded match location, flagging matches whose address falls
/// inside the scanner's own scratch buffer — those are scanner artifacts by
/// construction (the scratch only ever holds copies of other memory).
fn dump_match_forensics(label: &str, report: &ScanReport, scanner: &Scanner) {
    let scratch_start = scanner.scratch.start;
    let scratch_end = scanner.scratch.end;
    eprintln!("  {label}: {} match(es)", report.matches);
    for d in &report.match_details {
        let tag = if d.addr >= scratch_start && d.addr < scratch_end {
            "  <-- INSIDE SCANNER SCRATCH (artifact)"
        } else {
            ""
        };
        eprintln!(
            "    @ {:#014x}  region=[{:#014x} +{:#x}] prot={:#x} type={:#x}{}",
            d.addr, d.region_base, d.region_size, d.protect, d.mem_type, tag
        );
    }
}

// ─── Verification Plan #4 ─────────────────────────────────────────────────

#[test]
fn secure_input_clear_leaves_no_canary_in_memory() {
    // The needle is a 32-byte runtime-only ASCII pattern; collision with
    // an unrelated process page is ~26^-32 ≈ 10^-45.
    let canary = build_canary();
    let canary_str = std::str::from_utf8(&canary).expect("ASCII A-Z by construction");

    // Pre-allocate the scanner so we don't create/drop its scratch
    // buffer (and its containing region) between phases.
    let mut scanner = Scanner::new();
    // Warm-up scan so the first measurement isn't biased by lazy
    // allocations the runtime does on first use.
    let _ = scan_self_for(&mut scanner, &canary);

    // ── Phase 0: baseline ─────────────────────────────────────────────
    // The Vec<u8> needle is the only buffer holding the pattern.
    let baseline = scan_self_for(&mut scanner, &canary);

    // ── Phase 1: lock flow (type → paint → bump-clear → drop) ─────────
    let (live_after_type, live_after_paint, after_clear) = {
        let trigger = ClearTrigger::new();
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(400.0).height(100.0));
        let input_idx = tree.add_child(root, SecureInput::new().clear_on(trigger));
        tree.compute_layout(400.0, 100.0);

        // Focus the input so subsequent CharInput events land in the
        // buffer. This is the same MouseDown → FocusGained path
        // event_loop drives live.
        let mut event_ctx = EventContext::new();
        let rect = tree.layout_rect(input_idx);
        tree.dispatch_event(
            &WidgetEvent::MouseDown {
                position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
                button: MouseButton::Left,
            },
            &mut event_ctx,
        );

        // Type each canary byte as a CharInput so the SecureString
        // buffer ends up holding the full pattern. SecureInput's
        // `max_bytes` default is 256, so all 32 bytes fit without a
        // realloc (Phase 20's capacity guard).
        for ch in canary_str.chars() {
            tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut event_ctx);
        }
        let live_after_type = scan_self_for(&mut scanner, &canary);

        // Paint — should shape only the *mask* (●●●) through cosmic-text,
        // never the canary. Phase 42 / cosmic-text residue verification
        // claims this; we test it here by diffing against the post-type
        // scan. If paint adds copies, the secret leaked into shaping.
        let mut paint_ctx = PaintContext::default();
        tree.paint(&mut paint_ctx);
        let live_after_paint = scan_self_for(&mut scanner, &canary);

        // "Lock" — bump the trigger; the next paint runs `sync_clear`,
        // which calls `SecureString::clear` (zeroize-then-truncate). The
        // app-visible Knot/password_manager lock flow ends here.
        trigger.bump();
        tree.paint(&mut paint_ctx);

        // Scan with the SecureInput *still alive* but cleared, so we
        // know `clear` alone (no drop) is sufficient for residue 0.
        let after_clear = scan_self_for(&mut scanner, &canary);

        // Tree drops here → SecureInput drops → SecureString drops →
        // zeroize-on-drop fires as a second line of defence.
        (live_after_type, live_after_paint, after_clear)
    };

    // ── Phase 2: post-drop ───────────────────────────────────────────
    let after_drop = scan_self_for(&mut scanner, &canary);

    // ── Assertions ────────────────────────────────────────────────────
    let typing_copies = live_after_type.matches.saturating_sub(baseline.matches);
    let paint_extra = live_after_paint
        .matches
        .saturating_sub(live_after_type.matches);
    let post_clear_residue = after_clear.matches.saturating_sub(baseline.matches);
    let post_drop_residue = after_drop.matches.saturating_sub(baseline.matches);

    eprintln!("=== Verification Plan #4: SecureInput end-to-end ===");
    eprintln!("canary length: {} bytes", canary.len());
    eprintln!("baseline matches:                  {}", baseline.matches);
    eprintln!(
        "after typing:                      {}  (+{} via SecureString buffer)",
        live_after_type.matches, typing_copies
    );
    eprintln!(
        "after paint:                       {}  (+{} via cosmic-text — should be 0)",
        live_after_paint.matches, paint_extra
    );
    eprintln!(
        "after trigger.bump + paint:        {}  (residue: +{}; should be 0)",
        after_clear.matches, post_clear_residue
    );
    eprintln!(
        "after tree drop:                   {}  (residue: +{}; should be 0)",
        after_drop.matches, post_drop_residue
    );
    eprintln!(
        "regions scanned: {}, bytes scanned: {:.1} MiB",
        after_drop.regions_scanned,
        after_drop.bytes_scanned as f64 / (1024.0 * 1024.0)
    );
    eprintln!(
        "scratch=[{:#014x}..{:#014x}) needle@{:#014x}",
        scanner.scratch.start,
        scanner.scratch.end,
        canary.as_ptr() as usize
    );
    dump_match_forensics("baseline", &baseline, &scanner);
    dump_match_forensics("after_type", &live_after_type, &scanner);
    dump_match_forensics("after_paint", &live_after_paint, &scanner);
    dump_match_forensics("after_clear", &after_clear, &scanner);
    dump_match_forensics("after_drop", &after_drop, &scanner);

    // Sanity: typing must have produced a detectable copy. Otherwise
    // the scanner is broken or SecureInput silently dropped the input.
    assert!(
        typing_copies >= 1,
        "typing the canary must produce at least one extra copy in memory \
         (baseline={}, after_type={})",
        baseline.matches,
        live_after_type.matches
    );

    // Phase 42 / cosmic-text residue verification claim: SecureInput
    // shapes only the mask through cosmic-text, so the password never
    // reaches the shaper. If this asserts, that design claim is broken.
    assert_eq!(
        paint_extra, 0,
        "paint must not push the canary into cosmic-text \
         (saw +{} new copies during paint — SecureInput is leaking the \
         secret into the shaper)",
        paint_extra
    );

    // The clear flow alone must zero the buffer — no drop required.
    // This is the actual Knot lock-screen guarantee: the user expects
    // their master key to be unrecoverable the moment they hit Ctrl+L,
    // not just when the app exits.
    assert_eq!(
        post_clear_residue, 0,
        "ClearTrigger.bump + paint (sync_clear) must zeroize the \
         SecureString buffer (residue +{})",
        post_clear_residue
    );

    // Drop is the second line of defence; with clear already at 0, drop
    // must stay at 0. Asserting separately catches an allocator-reuse
    // regression where a freed buffer somehow gets re-stamped with stale
    // bytes between the clear scan and the post-drop scan.
    assert_eq!(
        post_drop_residue, 0,
        "after full drop the SecureString buffer's bytes must be gone \
         (residue +{})",
        post_drop_residue
    );
}

/// Reveal-toggle counterpart to the test above.
///
/// The masked path never feeds the secret to cosmic-text, so the test above
/// asserting `paint_extra == 0` is almost trivially true for it. The reveal
/// path is the interesting case: painting a *revealed* `SecureInput` shapes the
/// **real** plaintext through cosmic-text (`shape_text_uncached`). This is only
/// safe because the vendored cosmic-text fork holds each shaped line in a
/// `Zeroizing<String>` that wipes when the transient shape buffer drops at the
/// end of the call — the whole reason the fork exists.
///
/// So: type a canary, reveal it, paint, and assert the revealed paint added
/// **zero** persistent copies beyond the `SecureString` buffer itself. If the
/// fork's zeroize were dropped (or reveal routed through the cached/persistent
/// path), the shaped plaintext would linger and this would fail.
#[test]
fn secure_input_reveal_paint_leaves_no_canary() {
    let canary = build_canary();
    let canary_str = std::str::from_utf8(&canary).expect("ASCII A-Z by construction");

    let mut scanner = Scanner::new();
    let _ = scan_self_for(&mut scanner, &canary); // warm-up
    let baseline = scan_self_for(&mut scanner, &canary);

    let (after_type, after_reveal_paint) = {
        // Bare widget (no tree) so we can assert `is_revealed()` directly and
        // aim the eye click precisely. Field is 400x40 → the eye zone is the
        // trailing 40px square (x ∈ [360, 400]).
        let mut input = SecureInput::new().revealable();
        let rect = Rect::new(0.0, 0.0, 400.0, 40.0);
        let mut ev = EventContext::new();

        input.event(&WidgetEvent::FocusGained, rect, &mut ev);
        for ch in canary_str.chars() {
            input.event(&WidgetEvent::CharInput { ch }, rect, &mut ev);
        }
        let after_type = scan_self_for(&mut scanner, &canary);

        // Click the eye → reveal.
        input.event(
            &WidgetEvent::MouseDown {
                position: Point::new(382.0, 20.0),
                button: MouseButton::Left,
            },
            rect,
            &mut ev,
        );
        assert!(input.is_revealed(), "clicking the eye must reveal");

        // Paint the revealed field: shapes the REAL secret via cosmic-text.
        let mut p = PaintContext::default();
        input.paint(rect, &mut p);
        assert!(
            !p.secure_glyphs.is_empty(),
            "a revealed field must actually shape the secret (into the secure atlas)"
        );
        let after_reveal_paint = scan_self_for(&mut scanner, &canary);

        (after_type, after_reveal_paint)
    };

    let typing_copies = after_type.matches.saturating_sub(baseline.matches);
    let reveal_paint_extra = after_reveal_paint
        .matches
        .saturating_sub(after_type.matches);

    eprintln!("=== SecureInput reveal-paint residue ===");
    eprintln!("baseline matches:            {}", baseline.matches);
    eprintln!(
        "after typing:                {}  (+{} via SecureString buffer)",
        after_type.matches, typing_copies
    );
    eprintln!(
        "after revealed paint:        {}  (+{} via cosmic-text — should be 0)",
        after_reveal_paint.matches, reveal_paint_extra
    );
    eprintln!(
        "scratch=[{:#014x}..{:#014x}) needle@{:#014x}",
        scanner.scratch.start,
        scanner.scratch.end,
        canary.as_ptr() as usize
    );
    dump_match_forensics("baseline", &baseline, &scanner);
    dump_match_forensics("after_type", &after_type, &scanner);
    dump_match_forensics("after_reveal_paint", &after_reveal_paint, &scanner);

    assert!(
        typing_copies >= 1,
        "typing the canary must produce at least one copy \
         (baseline={}, after_type={})",
        baseline.matches,
        after_type.matches
    );

    assert_eq!(
        reveal_paint_extra, 0,
        "revealing shapes the real secret through the uncached + zeroizing \
         cosmic-text fork, so the shaped plaintext must not survive the paint \
         call (saw +{} extra copies — the fork's zeroize is not firing, or \
         reveal is routing through a retained path)",
        reveal_paint_extra
    );
}
