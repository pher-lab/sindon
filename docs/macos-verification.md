# macOS verification — the script for borrowed hardware

sindon has never run on a Mac that a person was looking at. CI compiles it,
tests it, and starts it on a paravirtual GPU with nobody watching; that is a
floor, and the README says so. This file is the plan for the session that
replaces the floor with an observation.

It assumes the hardware is **borrowed and time-boxed**. Everything below is
ordered so that stopping early still leaves the most valuable answers
collected, and so that no minute is spent re-proving something CI already
knows. Read [§Already settled](#already-settled) before starting: several
obvious-looking checks are already answered, and repeating them is the easiest
way to lose the session.

Completing §Tier 1 is the trigger for publishing 0.1.4. The `[Unreleased]`
section of `CHANGELOG.md` is already holding work that waits on it.

Write the answers into
[macos-verification-results.md](macos-verification-results.md) — the same
checks as a form, short enough to keep open beside the app, plus the commands
to paste, the permissions to ask the owner for, and what to clean up before
handing borrowed hardware back.

## Already settled — do not spend Mac time here

| Question | Answer | Where |
|---|---|---|
| Do the ten published crates compile for Darwin? | yes, warning-free | CI `macOS (compile + test)` |
| Does the test suite pass there? | yes, 97 suites / 1250 tests | same |
| Do `PT_DENY_ATTACH` and `setrlimit(RLIMIT_CORE,0)` run on real Darwin? | yes | same |
| Do `knot` and `vault` build, vendored OpenSSL and all? | yes, ~2 min | CI `macOS (example apps)` |
| Does a sindon binary survive being started? | yes, `hello_world` and `knot`, clean stderr | same |
| Is OS theme detection implemented on macOS? | yes — winit reads `NSApplication.effectiveAppearance` and KVOs it | winit source |

Anything in that table failing on the Mac is itself a finding, but do not go
looking for it. Start at Tier 1.

## Setup

Order matters: get a window on screen before starting the slow build.

```sh
xcode-select --install          # linker + system headers, ~5 min
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/pher-lab/sindon.git && cd sindon

cargo run -p hello_world        # fast: no SQLCipher, no OpenSSL
cargo build -p knot             # slow: builds OpenSSL from source, ~2 min + deps
```

`hello_world` appearing is already the first Tier 1 answer. Start the `knot`
build in a second terminal while looking at it.

Everything below uses `knot` unless stated. That is deliberate: `knot` is the
only thing in the repo that exercises IME, the clipboard, file dialogs and
capture prevention together, and it is the only one that calls
`.capture_prevention(true)`.

## Tier 1 — only a Mac can answer these, and they gate the release

### 1. Does a sindon frame look like a UI?

Nobody has seen one on this platform. Open `hello_world`, then `knot`.

**Pass**: text is legible, widgets are where the layout says, colours are not
inverted or washed out, nothing flickers.

⚠ Judge this with your eyes, not a screenshot — screenshots on HDR displays
misreport colour, which is why this project treats them as layout evidence
only.

### 2. Capture prevention — the one this was implemented for

`knot` calls `.capture_prevention(true)`, so its window should be excluded from
capture. Try **more than one capture path**; they are not equivalent:

| path | how | predicted |
|---|---|---|
| system screenshot | `Cmd+Shift+3`, `Cmd+Shift+4` | window black / excluded |
| CLI | `screencapture -x /tmp/shot.png` then open it | window black / excluded |
| QuickTime screen recording | QuickTime → File → New Screen Recording | **window visible** |

The third row is the interesting one. winit documents QuickTime as able to read
a window with `NSWindowSharingNone`, and `docs/SECURITY.md` repeats that as a
limit. **Either outcome is a result**: QuickTime seeing the content confirms
the documented hole; QuickTime showing black means our docs are more pessimistic
than reality and should be corrected downward.

Also try a capture path we have not thought of — Zoom/Teams screen share, or a
third-party recorder — since "which paths does it actually stop" is the whole
question and CI could never ask it.

**Pass** for the release is rows 1–2 excluded. Record row 3 either way.

Control for the run: `cargo run -p counter` (or any example) does *not* enable
capture prevention. If that window is also black in a screenshot, something is
wrong with the test, not with sindon.

### 3. Japanese IME

The largest unknown. Every IME behaviour this project knows was learned on
Windows, and the fixes there were platform-specific enough that none of it
transfers by argument.

Enable Japanese input (System Settings → Keyboard → Input Sources), focus
`knot`'s note editor, and type Japanese:

- does a preedit (未確定) string appear inline, underlined?
- does the conversion candidate window appear **near the caret**, not at the
  window corner? (this is `ime_cursor_area` plumbing)
- does 文節 navigation with arrow keys move the highlighted clause?
- does the committed text land where the caret was?
- does `SecureInput` (the master-password field on `knot`'s lock screen)
  *refuse* IME? That is intentional: secrets must not pass through an IME
  composition buffer.

**Pass**: preedit appears and commits correctly, candidates follow the caret.
Anything less is a finding worth the whole trip.

### 4. Retina

DPI scaling was implemented and verified on Windows only. macOS Retina is a
clean 2× — the easy case — so the risk is not 2× itself but a scaled
resolution producing a non-integer factor.

- text crisp, not soft or doubled (compare against the Windows screenshots in
  `docs/dogfood-log.md` if unsure)
- layout at the right physical size, not half or double
- System Settings → Displays → a *scaled* resolution → does the window follow
  live? (non-integer factor path)

### 5. VoiceOver and a11y bounds

AccessKit was verified with a Windows screen reader only. macOS is the second
platform, and bounds are the first thing to break because the coordinate
systems differ.

`Cmd+F5` toggles VoiceOver. Tab through `knot`.

- does VoiceOver announce the focused widget with the right role and label?
- does the VoiceOver cursor's box land **on** the widget rather than offset or
  scaled by 2?
- ⚠ do secrets stay unspoken? `SecureInput` must not read its contents aloud.
  This is the inbound mirror of the non-exposure rule and is the one item here
  that is a security check, not a polish check.

## Tier 2 — do these if time remains

### 6. Font fallback

`knot` asks for `Yu Gothic UI`, which ships with Windows and **does not exist
on macOS** (`examples/knot/src/main.rs`). Text will fall back.

**Predicted**: the ragged Latin+CJK two-typeface look that `Yu Gothic UI` was
chosen to fix on Windows comes back. If it does, the comment at that call site
already names the intended answer — bundle a family such as Noto Sans JP via
`.font(..)` — and this observation is what turns that from a guess into a task.

### 7. Shortcuts use Ctrl, not Cmd

**Predicted, from source rather than guessed**: `Shortcut::ctrl` builds
`Modifiers::CTRL`, and the router matches modifier sets with `!=` — exact
equality. So on macOS every `knot` binding wants **Ctrl**, and **Cmd does
nothing**. `Cmd+F` will not open find; `Ctrl+F` will.

Confirm it, then judge the framework question it raises: should
`Shortcut::ctrl` mean "the platform's primary accelerator" (Cmd on macOS, Ctrl
elsewhere) rather than literally Ctrl? That is a public-API shape decision, so
it is cheapest while the crates are still 0.1.x.

### 8. Clipboard

Copy and paste text in `knot`; copy an entry in `vault`. Check the
auto-clearing secure path actually clears — copy a secret, wait out the
timeout, paste into TextEdit, confirm nothing arrives.

### 9. File dialogs

`knot`'s import/export and image attachment paths open native dialogs through
`rfd`. Confirm they open, return a path, and that cancelling returns cleanly.

### 10. Trackpad scrolling

**Predicted to work**: the event loop handles `PixelDelta` and divides by the
scale factor, so precision deltas are already logical units. What is unverified
is *feel*: direction under natural scrolling, and whether momentum produces
sensible velocity in `ScrollView` and `VirtualList`.

### 11. OS theme

System Settings → Appearance → Light/Dark. `knot` follows the OS theme through
`system_theme_signal()`. Implemented for macOS in winit; never watched.

**Pass**: the app cross-fades to the new theme without a restart.

## Recording what happens

Fill in [macos-verification-results.md](macos-verification-results.md) as you
go, on the machine, while looking at the thing being judged. Afterwards the
findings go where the equivalent Linux and Windows findings went:

- a bug or a degraded behaviour → `CHANGELOG.md` `[Unreleased]`, and the
  README's Platform support section if it changes what a user should expect
- capture prevention specifically → `docs/SECURITY.md`, which is the file this
  project treats as authoritative when the summaries drift
- anything that was already predicted here → say so, so the next reader learns
  which predictions held

Then publish 0.1.4. The macOS rows in the README stop saying "never observed"
only for what was actually observed — the honest floor moves up by exactly the
checks that were run, and no further.
