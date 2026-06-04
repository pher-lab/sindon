//! Dependency-free "lite" syntax highlighter for fenced code blocks.
//!
//! Knot's preview deliberately avoids `syntect`: it pulls a large dependency
//! (plus its own grammar/theme files) into a privacy-first, self-built app, and
//! a notes preview doesn't need editor-grade accuracy. Instead this module does
//! a small char-by-char tokenize per language and classifies each slice into one
//! of a handful of [`TokenClass`]es. `preview.rs` maps those classes onto theme
//! color tokens, so highlighting tracks a Light/Dark/System swap like the rest
//! of the preview.
//!
//! The tokenizer is intentionally approximate — enough to make a code block
//! *read* as code (comments recede, strings/keywords/numbers pop) without
//! claiming to parse the language. It handles: line comments, block comments
//! (which carry across line boundaries), string/char literals (with backslash
//! escapes, single-line only), numbers, and keyword lookup for identifiers.
//! Everything else is [`TokenClass::Plain`].
//!
//! An unknown or absent language returns `None` from [`highlight`]; the caller
//! then renders the block with no highlighting (one base color), exactly as
//! before this module existed.

/// Semantic class for a highlighted slice of source. `preview.rs` resolves each
/// to a theme color; [`Plain`](TokenClass::Plain) inherits the widget's base
/// (reactive) color rather than overriding it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenClass {
    /// Identifiers, punctuation, whitespace — the default code foreground.
    Plain,
    /// A language keyword.
    Keyword,
    /// A string or char literal (including its delimiters).
    Str,
    /// A numeric literal.
    Number,
    /// A line or block comment (including its markers).
    Comment,
}

/// One classified, contiguous slice of a source line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Token {
    pub text: String,
    pub class: TokenClass,
}

/// Grammar knobs for one language family. Kept tiny on purpose: the tokenizer is
/// approximate, so a language is described by its comment markers, the quote
/// characters that open a string, and a flat keyword set.
struct Lang {
    /// Line-comment markers (e.g. `//`, `#`). Everything from the first match to
    /// end of line is a comment.
    line_comments: &'static [&'static str],
    /// Block-comment delimiters `(open, close)`, if the language has them. State
    /// carries across lines.
    block_comment: Option<(&'static str, &'static str)>,
    /// Characters that open a string/char literal. The same character closes it.
    string_delims: &'static [char],
    /// Reserved words rendered as [`TokenClass::Keyword`].
    keywords: &'static [&'static str],
}

// ── Keyword tables ──────────────────────────────────────────────────────────

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "union",
    "unsafe", "use", "where", "while",
];

// A generous superset across C/C++/Java/JavaScript/TypeScript/Go/Kotlin/Swift/
// C#. A lite highlighter doesn't need per-dialect precision — an extra keyword
// that another dialect uses as an identifier is a rare, harmless mis-tint.
const C_LIKE_KEYWORDS: &[&str] = &[
    "abstract",
    "auto",
    "bool",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "constexpr",
    "continue",
    "debugger",
    "default",
    "defer",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "export",
    "extends",
    "extern",
    "false",
    "final",
    "finally",
    "float",
    "fn",
    "for",
    "func",
    "function",
    "go",
    "goto",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "int",
    "interface",
    "let",
    "long",
    "namespace",
    "new",
    "null",
    "nullptr",
    "package",
    "private",
    "protected",
    "public",
    "range",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "super",
    "switch",
    "template",
    "this",
    "throw",
    "throws",
    "true",
    "try",
    "typedef",
    "typeof",
    "union",
    "unsigned",
    "val",
    "var",
    "virtual",
    "void",
    "volatile",
    "while",
    "yield",
];

const PYTHON_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "True", "try",
    "while", "with", "yield",
];

const SHELL_KEYWORDS: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in",
    "local", "readonly", "return", "select", "then", "until", "while",
];

const JSON_KEYWORDS: &[&str] = &["true", "false", "null"];

// ── Language definitions ────────────────────────────────────────────────────

// Rust strings: only `"`. `'` is excluded on purpose — a lifetime (`&'a str`)
// has no closing quote and would otherwise swallow the rest of the line. Char
// literals (`'a'`) just render plain; an acceptable miss for a lite pass.
const RUST: Lang = Lang {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    string_delims: &['"'],
    keywords: RUST_KEYWORDS,
};

const C_LIKE: Lang = Lang {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    string_delims: &['"', '\'', '`'],
    keywords: C_LIKE_KEYWORDS,
};

const PYTHON: Lang = Lang {
    line_comments: &["#"],
    block_comment: None,
    string_delims: &['"', '\''],
    keywords: PYTHON_KEYWORDS,
};

const SHELL: Lang = Lang {
    line_comments: &["#"],
    block_comment: None,
    string_delims: &['"', '\''],
    keywords: SHELL_KEYWORDS,
};

const JSON: Lang = Lang {
    line_comments: &[],
    block_comment: None,
    string_delims: &['"'],
    keywords: JSON_KEYWORDS,
};

/// Map a fenced code block's info string to a grammar, or `None` for an unknown
/// / empty language (caller then renders without highlighting).
///
/// Only the first token of the info string matters: ```` ```rust,no_run ````
/// and ```` ```rust ignore ```` both resolve to Rust.
fn lang_for(info: &str) -> Option<&'static Lang> {
    let tag = info
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())?
        .to_ascii_lowercase();
    Some(match tag.as_str() {
        "rust" | "rs" => &RUST,
        "c" | "h" | "cpp" | "c++" | "cc" | "hpp" | "cxx" | "java" | "js" | "javascript" | "jsx"
        | "ts" | "typescript" | "tsx" | "go" | "golang" | "kt" | "kotlin" | "swift" | "cs"
        | "c#" | "csharp" | "dart" | "scala" | "php" => &C_LIKE,
        "python" | "py" => &PYTHON,
        "bash" | "sh" | "shell" | "zsh" | "console" => &SHELL,
        "json" | "jsonc" => &JSON,
        _ => return None,
    })
}

/// Tokenize `source` for the language named by `info`. Returns one token list
/// per line (split on `\n`, in order), or `None` when the language is unknown —
/// in which case the caller renders the block plain.
///
/// Lines are tokenized in sequence so block-comment state can carry across the
/// `\n` boundary: an unterminated `/*` on one line keeps the next line in
/// comment state until its `*/`.
pub fn highlight(source: &str, info: &str) -> Option<Vec<Vec<Token>>> {
    let lang = lang_for(info)?;
    let mut in_block = false;
    let mut out = Vec::new();
    for line in source.split('\n') {
        let (tokens, still_in_block) = tokenize_line(line, lang, in_block);
        in_block = still_in_block;
        out.push(tokens);
    }
    Some(out)
}

/// Tokenize a single line. `in_block` is whether the line begins inside an
/// unterminated block comment; the returned bool is the same state for the next
/// line.
fn tokenize_line(line: &str, lang: &Lang, mut in_block: bool) -> (Vec<Token>, bool) {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;

    // Continuation of a block comment opened on an earlier line: consume to the
    // closer (or the whole line if it isn't found).
    if in_block {
        let close = lang
            .block_comment
            .expect("in_block implies a block_comment grammar")
            .1;
        let start = i;
        loop {
            if i >= n {
                push(&mut tokens, &chars[start..n], TokenClass::Comment);
                return (tokens, true);
            }
            if matches_at(&chars, i, close) {
                i += close.chars().count();
                push(&mut tokens, &chars[start..i], TokenClass::Comment);
                in_block = false;
                break;
            }
            i += 1;
        }
    }

    while i < n {
        let c = chars[i];

        // Line comment: rest of the line.
        if lang.line_comments.iter().any(|p| matches_at(&chars, i, p)) {
            push(&mut tokens, &chars[i..n], TokenClass::Comment);
            return (tokens, false);
        }

        // Block comment open: scan for the closer; if absent, the line ends
        // still inside the comment.
        if let Some((open, close)) = lang.block_comment {
            if matches_at(&chars, i, open) {
                let start = i;
                i += open.chars().count();
                loop {
                    if i >= n {
                        push(&mut tokens, &chars[start..n], TokenClass::Comment);
                        return (tokens, true);
                    }
                    if matches_at(&chars, i, close) {
                        i += close.chars().count();
                        break;
                    }
                    i += 1;
                }
                push(&mut tokens, &chars[start..i], TokenClass::Comment);
                continue;
            }
        }

        // String / char literal: from the opening quote to its match. A
        // backslash escapes the next char (so `"a\"b"` stays one string). An
        // unterminated quote runs to end of line — strings don't carry across
        // lines here.
        if lang.string_delims.contains(&c) {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let end = i.min(n);
            push(&mut tokens, &chars[start..end], TokenClass::Str);
            continue;
        }

        // Identifier or keyword: consumed whole so `foo123` never splits into an
        // identifier + a number.
        if is_ident_start(c) {
            let start = i;
            while i < n && is_ident_continue(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let class = if lang.keywords.contains(&word.as_str()) {
                TokenClass::Keyword
            } else {
                TokenClass::Plain
            };
            push(&mut tokens, &chars[start..i], class);
            continue;
        }

        // Numeric literal: a digit not part of an identifier. `is_number_continue`
        // is loose (hex/exponent/suffix/underscore) since exact numeric grammar
        // isn't worth it for a preview.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && is_number_continue(chars[i]) {
                i += 1;
            }
            push(&mut tokens, &chars[start..i], TokenClass::Number);
            continue;
        }

        // Punctuation / whitespace: plain. `push` merges it into the previous
        // plain run, so spans stay coarse.
        push(&mut tokens, &chars[i..i + 1], TokenClass::Plain);
        i += 1;
    }

    (tokens, in_block)
}

/// Append `slice` as a token, merging into the previous token when it shares the
/// same class so adjacent same-color runs collapse to one span.
fn push(tokens: &mut Vec<Token>, slice: &[char], class: TokenClass) {
    if slice.is_empty() {
        return;
    }
    if let Some(last) = tokens.last_mut() {
        if last.class == class {
            last.text.extend(slice.iter());
            return;
        }
    }
    tokens.push(Token {
        text: slice.iter().collect(),
        class,
    });
}

/// Whether `pat` (a short ASCII marker) appears in `chars` starting at `i`.
fn matches_at(chars: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(k, pc)| chars.get(i + k) == Some(&pc))
}

/// Identifier start: ASCII letter, `_`, or any non-ASCII (so identifiers in
/// non-Latin scripts stay whole rather than fragmenting into plain runs).
fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic() || !c.is_ascii()
}

fn is_ident_continue(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// Loose numeric continuation: digits, a decimal point, and letters/underscore
/// so `0xFF`, `1e10`, `1_000`, and `1u32` stay one token.
fn is_number_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokenize a single self-contained line (no carried block state).
    fn toks(line: &str, info: &str) -> Vec<Token> {
        let lines = highlight(line, info).expect("known language");
        assert_eq!(lines.len(), 1, "single line in, single line out");
        lines.into_iter().next().unwrap()
    }

    /// Collapse to `(text, class)` pairs for terse assertions.
    fn pairs(line: &str, info: &str) -> Vec<(String, TokenClass)> {
        toks(line, info)
            .into_iter()
            .map(|t| (t.text, t.class))
            .collect()
    }

    /// The first token of class `class`, if any.
    fn first_of(line: &str, info: &str, class: TokenClass) -> Option<String> {
        toks(line, info)
            .into_iter()
            .find(|t| t.class == class)
            .map(|t| t.text)
    }

    /// Concatenate every token's text back together — must always equal the
    /// input line (the tokenizer partitions, never drops or duplicates).
    fn reconstruct(line: &str, info: &str) -> String {
        toks(line, info).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn unknown_language_is_not_highlighted() {
        assert!(highlight("anything at all", "brainfuck").is_none());
        assert!(highlight("plain text", "").is_none());
    }

    #[test]
    fn info_string_first_token_selects_the_language() {
        // Attributes after the language name (CommonMark allows them) must not
        // defeat detection.
        assert!(highlight("let x = 1;", "rust,no_run").is_some());
        assert!(highlight("let x = 1;", "rust ignore").is_some());
        assert!(highlight("x = 1", "PYTHON").is_some(), "case-insensitive");
    }

    #[test]
    fn rust_keyword_string_and_comment() {
        // `let` is a keyword, the string keeps its quotes, and the trailing `//`
        // runs to end of line.
        let p = pairs(r#"let s = "hi"; // tail"#, "rust");
        assert!(p.contains(&("let".to_string(), TokenClass::Keyword)));
        assert!(p.contains(&(r#""hi""#.to_string(), TokenClass::Str)));
        assert_eq!(
            first_of(r#"let s = "hi"; // tail"#, "rust", TokenClass::Comment).as_deref(),
            Some("// tail")
        );
    }

    #[test]
    fn standalone_number_is_a_number_token() {
        assert_eq!(
            first_of("x = 42", "rust", TokenClass::Number).as_deref(),
            Some("42")
        );
        assert_eq!(
            first_of("addr = 0xFF_u8", "rust", TokenClass::Number).as_deref(),
            Some("0xFF_u8")
        );
    }

    #[test]
    fn identifier_with_digits_is_not_split() {
        // `foo123` is consumed as one identifier (Plain), so it never produces a
        // spurious Number token mid-word.
        let p = pairs("foo123 bar", "rust");
        assert!(
            p.iter().all(|(_, c)| *c != TokenClass::Number),
            "no number token should appear, got {p:?}"
        );
        assert_eq!(reconstruct("foo123 bar", "rust"), "foo123 bar");
    }

    #[test]
    fn escaped_quote_stays_inside_the_string() {
        // `"a\"b"` is a single string token, not two strings split at the
        // escaped quote.
        let s = first_of(r#"x = "a\"b" y"#, "rust", TokenClass::Str);
        assert_eq!(s.as_deref(), Some(r#""a\"b""#));
    }

    #[test]
    fn unterminated_string_runs_to_end_of_line() {
        let s = first_of(r#"x = "oops"#, "rust", TokenClass::Str);
        assert_eq!(s.as_deref(), Some(r#""oops"#));
    }

    #[test]
    fn rust_lifetime_is_not_a_string() {
        // `'` is not a Rust string delimiter here, so `&'a str` must not be
        // swallowed as a string running to end of line.
        let p = pairs("fn f<'a>(s: &'a str) {}", "rust");
        assert!(
            p.iter().all(|(_, c)| *c != TokenClass::Str),
            "a lifetime must not register as a string, got {p:?}"
        );
        assert!(
            p.iter()
                .any(|(t, c)| t == "fn" && *c == TokenClass::Keyword)
        );
        assert_eq!(
            reconstruct("fn f<'a>(s: &'a str) {}", "rust"),
            "fn f<'a>(s: &'a str) {}"
        );
    }

    #[test]
    fn block_comment_carries_across_lines() {
        // `/*` on line 1 keeps lines 2–3 in comment state until `*/` on line 3;
        // the `let` after the closer on line 3 is live code again.
        let src = "code /* open\nstill comment\nclose */ let x;";
        let lines = highlight(src, "rust").unwrap();
        assert_eq!(lines.len(), 3);

        // Line 1: trailing block-comment open.
        assert!(
            lines[0]
                .iter()
                .any(|t| t.class == TokenClass::Comment && t.text.contains("/* open"))
        );
        // Line 2: entirely comment.
        assert!(lines[1].iter().all(|t| t.class == TokenClass::Comment));
        assert_eq!(
            lines[1].iter().map(|t| t.text.as_str()).collect::<String>(),
            "still comment"
        );
        // Line 3: comment closes, then real code resumes.
        assert!(
            lines[2]
                .iter()
                .any(|t| t.class == TokenClass::Comment && t.text.contains("*/"))
        );
        assert!(
            lines[2]
                .iter()
                .any(|t| t.text == "let" && t.class == TokenClass::Keyword)
        );
    }

    #[test]
    fn inline_block_comment_does_not_leak_state() {
        // A `/* ... */` that closes on the same line leaves the next line live.
        let lines = highlight("a /* mid */ b\nlet y;", "rust").unwrap();
        assert!(
            lines[1]
                .iter()
                .any(|t| t.text == "let" && t.class == TokenClass::Keyword)
        );
    }

    #[test]
    fn python_uses_hash_comments_and_has_no_block_comments() {
        assert_eq!(
            first_of("x = 1  # note", "python", TokenClass::Comment).as_deref(),
            Some("# note")
        );
        // `/*` is not a comment in Python — it's just punctuation, so the `def`
        // after it still highlights.
        let p = pairs("a /* not */ def", "python");
        assert!(
            p.iter()
                .any(|(t, c)| t == "def" && *c == TokenClass::Keyword)
        );
        assert!(p.iter().all(|(_, c)| *c != TokenClass::Comment));
    }

    #[test]
    fn shell_single_quotes_are_strings() {
        let s = first_of("echo 'hello world'", "bash", TokenClass::Str);
        assert_eq!(s.as_deref(), Some("'hello world'"));
    }

    #[test]
    fn json_highlights_keywords_strings_and_numbers() {
        let p = pairs(r#"{"on": true, "n": 42}"#, "json");
        assert!(
            p.iter()
                .any(|(t, c)| t == r#""on""# && *c == TokenClass::Str)
        );
        assert!(
            p.iter()
                .any(|(t, c)| t == "true" && *c == TokenClass::Keyword)
        );
        assert!(p.iter().any(|(t, c)| t == "42" && *c == TokenClass::Number));
    }

    #[test]
    fn c_like_covers_common_aliases() {
        for info in ["js", "ts", "go", "java", "cpp", "c#"] {
            let p = pairs("return 0;", info);
            assert!(
                p.iter()
                    .any(|(t, c)| t == "return" && *c == TokenClass::Keyword),
                "`return` should be a keyword in {info}, got {p:?}"
            );
        }
    }

    #[test]
    fn adjacent_plain_runs_coalesce() {
        // Identifiers, punctuation, and whitespace of the same (Plain) class
        // merge into one span rather than one span per token/char — so an
        // all-plain line is a single span, and a keyword breaks the run.
        assert_eq!(
            pairs("a + b", "rust"),
            vec![("a + b".to_string(), TokenClass::Plain)]
        );

        let p = pairs("let a + b", "rust");
        assert_eq!(
            p,
            vec![
                ("let".to_string(), TokenClass::Keyword),
                (" a + b".to_string(), TokenClass::Plain),
            ],
            "the keyword splits the line; the rest is one coalesced plain run"
        );
    }

    #[test]
    fn non_ascii_identifier_stays_whole() {
        // A Japanese identifier must not fragment into per-char plain runs: the
        // whole `名前` rides inside a single plain span, and the line round-trips.
        let p = pairs("let 名前 = 1;", "rust");
        assert!(
            p.iter()
                .any(|(t, c)| t.contains("名前") && *c == TokenClass::Plain),
            "the non-ASCII identifier should sit in one plain span, got {p:?}"
        );
        assert_eq!(reconstruct("let 名前 = 1;", "rust"), "let 名前 = 1;");
    }

    #[test]
    fn empty_and_blank_lines_produce_no_tokens() {
        let lines = highlight("\n   \ncode", "rust").unwrap();
        assert!(lines[0].is_empty(), "empty line has no tokens");
        // A whitespace-only line collapses to a single plain run.
        assert!(lines[1].iter().all(|t| t.class == TokenClass::Plain));
    }
}
