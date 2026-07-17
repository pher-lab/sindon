//! cosmic-text residue verification.
//!
//! Question we're answering: when a `SecureString` is shaped through
//! `TextEngine::shape_text`, does cosmic-text leave a plaintext copy of
//! the secret somewhere in process memory after the buffer is dropped?
//!
//! Method: scan our own process memory for a unique runtime-generated
//! canary, before/during/after the secret is exposed to cosmic-text.
//!
//! This test only runs on Windows (uses VirtualQueryEx + ReadProcessMemory).
//! It is a hard gate: once the secret and its shaped buffer are dropped, zero
//! canary copies may remain on the heap (`final_residue == 0`). This holds
//! because shaping goes through shroud's vendored fork of cosmic-text, whose
//! `BufferLine` zeroizes its plaintext on drop (see `third_party/cosmic-text`).
//!
//! A scan counts a canary across the *whole process's* memory, so no two scans
//! may overlap in time: a live scanner's scratch holds a copy of the last
//! window it read, and this test's canary sitting in *someone else's* scratch
//! is indistinguishable from a real leak. The scratch is therefore process-wide
//! and every scan holds it for the whole walk — see `Scratch`. Keep that true
//! if a second `#[test]` is ever added to this file: libtest would run it on a
//! parallel thread, and the scanner is what stands between a torn read and a
//! false accusation that the secret leaked.
//!
//! This scanner is duplicated in `crates/shroud_widgets/tests/zeroize_verification.rs`
//! (that file's module doc carries the standing note); a fix to one belongs in
//! the other.

#![cfg(windows)]

use std::sync::{Mutex, OnceLock};

use shroud_security::SecureString;
use shroud_text::TextEngine;

use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY, VirtualQueryEx,
};
use windows::Win32::System::Threading::GetCurrentProcess;

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
///    "residue after drop" flake: dropping the shaped tree resized the
///    scratch's heap region, shifting the chunk grid so the needle Vec
///    (allocated just before the scratch, hence nearby on the heap) landed in
///    the sliver with a small period.
///
/// 2. **A sibling's** (fixed 2026-07-17, `0d6861b`, in this scanner's
///    duplicate in `shroud_widgets` — that binary has two scanning tests).
///    libtest runs a binary's tests on parallel threads, and the two scratches
///    were allocated back-to-back on the heap, so each scanner hopped over its
///    own and then read its sibling's mid-churn, counting the sibling's
///    snapshot of the canary. Being a torn read of a moving buffer, the
///    phantom count wobbled with how the two walks interleaved: landing on
///    `baseline` flaked the "shaping added a copy" sanity check, while landing
///    on a later phase fired the residue **security** assert with a false "the
///    secret leaked" message. This file has exactly one `#[test]`, so it has
///    never been hit here — the point is that adding a second one would
///    otherwise silently reintroduce it.
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

#[test]
fn cosmic_text_residue_after_drop() {
    let canary = build_canary();
    let canary_str = std::str::from_utf8(&canary).expect("canary is valid ASCII by construction");

    // Pre-allocate the scanner so we don't create/drop the scratch
    // buffer (and its containing region) between phases.
    let mut scanner = Scanner::new();

    // Warm-up scan to settle any lazy allocations the runtime does on
    // first measurement.
    let _ = scan_self_for(&mut scanner, &canary);

    // -- Phase 0: baseline --------------------------------------------------
    // Only `canary` (this Vec<u8>) holds the pattern at this point.
    let baseline = scan_self_for(&mut scanner, &canary);

    // -- Phase 1: live (secret + shape held) --------------------------------
    let (live, initial_residue_floor) = {
        let secret = SecureString::new(canary_str);
        let mut engine = TextEngine::new();
        // Shape the secret through cosmic-text. The output `ShapedText`
        // does NOT carry the text, only glyph cache keys + positions —
        // so any post-call residue lives in cosmic-text internals or in
        // freed-but-unzeroed heap.
        let _shaped = secret.expose(|s| engine.shape_text(s, 16.0, 20.0, None));
        let live = scan_self_for(&mut scanner, &canary);
        // Drop engine first (BufferLine inside `shape_text` is already
        // dropped, but FontSystem caches may hold derived state).
        drop(engine);
        let after_engine = scan_self_for(&mut scanner, &canary);
        (live, after_engine)
        // `secret` and `_shaped` are dropped at end of scope.
    };

    // -- Phase 2: after drop ------------------------------------------------
    let after_drop = scan_self_for(&mut scanner, &canary);

    // -- Report --------------------------------------------------------------
    let initial_copies = live.matches.saturating_sub(baseline.matches);
    let after_engine_only_residue = initial_residue_floor
        .matches
        .saturating_sub(baseline.matches);
    let final_residue = after_drop.matches.saturating_sub(baseline.matches);

    eprintln!("=== cosmic-text residue verification ===");
    eprintln!("canary length: {} bytes", canary.len());
    eprintln!("baseline matches:                    {}", baseline.matches);
    eprintln!("  (Vec<u8> needle only)");
    eprintln!(
        "live matches:                        {}  (+{} copies during shape)",
        live.matches, initial_copies
    );
    eprintln!(
        "  private={} image={} mapped={}",
        live.matches_in_private, live.matches_in_image, live.matches_in_mapped
    );
    eprintln!(
        "after engine drop:                   {}  (residue floor: +{})",
        initial_residue_floor.matches, after_engine_only_residue
    );
    eprintln!(
        "after secret drop:                   {}  (final residue: +{})",
        after_drop.matches, final_residue
    );
    eprintln!(
        "  private={} image={} mapped={}",
        after_drop.matches_in_private, after_drop.matches_in_image, after_drop.matches_in_mapped
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
    dump_match_forensics("live", &live, &scanner);
    dump_match_forensics("after_engine_drop", &initial_residue_floor, &scanner);
    dump_match_forensics("after_drop", &after_drop, &scanner);

    // Sanity: while the secret is live we must be able to find its canary,
    // otherwise the scanner is broken and the `final_residue == 0` gate below
    // would be a false pass. (The live copy is the `SecureString` itself;
    // cosmic-text's own line buffer is already wiped by the time we scan here,
    // since its `Buffer` is dropped inside `shape_text`.)
    assert!(
        initial_copies >= 1,
        "scanner found no canary while the secret was live (baseline={}, live={}); \
         scanner may be broken — a residue==0 result cannot be trusted",
        baseline.matches,
        live.matches
    );

    // The gate: once the secret and the shaped buffer are dropped, not one byte
    // of plaintext may survive on the heap. This holds because shroud vendors a
    // fork of cosmic-text whose `BufferLine` zeroizes its text on drop (see
    // `third_party/cosmic-text` and shroud_text's crate docs). Before the fork
    // this was reproducibly +1 (one un-zeroed copy per shape); the fork drives
    // it to 0. Regressing the fork — or routing a secret through an un-forked
    // cosmic-text — trips this assert.
    assert_eq!(
        final_residue, 0,
        "plaintext residue survived secret drop: {final_residue} canary copies \
         still on the heap (expected 0). The cosmic-text fork's drop-zeroize may \
         have regressed.",
    );
}
