//! Gate: has the facade drifted from what it promises?
//!
//! `crates/sindon/src/lib.rs` states a rule — no third-party type an
//! application would have to name or construct appears in `sindon::*` —
//! and enforces it by hand, by re-exporting five crates item by item
//! instead of whole. Nothing checked that the hand-work stayed done.
//! Add `pub use sindon_render::Renderer;` to the facade, or a new `pub`
//! item to a curated crate, and every build stays green; the only
//! defence was that the facade is one short file somebody might reread.
//!
//! So this test makes the classification exhaustive. For each curated
//! crate, every public name at the crate root must be either
//!
//!   * re-exported by the facade, or
//!   * listed in [`DELIBERATELY_INTERNAL`] below, with a reason.
//!
//! A name that is neither fails the test. That is the whole idea: the
//! failure is not "you did something wrong", it is "a decision is
//! outstanding, and it is yours". Adding an item to `sindon_platform`
//! should cost one line in one of two places, chosen deliberately.
//!
//! # What this does not cover
//!
//! `core`, `reactive`, `security` and `widgets` are re-exported *whole*,
//! so they have no unclassified names by construction and this test says
//! nothing about them. That is not free: `sindon_widgets` depends on
//! `sindon_render`, `sindon_text` and `sindon_layout`, so a new
//! `pub fn` there whose signature names `wgpu::Device` or `taffy::Style`
//! would reach `sindon::widgets::*` and break the rule with this test
//! still green. Catching that needs the public API's *types*, not its
//! names, which on stable means rustdoc JSON, which means nightly. The
//! honest state is: the additive-drift half is covered here, the
//! type-leak half is still carried by review.
//!
//! [`WHOLE_REEXPORTS`] and [`CURATED`] are pinned for the same reason
//! the names are: turning a curated crate into a whole re-export is a
//! one-line change that would silently delete this test's subject
//! matter.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

/// Crates whose entire public API is sindon's own vocabulary, re-exported
/// whole as `sindon::<name>`.
const WHOLE_REEXPORTS: &[(&str, &str)] = &[
    ("sindon_core", "core"),
    ("sindon_reactive", "reactive"),
    ("sindon_security", "security"),
    ("sindon_widgets", "widgets"),
];

/// Crates re-exported item by item, and the facade module that does it.
const CURATED: &[(&str, &str)] = &[
    ("sindon_layout", "layout"),
    ("sindon_render", "render"),
    ("sindon_text", "text"),
    ("sindon_platform", "platform"),
    ("sindon_app", "app"),
];

/// Public at the crate root on purpose, and kept out of the facade on
/// purpose. Each entry is a decision that was made once; the reason is
/// there so the next person can disagree with it knowingly rather than
/// by accident.
///
/// Modules are written `mod name` to tell them from an item of the same
/// name. One name is genuinely both — `sindon_platform` has a
/// `system_locale` module *and* a `system_locale` function, and the
/// facade re-exports the function. Matching is by name, so the module
/// rides along on the function's classification. It is the only such
/// collision, and a new one would be invisible here.
const DELIBERATELY_INTERNAL: &[(&str, &str, &str)] = &[
    // ── sindon_layout ───────────────────────────────────────────────
    (
        "sindon_layout",
        "LayoutEngine",
        "engine-level; drives taffy",
    ),
    (
        "sindon_layout",
        "MeasureQuery",
        "engine-level; handed to widgets by the engine, never built by an app",
    ),
    (
        "sindon_layout",
        "LayoutNodeId",
        "is a taffy::NodeId — the type the curation exists to keep out",
    ),
    // ── sindon_render ───────────────────────────────────────────────
    (
        "sindon_render",
        "mod atlas",
        "flattened; nothing in it is app-facing",
    ),
    (
        "sindon_render",
        "mod image",
        "flattened; the facade re-exports its items directly",
    ),
    (
        "sindon_render",
        "mod renderer",
        "flattened; see the items below",
    ),
    ("sindon_render", "mod secure_atlas", "flattened"),
    ("sindon_render", "AtlasRegion", "renderer internals"),
    (
        "sindon_render",
        "TextureAtlas",
        "constructed from a wgpu::Device",
    ),
    (
        "sindon_render",
        "SecureTextureAtlas",
        "constructed from a wgpu::Device",
    ),
    (
        "sindon_render",
        "Renderer",
        "constructed from wgpu::Device / wgpu::Queue — integrator-level",
    ),
    ("sindon_render", "RenderError", "only Renderer returns it"),
    (
        "sindon_render",
        "RenderTimings",
        "only Renderer produces it",
    ),
    (
        "sindon_render",
        "DrawGlyph",
        "draw command; widgets paint through PaintContext instead",
    ),
    ("sindon_render", "DrawImage", "draw command; see DrawGlyph"),
    ("sindon_render", "DrawRect", "draw command; see DrawGlyph"),
    // ── sindon_text ─────────────────────────────────────────────────
    ("sindon_text", "Attrs", "cosmic-text re-export"),
    ("sindon_text", "CacheKey", "cosmic-text re-export"),
    ("sindon_text", "FontSystem", "cosmic-text re-export"),
    ("sindon_text", "Metrics", "cosmic-text re-export"),
    ("sindon_text", "Shaping", "cosmic-text re-export"),
    // ── sindon_platform ─────────────────────────────────────────────
    (
        "sindon_platform",
        "mod caret",
        "flattened to caret_blink_time",
    ),
    (
        "sindon_platform",
        "mod clipboard",
        "flattened; the facade re-exports its items directly",
    ),
    ("sindon_platform", "mod dialog", "flattened to FileDialog"),
    (
        "sindon_platform",
        "mod display_protection",
        "integrator-level; an app turns capture prevention on with App::capture_prevention",
    ),
    (
        "sindon_platform",
        "mod system_theme",
        "flattened to SystemTheme",
    ),
    (
        "sindon_platform",
        "mod window",
        "built from an Arc<winit::Window>",
    ),
    (
        "sindon_platform",
        "DisplayProtection",
        "built from an Arc<winit::Window>; App::capture_prevention is the app-level door",
    ),
    (
        "sindon_platform",
        "DisplayProtectionLevel",
        "only reached through DisplayProtection",
    ),
    (
        "sindon_platform",
        "DisplayProtectionResult",
        "only reached through DisplayProtection",
    ),
    (
        "sindon_platform",
        "PlatformWindow",
        "built from an Arc<winit::Window>",
    ),
    // ── sindon_app ──────────────────────────────────────────────────
    (
        "sindon_app",
        "mod a11y",
        "accesskit types reach its signatures; App::accessibility is the app-level door",
    ),
    (
        "sindon_app",
        "mod event_loop",
        "flattened; see the items above",
    ),
    (
        "sindon_app",
        "mod perf",
        "flattened to FRAME_BUDGET / FrameTimings / PerfSnapshot",
    ),
];

// ───────────────────────────── the tests ─────────────────────────────

#[test]
fn every_public_name_in_a_curated_crate_is_either_exported_or_declared_internal() {
    let facade = Facade::parse();
    let mut unclassified: Vec<String> = Vec::new();

    for (krate, module) in CURATED {
        let exported = facade.curated.get(*module).unwrap_or_else(|| {
            panic!("the facade no longer has a `pub mod {module}` — see the CURATED table")
        });

        for name in public_names(&read_lib(krate)) {
            let shown = name.to_string();
            if exported.contains(name.as_str()) || is_declared_internal(krate, &shown) {
                continue;
            }
            unclassified.push(format!("  {krate}::{shown}"));
        }
    }

    assert!(
        unclassified.is_empty(),
        "\n\
         These names are public at a curated crate's root, but the facade neither\n\
         re-exports them nor declares them internal:\n\
         \n{}\n\n\
         Decide, one line either way:\n\
         \n  \
         * app-facing  -> add it to the matching `pub mod` in crates/sindon/src/lib.rs\n  \
         * not         -> add it to DELIBERATELY_INTERNAL in this file, with the reason\n\
         \n\
         The rule the facade states is: no third-party type an application would\n\
         have to name or construct appears in `sindon::*`. If the new item's\n\
         signature names a wgpu, winit, taffy, accesskit or cosmic-text type, that\n\
         is the answer.\n",
        unclassified.join("\n")
    );
}

#[test]
fn the_internal_ledger_does_not_rot() {
    let facade = Facade::parse();
    let mut stale: Vec<String> = Vec::new();
    let mut contradicted: Vec<String> = Vec::new();

    for (krate, name, _why) in DELIBERATELY_INTERNAL {
        let public: BTreeSet<String> = public_names(&read_lib(krate))
            .iter()
            .map(PubName::to_string)
            .collect();
        if !public.contains(*name) {
            stale.push(format!("  {krate}::{name}"));
            continue;
        }
        // Bare item names are what the facade re-exports; `mod x` entries
        // can never collide with one, so only the former can contradict.
        if let Some((_, module)) = CURATED.iter().find(|(c, _)| c == krate)
            && facade.curated[*module].contains(*name)
        {
            contradicted.push(format!("  {krate}::{name}"));
        }
    }

    assert!(
        stale.is_empty(),
        "\nDELIBERATELY_INTERNAL names items that are no longer public:\n\n{}\n\n\
         They were removed or renamed. Drop these lines — a ledger nobody\n\
         prunes stops being read.\n",
        stale.join("\n")
    );
    assert!(
        contradicted.is_empty(),
        "\nDELIBERATELY_INTERNAL claims these are internal, but the facade\n\
         exports them:\n\n{}\n\n\
         One of the two is wrong, and the facade is the one users see.\n",
        contradicted.join("\n")
    );
}

#[test]
fn the_split_between_whole_and_curated_re_exports_is_pinned() {
    let facade = Facade::parse();

    let expected_whole: BTreeMap<String, String> = WHOLE_REEXPORTS
        .iter()
        .map(|(k, m)| ((*m).to_string(), (*k).to_string()))
        .collect();
    assert_eq!(
        facade.whole, expected_whole,
        "\nThe set of crates re-exported *whole* changed.\n\n\
         This is the load-bearing line of the curation: a crate re-exported whole\n\
         puts its entire public API into `sindon::*`, third-party types included,\n\
         and no test in this file can see that happen. If the change is intended,\n\
         update WHOLE_REEXPORTS — and check what the crate's API actually names.\n"
    );

    let expected_curated: BTreeSet<String> =
        CURATED.iter().map(|(_, m)| (*m).to_string()).collect();
    let actual_curated: BTreeSet<String> = facade.curated.keys().cloned().collect();
    assert_eq!(
        actual_curated, expected_curated,
        "\nThe set of item-by-item curated modules changed. Update CURATED.\n"
    );
}

#[test]
fn the_parser_can_still_read_what_it_is_pointed_at() {
    // Every assertion above is of the form "this set contains no
    // surprises", which a parser that silently returns nothing satisfies
    // perfectly. Anchor it: these names exist today, and if the parser
    // stops finding them it has broken, not the facade.
    for (krate, anchor) in [
        ("sindon_layout", "FlexStyle"),
        ("sindon_render", "Renderer"),
        ("sindon_text", "FontSystem"),
        ("sindon_platform", "mod storage"),
        ("sindon_app", "App"),
    ] {
        let names: BTreeSet<String> = public_names(&read_lib(krate))
            .iter()
            .map(PubName::to_string)
            .collect();
        assert!(
            names.contains(anchor),
            "parser found {} names in {krate} and `{anchor}` was not among them",
            names.len()
        );
    }

    let facade = Facade::parse();
    assert!(
        facade.curated["platform"].contains("SecureClipboard"),
        "parser lost the facade's platform re-exports"
    );
    assert!(
        facade.curated["app"].contains("AppHandle"),
        "parser lost the facade's app re-exports"
    );
}

// ─────────────────────────── reading the source ──────────────────────

fn is_declared_internal(krate: &str, name: &str) -> bool {
    DELIBERATELY_INTERNAL
        .iter()
        .any(|(k, n, _)| *k == krate && *n == name)
}

fn read_lib(krate: &str) -> String {
    // CARGO_MANIFEST_DIR is crates/sindon, so its parent holds the rest.
    // The sibling sources are not in the published .crate — the facade's
    // manifest excludes this directory for that reason.
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", krate, "src", "lib.rs"]
        .iter()
        .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// A name that is public at a crate root.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PubName {
    Module(String),
    Item(String),
}

impl PubName {
    /// The bare identifier, for matching against the facade's re-exports.
    fn as_str(&self) -> &str {
        match self {
            PubName::Module(n) | PubName::Item(n) => n,
        }
    }
}

impl fmt::Display for PubName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PubName::Module(n) => write!(f, "mod {n}"),
            PubName::Item(n) => write!(f, "{n}"),
        }
    }
}

/// The facade, as parsed out of its own source.
struct Facade {
    /// facade module name -> crate re-exported whole through it
    whole: BTreeMap<String, String>,
    /// facade module name -> the names it re-exports
    curated: BTreeMap<String, BTreeSet<String>>,
}

impl Facade {
    fn parse() -> Self {
        let src = strip_comments(
            &std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
                .expect("cannot read the facade's own lib.rs"),
        );

        // Inline `pub mod name { ... }` blocks are the curated modules;
        // lift them out first so the remainder is only top-level items.
        let (blocks, top_level) = split_off_inline_modules(&src);

        let curated = blocks
            .into_iter()
            .map(|(name, body)| {
                let names = pub_use_statements(&body)
                    .into_iter()
                    .flat_map(|s| exported_names(&s))
                    .collect();
                (name, names)
            })
            .collect();

        let mut whole = BTreeMap::new();
        for statement in pub_use_statements(&top_level) {
            // `pub use sindon_core as core;` — the only shape a whole
            // re-export can take.
            if let Some((path, alias)) = statement.split_once(" as ")
                && !path.contains("::")
            {
                whole.insert(alias.trim().to_string(), path.trim().to_string());
            }
        }

        Facade { whole, curated }
    }
}

/// Every public name declared at the root of a crate's `lib.rs`.
fn public_names(src: &str) -> BTreeSet<PubName> {
    let src = strip_comments(src);
    let mut names = BTreeSet::new();

    for line in src.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("pub mod ")
            && let Some(name) = rest.split([';', ' ', '{']).next()
            && !name.is_empty()
        {
            names.insert(PubName::Module(name.to_string()));
        }
    }
    for statement in pub_use_statements(&src) {
        for name in exported_names(&statement) {
            names.insert(PubName::Item(name));
        }
    }

    names
}

/// The body of every `pub use ...;` statement, brace groups joined onto
/// one line so a multi-line re-export reads the same as a single-line one.
fn pub_use_statements(src: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut rest = src;

    while let Some(at) = find_statement_start(rest, "pub use ") {
        let after = &rest[at + "pub use ".len()..];
        let Some(end) = after.find(';') else { break };
        let statement = after[..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        statements.push(statement);
        rest = &after[end + 1..];
    }

    statements
}

/// The names a `use` body brings into scope, i.e. what a re-export makes
/// public. `a::b::C` -> `C`; `a::{B, C as D}` -> `B`, `D`.
fn exported_names(statement: &str) -> Vec<String> {
    let leaf = |segment: &str| -> Option<String> {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        let name = match segment.split_once(" as ") {
            Some((_, alias)) => alias.trim(),
            None => segment.rsplit("::").next().unwrap_or(segment).trim(),
        };
        (!name.is_empty() && name != "self").then(|| name.to_string())
    };

    match (statement.find('{'), statement.rfind('}')) {
        (Some(open), Some(close)) if open < close => statement[open + 1..close]
            .split(',')
            .filter_map(leaf)
            .collect(),
        _ => leaf(statement).into_iter().collect(),
    }
}

/// Pull `pub mod name { ... }` blocks out, returning them and what is left.
fn split_off_inline_modules(src: &str) -> (Vec<(String, String)>, String) {
    let mut blocks = Vec::new();
    let mut remainder = String::new();
    let mut rest = src;

    while let Some(at) = find_statement_start(rest, "pub mod ") {
        let after = &rest[at + "pub mod ".len()..];
        let Some(brace) = after.find('{') else {
            remainder.push_str(&rest[..at + "pub mod ".len()]);
            rest = after;
            continue;
        };
        // A `;` before the brace means this was a plain declaration and
        // the brace belongs to something later.
        let head = &after[..brace];
        if head.contains(';') {
            let cut = at + "pub mod ".len() + head.find(';').unwrap() + 1;
            remainder.push_str(&rest[..cut]);
            rest = &rest[cut..];
            continue;
        }
        let name = head.trim().to_string();
        let body_start = brace + 1;
        let Some(body_len) = balanced_len(&after[body_start..]) else {
            break;
        };
        blocks.push((name, after[body_start..body_start + body_len].to_string()));
        remainder.push_str(&rest[..at]);
        rest = &after[body_start + body_len + 1..];
    }
    remainder.push_str(rest);

    (blocks, remainder)
}

/// Length of the text up to the `}` that closes the already-open brace.
fn balanced_len(src: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in src.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Offset of `needle` where it starts a line, so `// pub use ...` in
/// prose and `#[doc = "pub use"]` cannot be mistaken for code.
fn find_statement_start(src: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = src[from..].find(needle) {
        let at = from + rel;
        let line_start = src[..at].rfind('\n').map_or(0, |n| n + 1);
        if src[line_start..at].trim().is_empty() {
            return Some(at);
        }
        from = at + needle.len();
    }
    None
}

/// Drop `//` comments, including the doc comments that carry example
/// code — `//! pub use sindon_render::Renderer;` in prose must not read
/// as a re-export.
fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.trim_start().starts_with("//") {
            true => "",
            false => line.split("//").next().unwrap_or(""),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
