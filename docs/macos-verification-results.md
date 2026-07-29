# macOS verification — results

Fill this in *on the Mac*, while looking at the thing being judged. The
reasoning, the pass criteria in full, and the reasons some checks are ordered
the way they are live in [macos-verification.md](macos-verification.md); this
file is the form, so it stays short enough to keep open beside the app.

Rules for filling it in:

- **Record what was seen, not what it means.** The write-up happens afterwards.
- A prediction that broke is worth more than one that held — say which.
- An item not reached stays unchecked. Blank is honest; guessing is not.

## Environment

```sh
sw_vers                                        # macOS version
sysctl -n machdep.cpu.brand_string             # chip
system_profiler SPDisplaysDataType | grep -E 'Resolution|Retina|Display Type'
```

- macOS version: `…`
- Chip: `…`
- Display / scaling: `…`
- Date, and how long the machine is available: `…`

⚠ CI only ever built for **arm64**. If this machine is Intel, nothing in
§Already settled applies to it and the first build is itself a finding.

## Before touching the app — ask the owner

- [ ] Disk space for Xcode CLI tools + rustup + target dir (several GB)
- [ ] **Screen Recording permission**, System Settings → Privacy & Security →
      Screen Recording, for **Terminal** and **QuickTime**. `Cmd+Shift+3` does
      not need it; `screencapture` from a terminal and QuickTime both do, and
      granting it needs an admin password. Tier 1 #2 stalls here otherwise.
- [ ] Permission to add a Japanese input source, toggle VoiceOver, switch
      Light/Dark, and change display resolution

## Setup

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/pher-lab/sindon.git && cd sindon

cargo run -p hello_world     # first pixels; no OpenSSL on this path
cargo build -p knot          # second terminal, in parallel: ~2 min + deps
```

- [ ] `hello_world` window appeared — wall-clock from `git clone` to pixels: `…`
- [ ] `cargo build -p knot` succeeded — time: `…`, warnings: `…`

## Tier 1 — gates the 0.1.4 release

### 1. Does a sindon frame look like a UI?

Legible text, widgets where the layout says, colours neither inverted nor
washed out, no flicker. ⚠ Judge with your eyes; screenshots misreport colour on
HDR displays.

- [ ] `hello_world` — verdict: `…`
- [ ] `knot` — verdict: `…`
- Anything off (what, where): `…`

### 2. Capture prevention

`knot` enables it; `counter` does not and is the control.

| path | result |
|---|---|
| `Cmd+Shift+3` / `Cmd+Shift+4` on `knot` | `…` |
| `screencapture -x /tmp/shot.png` then open it | `…` |
| QuickTime → New Screen Recording (**predicted: window visible**) | `…` |
| other path tried (Zoom/Teams share, third-party recorder): `…` | `…` |
| **control**: same paths against `counter` | `…` |

- [ ] Rows 1–2 excluded the window → release criterion met
- [ ] Control window *was* captured (if it was black too, the test is broken,
      not sindon)
- QuickTime outcome, either way, is a result — which docs need changing:
  `…`

### 3. Japanese IME

System Settings → Keyboard → Input Sources → Japanese. Focus `knot`'s note
editor.

- [ ] Preedit (未確定) appears inline and underlined
- [ ] Candidate window appears **near the caret**, not at the window corner
- [ ] 文節 navigation with arrow keys moves the highlighted clause
- [ ] Committed text lands where the caret was
- [ ] `SecureInput` (lock screen master password) **refuses** IME — intentional
- IME used (Japanese IM / Google 日本語入力 / other): `…`
- Deviations from Windows behaviour: `…`

### 4. Retina

- [ ] Text crisp, not soft or doubled
- [ ] Layout at the right physical size, not half or double
- [ ] System Settings → Displays → a *scaled* resolution → window follows live
      (this is the non-integer factor path — the actual risk)
- Scale factors observed: `…`

### 5. VoiceOver and a11y bounds

`Cmd+F5` toggles VoiceOver. Tab through `knot`.

- [ ] Focused widget announced with the right role and label
- [ ] VoiceOver cursor box lands **on** the widget — not offset, not scaled by 2
- [ ] ⚠ `SecureInput` contents stay **unspoken** (security check, not polish)
- Widgets that announced wrongly: `…`

## Tier 2

### 6. Font fallback

`knot` asks for `Yu Gothic UI`, which does not exist here. **Predicted**: the
ragged Latin+CJK two-typeface look returns.

- [ ] Checked — held / broke: `…`
- What it actually falls back to, and whether it looks acceptable: `…`

### 7. Shortcuts use Ctrl, not Cmd

**Predicted from source**: `Cmd+F` does nothing, `Ctrl+F` opens find.

- [ ] `Cmd+F`: `…`  /  `Ctrl+F`: `…`
- Other bindings spot-checked: `…`
- Judgement on the API question — should `Shortcut::ctrl` mean "the platform's
  primary accelerator"? (cheapest to change while 0.1.x): `…`

### 8. Clipboard

- [ ] Copy/paste text in `knot`
- [ ] Copy an entry in `vault`; wait out the auto-clear timeout, paste into
      TextEdit, confirm **nothing arrives**
- Notes: `…`

### 9. File dialogs

- [ ] Import / export opens a native dialog and returns a path
- [ ] Image attachment likewise
- [ ] Cancelling returns cleanly (no hang, no panic)

### 10. Trackpad scrolling

**Predicted to work**; unverified part is *feel*.

- [ ] Direction correct under natural scrolling
- [ ] Momentum velocity sensible in `ScrollView` and in `VirtualList`
- Notes: `…`

### 11. OS theme

System Settings → Appearance → Light/Dark.

- [ ] `knot` cross-fades to the new theme without a restart
- Notes: `…`

## Prediction ledger

The point of writing predictions down beforehand was to learn which kind of
reasoning holds. One line each.

| prediction | held? |
|---|---|
| QuickTime can read a capture-protected window | `…` |
| Cmd does nothing; Ctrl is the accelerator | `…` |
| `Yu Gothic UI` falls back and the CJK/Latin mix returns | `…` |
| Trackpad already works (PixelDelta ÷ scale factor) | `…` |
| Everything in §Already settled still holds on real hardware | `…` |

Surprises that no prediction covered: `…`

## Before giving the machine back

- [ ] `rustup self uninstall`
- [ ] Delete the clone (and `target/` inside it)
- [ ] Delete `~/Library/Application Support/knot` — the encrypted vault and
      settings `knot` created, including whatever master password was typed
- [ ] Delete `~/Library/Application Support/vault` if `vault` was run
- [ ] Revert: input sources, VoiceOver, Appearance, display resolution, Screen
      Recording permissions
- [ ] Collect the screenshots taken, then remove them from the machine

## Afterwards

Carry this file back and write up from it:

- bugs and degraded behaviour → `CHANGELOG.md` `[Unreleased]`, and the README's
  Platform support section where it changes what a user should expect
- capture prevention → `docs/SECURITY.md`, the authoritative file when
  summaries drift
- then publish **0.1.4**, moving the honest floor up by exactly the checks that
  were actually run
