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
//! Scanner is duplicated from `crates/shroud_text/tests/cosmic_residue.rs`.
//! Both tests walk this process's address space the same way; when a
//! third consumer appears, factor the walker into a shared dev-dep
//! helper. For two, duplication keeps the test binaries self-contained.
//!
//! Windows-only (`VirtualQueryEx` + `ReadProcessMemory`). Linux /
//! macOS equivalents would need their own walker; not in scope today.

#![cfg(windows)]

use shroud_core::Point;
use shroud_widgets::{
    ClearTrigger, Container, EventContext, MouseButton, PaintContext, SecureInput, WidgetEvent,
    WidgetTree,
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

#[derive(Default, Debug)]
struct ScanReport {
    matches: usize,
    matches_in_private: usize,
    matches_in_image: usize,
    matches_in_mapped: usize,
    regions_scanned: usize,
    bytes_scanned: u64,
}

/// Scanner with a fixed-address scratch buffer. The scratch is zeroed
/// before every read so that, when the walk crosses scratch's own
/// address (a self-read which the kernel turns into a no-op), we don't
/// retain canary bytes from the previous chunk and produce a spurious
/// match.
struct Scanner {
    buf: Box<[u8]>,
}

impl Scanner {
    fn new() -> Self {
        let buf = vec![0u8; 1 << 16].into_boxed_slice();
        Self { buf }
    }
}

fn scan_self_for(scanner: &mut Scanner, needle: &[u8]) -> ScanReport {
    assert!(!needle.is_empty());
    let mut report = ScanReport::default();
    let hproc = unsafe { GetCurrentProcess() };

    let mut addr: usize = 0;
    // Conservative user-mode upper bound on x64 Windows.
    let max_addr: usize = 0x7FFF_FFFE_0000;
    let buf = &mut scanner.buf;

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
                let want = (region_size - offset).min(buf.len());
                // Zero scratch before every read. If the kernel turns
                // this into a self-read (when we happen to be reading
                // scratch's own pages), we'll see zeros instead of the
                // previous chunk's content — and so won't double-count
                // a canary that's actually elsewhere.
                for b in buf.iter_mut() {
                    *b = 0;
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
