use std::cell::OnceCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use shroud_core::{Point, Rect, Theme};
use shroud_platform::{PlatformWindow, SecureClipboard, SystemTheme};
use shroud_reactive::{Reactive, Signal};
use shroud_render::renderer::Renderer;
use shroud_security::hardening;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::shortcut::{Shortcut, ShortcutContext, ShortcutId};
use shroud_widgets::tree::WidgetTree;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::WindowId;

/// Default cadence for the periodic tick when an `on_frame` hook is set.
const DEFAULT_TICK_INTERVAL: Duration = Duration::from_millis(500);

type FrameHook = Box<dyn FnMut(&mut FrameContext) + 'static>;
type ShortcutHandler = Box<dyn FnMut(&mut ShortcutContext) + 'static>;

/// Context handed to the per-frame tick hook ([`AppScope::on_frame`]).
///
/// Mirrors [`shroud_widgets::shortcut::ShortcutContext`]: the tick reaches
/// the deferred tree-command queue through the public [`Self::event_ctx`]
/// field — so it can `replace_screen`, push a layer, etc., exactly like an
/// event handler — and additionally learns how long the UI has been idle
/// via [`Self::idle`].
///
/// Commands enqueued on `event_ctx` are drained into the tree immediately
/// after the hook returns (before the redraw the tick schedules), so a
/// screen swap requested from the tick is laid out and painted on that very
/// frame.
pub struct FrameContext<'a> {
    /// The same deferred command queue event handlers use. Call e.g.
    /// `ctx.event_ctx.replace_screen(...)` to drive a screen transition
    /// from the tick.
    pub event_ctx: &'a mut EventContext,
    idle: Duration,
}

impl FrameContext<'_> {
    /// Time since the last user input event — key press, mouse move,
    /// click, scroll, or IME activity. Resets to ~zero on any interaction
    /// and grows monotonically while the user is idle. Use it to drive
    /// inactivity timers such as an auto-lock.
    ///
    /// Note that raw mouse movement counts as activity (matching OS
    /// screensaver / idle conventions), so simply moving the pointer while
    /// reading keeps the session active.
    pub fn idle(&self) -> Duration {
        self.idle
    }
}

thread_local! {
    /// Process-wide OS-theme signal. Lazily created on the first
    /// `system_theme_signal()` call from this thread (the UI thread on
    /// every supported platform — winit's main-thread requirement
    /// makes that the only place either reads or writes happen). Held
    /// in a `thread_local!` rather than on `AppScope` so the signal is
    /// callable *before* `App::run`, which is the difference between
    /// "you can subscribe inside the build closure" and "you can fold
    /// the OS theme into a `Reactive<Theme>` passed to `App::theme(...)`".
    static SYSTEM_THEME_SIGNAL: OnceCell<Signal<Option<SystemTheme>>>
        = const { OnceCell::new() };
}

/// Reactive signal carrying the OS theme preference. Lazily created
/// on first access from the UI thread; subsequent calls (including
/// the event loop's `WindowEvent::ThemeChanged` write) return the
/// same handle.
///
/// Stable across `App::run` boundaries — call it from `main()` to
/// pre-build a `Reactive<Theme>` that mixes the OS preference with
/// in-app user settings before handing the result to `App::theme(...)`,
/// or call it inside the build closure (via
/// [`AppScope::system_theme`]) when an `AppScope` is already in hand.
///
/// The Signal payload starts as `None` and becomes `Some(Light | Dark)`
/// the moment the window reports a preference. On Linux outside
/// GNOME / KDE this may stay `None` for the life of the app — code
/// reading the signal should always treat `None` as "fall back to
/// app default".
pub fn system_theme_signal() -> Signal<Option<SystemTheme>> {
    SYSTEM_THEME_SIGNAL.with(|cell| *cell.get_or_init(|| Signal::new(None)))
}

/// Custom events sent to the event loop via [`AppHandle`].
///
/// Kept internal — users only interact with the thin [`AppHandle`] API.
/// Expand this enum if future phases need to carry data across the thread
/// boundary (e.g. batched signal updates produced off the UI thread).
#[derive(Debug, Clone, Copy)]
pub(crate) enum AppEvent {
    /// Request a redraw. Reactive values are re-read on paint, so this is
    /// how external producers (timer threads, async tasks, IPC) push UI
    /// updates without touching the widget tree directly.
    Wake,
}

/// Thread-safe handle to a running shroud app.
///
/// Clone it into any thread that needs to push the UI forward after
/// mutating a `Signal`. `wake()` is non-blocking and safe to call from
/// non-UI threads; it triggers a redraw on the event loop, which re-runs
/// every `Reactive` closure against the latest signal values.
///
/// Because `Signal` itself is `!Send`, producers living off the UI thread
/// should own their state separately (e.g. an `Arc<AtomicBool>`) and use
/// `wake()` only to signal that the UI should refresh. Code running on the
/// UI thread (event callbacks, the `App::run` build closure) may still
/// mutate signals directly.
#[derive(Clone, Debug)]
pub struct AppHandle {
    proxy: EventLoopProxy<AppEvent>,
}

impl AppHandle {
    /// Request a redraw from any thread. Errors (event loop already closed)
    /// are swallowed because by that point nothing can observe the wake.
    pub fn wake(&self) {
        let _ = self.proxy.send_event(AppEvent::Wake);
    }
}

/// UI-thread scope handed to the [`App::run`] build closure.
///
/// Exposes the thread-safe [`AppHandle`] and any registration points that
/// must run on the UI thread — currently [`Self::on_frame`] for per-frame
/// tick callbacks. The scope is consumed after the build closure returns;
/// anything registered on it is handed to the event loop for the lifetime
/// of the app.
pub struct AppScope {
    handle: AppHandle,
    frame_hook: Option<FrameHook>,
    /// Shortcut registrations queued during the build closure. Drained
    /// into the tree's [`ShortcutRouter`] right after the closure returns,
    /// preserving the ids handed back to the caller from
    /// [`Self::on_shortcut`].
    pending_shortcuts: Vec<(ShortcutId, Shortcut, ShortcutHandler)>,
    next_shortcut_id: u64,
}

impl AppScope {
    fn new(handle: AppHandle) -> Self {
        Self {
            handle,
            frame_hook: None,
            pending_shortcuts: Vec::new(),
            next_shortcut_id: 0,
        }
    }

    /// Borrow the thread-safe handle. Call `.clone()` on it to hand to
    /// background threads that need to [`AppHandle::wake`] the UI.
    pub fn handle(&self) -> &AppHandle {
        &self.handle
    }

    /// Register a callback that runs on a fixed cadence on the UI thread.
    ///
    /// Fires every [`App::tick_interval`] (default 500 ms), anchored to
    /// the previous fire so unrelated events (mouse moves, focus
    /// changes) don't drift the schedule. Each fire also schedules a
    /// redraw so the paint that follows sees any state the hook changed.
    ///
    /// Use this for driving timers, polling external state, or
    /// advancing animations without a background waker thread. The
    /// hook does not fire on input-triggered paints — keep it cheap
    /// and idempotent so the cadence stays predictable.
    ///
    /// The hook receives a [`FrameContext`], which exposes the deferred
    /// command queue (`ctx.event_ctx.replace_screen(...)` to swap screens
    /// from the tick) and [`FrameContext::idle`] (time since the last user
    /// input, for inactivity timers like an auto-lock). Any commands the
    /// hook enqueues are applied before the redraw it schedules.
    ///
    /// Only one hook is supported per app — a second call replaces the
    /// first.
    pub fn on_frame<F>(&mut self, f: F)
    where
        F: FnMut(&mut FrameContext) + 'static,
    {
        self.frame_hook = Some(Box::new(f));
    }

    /// Register an app-level keyboard shortcut.
    ///
    /// The handler runs on the UI thread when a matching `KeyDown`
    /// reaches the widget tree, *before* the Escape and Tab interceptors
    /// — so a registered binding wins over both layer dismiss and focus
    /// navigation. See [`shroud_widgets::shortcut`] for the scope rules
    /// (text-input suppression, layer opt-out).
    ///
    /// Returns a [`ShortcutId`] that can be passed to the tree's
    /// `shortcut_router_mut().remove(...)` to drop the binding later.
    /// Most apps register once in the build closure and never remove.
    ///
    /// ```ignore
    /// App::new().run(|scope| {
    ///     scope.on_shortcut(Shortcut::ctrl('l'), |ctx| {
    ///         ctx.event_ctx.replace_screen(|t| build_lock_screen(t));
    ///     });
    ///     WidgetTree::new()
    /// });
    /// ```
    pub fn on_shortcut<F>(&mut self, shortcut: Shortcut, handler: F) -> ShortcutId
    where
        F: FnMut(&mut ShortcutContext) + 'static,
    {
        let id = ShortcutId::from_raw(self.next_shortcut_id);
        self.next_shortcut_id += 1;
        self.pending_shortcuts
            .push((id, shortcut, Box::new(handler)));
        id
    }

    /// Convenience accessor for the OS-theme signal — thin wrapper
    /// over [`system_theme_signal`].
    ///
    /// Both return the same underlying `Signal<Option<SystemTheme>>`;
    /// the free-fn form exists for code that needs to read the signal
    /// *before* `App::run` is called (e.g. building a
    /// `Reactive<Theme>` to hand to `App::theme(...)`). Use whichever
    /// shape reads naturally at the call site.
    ///
    /// ```ignore
    /// App::new().run(|scope| {
    ///     let os_theme = scope.system_theme();
    ///     let label = TextWidget::reactive(move || match os_theme.get() {
    ///         Some(SystemTheme::Dark) => "OS prefers dark",
    ///         Some(SystemTheme::Light) => "OS prefers light",
    ///         None => "OS preference unknown",
    ///     }.to_string());
    ///     /* … */
    /// });
    /// ```
    pub fn system_theme(&self) -> Signal<Option<SystemTheme>> {
        system_theme_signal()
    }

    /// One-shot snapshot of the OS locale, as a BCP-47 tag (e.g.
    /// `"ja-JP"`, `"en-US"`). Convenience re-export of
    /// [`shroud_platform::system_locale()`] so apps can read it through
    /// the same `AppScope` they already hold inside `App::run`.
    ///
    /// Returns `None` if the OS could not be queried. Locale changes
    /// during the process lifetime aren't surfaced as events on any
    /// supported platform — re-call to refresh.
    pub fn system_locale(&self) -> Option<String> {
        shroud_platform::system_locale()
    }
}

/// Internal bag of window/runtime configuration. Users never see this —
/// they set fields through the fluent [`App`] builder.
struct AppConfig {
    title: String,
    width: u32,
    height: u32,
    disable_core_dumps: bool,
    ptrace_protection: bool,
    exploit_mitigation: bool,
    capture_prevention: bool,
    /// Theme source. `Reactive::Static(t)` for the historical
    /// `App::theme(Theme::dark())` shape; `Reactive::Dynamic(...)`
    /// when the theme is a `Signal<Theme>` or a derived closure (for
    /// live theme swap). Pulled on every paint, so swapping in a new
    /// `Theme` value through the signal repaints with fresh tokens.
    theme: Reactive<Theme>,
    tick_interval: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "shroud".to_string(),
            width: 800,
            height: 600,
            disable_core_dumps: true,
            ptrace_protection: true,
            // Off by default because `ProcessExtensionPointDisablePolicy` on
            // Windows blocks the extension-DLL plumbing the Microsoft IME for
            // Japanese (and other CJK IMEs) loads through — flipping it on at
            // App start would silently break Japanese / Chinese / Korean
            // typing in every shroud app. Security-strict deployments opt in
            // explicitly via `App::exploit_mitigation(true)` after deciding
            // they don't need IME (numeric kiosks, English-only flows).
            exploit_mitigation: false,
            capture_prevention: false,
            theme: Reactive::Static(Theme::default()),
            tick_interval: DEFAULT_TICK_INTERVAL,
        }
    }
}

/// Fluent builder for a shroud application.
///
/// Configure the window, theme, and hardening flags with the setter
/// methods, then call [`run`](Self::run) with a closure that builds the
/// widget tree. The closure receives an [`AppScope`] exposing the
/// thread-safe [`AppHandle`] (for waking the UI from other threads) and
/// [`AppScope::on_frame`] (for registering per-frame callbacks).
///
/// ```no_run
/// use shroud_app::App;
/// use shroud_widgets::tree::WidgetTree;
///
/// App::new()
///     .title("hello")
///     .size(800, 600)
///     .run(|_scope| WidgetTree::new());
/// ```
pub struct App {
    config: AppConfig,
}

impl App {
    /// Start a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: AppConfig::default(),
        }
    }

    /// Window title. Defaults to `"shroud"`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    /// Window dimensions in logical pixels. Defaults to 800×600.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self
    }

    /// Visual theme applied to the paint context.
    ///
    /// Accepts anything convertible into a [`Reactive<Theme>`] —
    /// `Theme::dark()` for a static theme (the historical shape),
    /// `Signal<Theme>` for a user-toggleable theme that flips on
    /// `signal.set(...)`, or `Reactive::derive(|| ...)` for a theme
    /// derived from multiple inputs (Knot-style `light | dark |
    /// system` resolution that folds in [`system_theme_signal`]).
    ///
    /// The current value is pulled on every paint frame, so updating
    /// the underlying source plus calling [`AppHandle::wake`] (or
    /// causing any other redraw — input, `WindowEvent::ThemeChanged`,
    /// `on_frame` tick) is enough to repaint with the new tokens. No
    /// widget-level rewiring needed; the swap touches paint context
    /// only.
    ///
    /// Defaults to [`Theme::default`] (currently the dark theme).
    pub fn theme(mut self, theme: impl Into<Reactive<Theme>>) -> Self {
        self.config.theme = theme.into();
        self
    }

    /// Enable screen capture prevention.
    ///
    /// When enabled, the window content appears black in screenshots
    /// and screen recordings. Uses `SetWindowDisplayAffinity` on Windows;
    /// a no-op on platforms without capture prevention support.
    /// Defaults to `false`.
    pub fn capture_prevention(mut self, on: bool) -> Self {
        self.config.capture_prevention = on;
        self
    }

    /// Control the always-on core dump hardening.
    ///
    /// Leaves secrets out of crash dumps by calling the OS hardening
    /// hook on startup. Defaults to `true`; flip to `false` only for
    /// debugging.
    pub fn disable_core_dumps(mut self, on: bool) -> Self {
        self.config.disable_core_dumps = on;
        self
    }

    /// Block debugger attach at startup.
    ///
    /// - Linux: `prctl(PR_SET_DUMPABLE, 0)` rejects ptrace from non-root.
    /// - macOS: `ptrace(PT_DENY_ATTACH)` rejects subsequent `PT_ATTACH`.
    /// - Windows: no-op (use [`Self::exploit_mitigation`] for process
    ///   hardening).
    ///
    /// Defaults to `true`; flip to `false` to attach a debugger during
    /// development.
    pub fn ptrace_protection(mut self, on: bool) -> Self {
        self.config.ptrace_protection = on;
        self
    }

    /// Apply OS-level exploit mitigations at startup.
    ///
    /// - Windows: `SetProcessMitigationPolicy` with
    ///   `ProcessExtensionPointDisablePolicy` — blocks legacy AppInit DLLs,
    ///   global IME hooks, and similar DLL-injection vectors.
    /// - Linux / macOS: no-op today; reserved for future seccomp / sandbox
    ///   hooks.
    ///
    /// **Defaults to `false`** because the Windows policy also blocks the
    /// extension-DLL plumbing that the Microsoft IME for Japanese (and
    /// other CJK IMEs) needs in order to deliver composition events to the
    /// window — turning it on would silently break Japanese / Chinese /
    /// Korean typing in every shroud app. Opt in explicitly for flows that
    /// don't accept text input (numeric kiosks, English-only utilities)
    /// where the added defence-in-depth against legacy DLL injection is
    /// worth losing IME support. A more granular replacement
    /// (`ProcessImageLoadPolicy` restricted to System32 / signed images,
    /// say) is a future-phase candidate.
    pub fn exploit_mitigation(mut self, on: bool) -> Self {
        self.config.exploit_mitigation = on;
        self
    }

    /// Cadence for the idle tick when an [`AppScope::on_frame`] hook is
    /// registered. Defaults to 500 ms, which is suitable for coarse
    /// timers (countdown UI, clipboard auto-clear). Lower for smoother
    /// animation, higher to save CPU when idle.
    ///
    /// Has no effect when no `on_frame` hook is set — the event loop
    /// still waits for input as before.
    pub fn tick_interval(mut self, interval: Duration) -> Self {
        self.config.tick_interval = interval;
        self
    }

    /// Take over the main thread and run the application.
    ///
    /// The `build` closure is called once, before the event loop starts,
    /// with an [`AppScope`] giving access to the thread-safe
    /// [`AppHandle`] and UI-thread registration points such as
    /// [`AppScope::on_frame`]. Return the fully-constructed
    /// [`WidgetTree`] from the closure.
    pub fn run<F>(self, build: F)
    where
        F: FnOnce(&mut AppScope) -> WidgetTree,
    {
        if self.config.disable_core_dumps {
            if let Err(e) = hardening::disable_core_dumps() {
                log::warn!("Failed to disable core dumps: {}", e);
            }
        }
        if self.config.ptrace_protection {
            if let Err(e) = hardening::enable_ptrace_protection() {
                log::warn!("Failed to enable ptrace protection: {}", e);
            }
        }
        if self.config.exploit_mitigation {
            if let Err(e) = hardening::enable_exploit_mitigation() {
                log::warn!("Failed to enable exploit mitigation: {}", e);
            }
        }

        let event_loop = EventLoop::<AppEvent>::with_user_event()
            .build()
            .expect("failed to create event loop");
        let handle = AppHandle {
            proxy: event_loop.create_proxy(),
        };

        // Build the tree eagerly so the closure can hand `handle` to
        // threads during construction. The window/renderer come online
        // in `resumed`.
        let mut scope = AppScope::new(handle.clone());
        let mut tree = build(&mut scope);
        let AppScope {
            frame_hook,
            pending_shortcuts,
            ..
        } = scope;

        // Replay shortcut registrations queued in the build closure into
        // the tree's router, preserving the ids returned from
        // `AppScope::on_shortcut`.
        {
            let router = tree.shortcut_router_mut();
            for (id, shortcut, handler) in pending_shortcuts {
                router.register_with_id(id, shortcut, handler);
            }
        }

        let mut handler = ShroudEventLoop {
            config: self.config,
            handle,
            window: None,
            renderer: None,
            tree: Some(tree),
            paint_ctx: None,
            event_ctx: EventContext::new(),
            cursor_position: Point::new(0.0, 0.0),
            frame_hook,
            next_tick: None,
            last_input: Instant::now(),
            last_ime_cursor_area: None,
            last_ime_allowed: None,
        };

        event_loop.run_app(&mut handler).expect("event loop error");
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a winit window event counts as user activity for the idle
/// clock behind [`FrameContext::idle`].
///
/// Keyboard, pointer (move/click/scroll), and IME events are activity.
/// OS- or window-manager-driven events (theme change, resize, focus,
/// redraw, close) are not — counting them would keep an unattended
/// session alive and defeat an auto-lock. Mouse *movement* is included
/// deliberately, matching screensaver / OS idle conventions so that
/// reading a long note (pointer drifting, scrolling) doesn't lock.
fn is_user_input(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::KeyboardInput { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::Ime(_)
            | WindowEvent::DroppedFile(_)
    )
}

/// Translate a winit named key into the corresponding shroud event.
///
/// Most named keys become a `KeyDown { key }`, but Space is a deliberate
/// exception — it is a printable character semantically (Input/SecureInput
/// insert ' ' into their buffer; Button/Checkbox use it for activation),
/// so we route it through `CharInput` instead. Without this case, winit
/// drops Space on the floor (it doesn't arrive as `Character`), which
/// silently broke space typing in inputs from day one and meant the
/// 19a-3 Button/Checkbox space activation only ever worked in tests.
///
/// Returns `None` for unmapped named keys (modifiers, function keys,
/// etc.) — those simply don't reach widgets today.
/// Translate a winit `Character` payload into the widget events the tree
/// expects.
///
/// Plain typing (no modifier, or just Shift for capital letters) maps each
/// char to a `CharInput` — `Input` and `SecureInput` consume those literally.
/// When *any non-shift* modifier is held (Ctrl, Alt, logo/super) the chars
/// instead become `KeyDown { Key::Character(ch) }` so the app-level shortcut
/// router (`ShortcutRouter`) can match e.g. Ctrl+L. The chars don't reach
/// `Input::event`'s `CharInput` arm because of that promotion, which is the
/// behavior change apps need to be aware of when upgrading.
///
/// Held in this isolated form so it can be unit-tested without spinning up
/// a winit instance (see [`feedback_test_translation_layer`] convention).
/// True for the Ctrl+V / Cmd+V key combos that conventionally mean paste.
///
/// `s` is winit's `Character` payload — when the OS reports Ctrl+V it
/// arrives as `"v"` (lowercase) plus the Ctrl modifier flag. Shift+Ctrl+V
/// is *not* paste (it is a different binding in many apps); we only
/// match the pure-Ctrl / pure-Logo cases.
///
/// Held in this isolated form so it can be unit-tested without a real
/// keyboard, mirroring [`translate_character`].
fn is_paste_combo(s: &str, mods: Modifiers) -> bool {
    if mods.shift || mods.alt {
        return false;
    }
    // Either Ctrl-only (Windows/Linux) or Logo/Cmd-only (macOS).
    let ctrl_only = mods.ctrl && !mods.logo;
    let logo_only = mods.logo && !mods.ctrl;
    if !(ctrl_only || logo_only) {
        return false;
    }
    matches!(s, "v" | "V")
}

fn translate_character(s: &str, mods: Modifiers) -> Vec<WidgetEvent> {
    if mods.has_non_shift() {
        s.chars()
            .map(|ch| WidgetEvent::KeyDown {
                key: Key::Character(ch),
            })
            .collect()
    } else {
        s.chars().map(|ch| WidgetEvent::CharInput { ch }).collect()
    }
}

fn translate_named_key(named: &WinitNamedKey) -> Option<WidgetEvent> {
    match named {
        WinitNamedKey::Space => Some(WidgetEvent::CharInput { ch: ' ' }),
        WinitNamedKey::Enter => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Enter),
        }),
        WinitNamedKey::Escape => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Escape),
        }),
        WinitNamedKey::Tab => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Tab),
        }),
        WinitNamedKey::Backspace => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Backspace),
        }),
        WinitNamedKey::Delete => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Delete),
        }),
        WinitNamedKey::ArrowLeft => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowLeft),
        }),
        WinitNamedKey::ArrowRight => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowRight),
        }),
        WinitNamedKey::ArrowUp => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowUp),
        }),
        WinitNamedKey::ArrowDown => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::ArrowDown),
        }),
        WinitNamedKey::Home => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::Home),
        }),
        WinitNamedKey::End => Some(WidgetEvent::KeyDown {
            key: Key::Named(NamedKey::End),
        }),
        _ => None,
    }
}

struct ShroudEventLoop {
    config: AppConfig,
    #[allow(dead_code)] // retained so user clones stay valid; future phases may read it
    handle: AppHandle,
    window: Option<PlatformWindow>,
    renderer: Option<Renderer>,
    tree: Option<WidgetTree>,
    paint_ctx: Option<PaintContext>,
    event_ctx: EventContext,
    cursor_position: Point,
    frame_hook: Option<FrameHook>,
    /// When the next frame-hook tick is due. Anchored to the prior tick
    /// plus `tick_interval`, not "now"; this keeps the cadence steady
    /// even when unrelated events (mouse moves, focus changes) cause
    /// extra `about_to_wait` calls.
    next_tick: Option<Instant>,
    /// Timestamp of the most recent user input event (key, mouse
    /// move/click, scroll, IME). Read by the frame-hook tick to compute
    /// [`FrameContext::idle`], the basis for inactivity timers like an
    /// auto-lock. Seeded to construction time so the idle clock starts at
    /// launch; updated at the top of `window_event` for input-class events
    /// only (OS-driven events like theme/resize don't count as activity).
    last_input: Instant,
    /// Most recent IME cursor area pushed to the platform window.
    /// Used to dedupe redundant `set_ime_cursor_area` calls when the
    /// same caret rect gets repainted across many frames (mouse moves,
    /// idle repaints) without the caret actually moving. `None` means
    /// nothing has been pushed yet this session.
    last_ime_cursor_area: Option<Rect>,
    /// Most recent `set_ime_allowed` value pushed to the platform
    /// window. Tracks the Tier 2 IME-bypass state so we only call
    /// `set_ime_allowed` when the desired value actually flips —
    /// otherwise every paint with a focused `SecureInput` would push
    /// the same `false` to the OS, and every paint without one would
    /// push the same `true`. `None` means nothing has been pushed yet;
    /// the first paint always pushes whatever the current focus dictates.
    last_ime_allowed: Option<bool>,
}

impl ShroudEventLoop {
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Read clipboard text and replay it as a burst of `CharInput` events
    /// against the current focus. Used by the Ctrl+V interceptor above.
    /// A read failure (clipboard unavailable, non-text content) is silently
    /// ignored — paste is a best-effort UX, not a critical path.
    fn dispatch_paste(&mut self) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let clipboard = SecureClipboard::new();
        let Ok(text) = clipboard.read() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        for ch in text.chars() {
            tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut self.event_ctx);
        }
        self.request_redraw();
    }
}

impl ApplicationHandler<AppEvent> for ShroudEventLoop {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        // Fire the frame hook whenever the scheduled tick is due,
        // regardless of what actually woke the loop. Anchoring to
        // `next_tick` (rather than "time since last wakeup") keeps the
        // cadence steady when unrelated events interleave.
        if self.frame_hook.is_none() {
            return;
        }

        let now = Instant::now();
        let due = self.next_tick.is_none_or(|t| now >= t);
        if !due {
            return;
        }

        // Take the hook out so the closure can borrow other `self` fields
        // (`event_ctx`, `tree`) through the `FrameContext` while it runs,
        // then put it back. Only one hook exists, so this can't race.
        if let Some(mut hook) = self.frame_hook.take() {
            let idle = now.saturating_duration_since(self.last_input);
            let mut ctx = FrameContext {
                event_ctx: &mut self.event_ctx,
                idle,
            };
            hook(&mut ctx);
            self.frame_hook = Some(hook);

            // Apply any deferred commands the tick enqueued (e.g. a
            // `replace_screen` for an auto-lock) before the redraw below,
            // mirroring how `dispatch_event` / `flush_pending_focus` drain.
            if let Some(tree) = self.tree.as_mut() {
                tree.apply_pending_commands(&mut self.event_ctx);
            }
        }
        self.next_tick = Some(now + self.config.tick_interval);
        // Nudge a paint so the UI reflects any state the hook touched.
        self.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // While a frame hook is registered, park the loop until the
        // next scheduled tick. `next_tick` is anchored to prior fires,
        // so this does not drift on every event.
        if let Some(deadline) = self.next_tick {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut platform_window = PlatformWindow::new(
            event_loop,
            &self.config.title,
            self.config.width,
            self.config.height,
        );

        if self.config.capture_prevention {
            let result = platform_window.set_capture_prevention(true);
            if !result.is_applied() {
                log::warn!("Screen capture prevention not available: {:?}", result);
            }
        }

        // Enable IME for composed-script input (Japanese, Chinese, Korean,
        // etc.) Without this winit drops composition events on the floor on
        // Windows / macOS / X11 and CJK users can't type at all — the
        // canonical "no Japanese input" symptom every framework eventually
        // discovers. Apps that need to disable IME per-flow can call
        // `PlatformWindow::set_ime_allowed(false)` directly.
        platform_window.set_ime_allowed(true);

        // Publish the initial OS theme snapshot. winit also fires
        // `ThemeChanged` shortly after window creation on most
        // platforms, but reading once here means subscribers see a
        // non-`None` value on the very first paint when the platform
        // already knows the preference. Writing unconditionally is
        // safe because `system_theme_signal()` lazily creates the
        // signal — a `None` from `platform_window.system_theme()`
        // leaves the existing `None` in place.
        if let Some(theme) = platform_window.system_theme() {
            system_theme_signal().set(Some(theme));
        }

        let window_arc = platform_window.arc();
        let renderer = pollster::block_on(Renderer::new(Arc::clone(&window_arc)));

        let paint_ctx = PaintContext::new(self.config.theme.get());

        self.window = Some(platform_window);
        self.renderer = Some(renderer);
        self.paint_ctx = Some(paint_ctx);

        window_arc.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Wake => self.request_redraw(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Stamp activity for the idle clock before the consuming match.
        // Only genuine user input counts — OS-driven events (theme change,
        // resize, redraw) must not keep an otherwise-idle session alive,
        // or an auto-lock would never fire.
        if is_user_input(&event) {
            self.last_input = Instant::now();
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            // Live OS theme change — publish unconditionally; the
            // thread-local signal is lazily created so writers don't
            // need to gate on subscriber presence. Redraw nudges
            // reactive closures (including any `Reactive<Theme>`
            // installed via `App::theme(...)`) to pull on next frame.
            WindowEvent::ThemeChanged(theme) => {
                system_theme_signal().set(Some(SystemTheme::from_winit(theme)));
                self.request_redraw();
            }

            // Keep EventContext's modifier snapshot current so widgets
            // (and the tree's Tab routing) see the right state on the
            // next key event. winit fires this eagerly whenever any
            // modifier state flips.
            WindowEvent::ModifiersChanged(mods) => {
                let state = mods.state();
                self.event_ctx.modifiers = Modifiers {
                    shift: state.shift_key(),
                    ctrl: state.control_key(),
                    alt: state.alt_key(),
                    logo: state.super_key(),
                };
            }

            // Window gained focus — re-apply IME open status. Windows
            // resets IME open on focus transitions in some configurations,
            // so calling `set_ime_allowed(true)` again here re-runs both
            // the winit IACE_DEFAULT path and our `ImmSetOpenStatus(true)`
            // override, keeping IME live across focus loss/regain.
            //
            // We also record the value we just pushed in `last_ime_allowed`
            // so the Tier 2 dedup in `RedrawRequested` sees the platform's
            // current state — otherwise a focused SecureInput's
            // `target_allowed = false` would match a stale `Some(false)`
            // from before the focus loss and skip the re-disable push,
            // leaving the OS IME alive while a password is being typed.
            WindowEvent::Focused(true) => {
                if let Some(window) = &self.window {
                    window.set_ime_allowed(true);
                }
                self.last_ime_allowed = Some(true);
            }

            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                self.request_redraw();
            }

            // ── Mouse events ─────────────────────────────────────
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Point::new(position.x as f32, position.y as f32);
                if let Some(tree) = &mut self.tree {
                    tree.dispatch_event(
                        &WidgetEvent::MouseMove {
                            position: self.cursor_position,
                        },
                        &mut self.event_ctx,
                    );
                    self.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let btn = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };

                let widget_event = match state {
                    ElementState::Pressed => WidgetEvent::MouseDown {
                        position: self.cursor_position,
                        button: btn,
                    },
                    ElementState::Released => WidgetEvent::MouseUp {
                        position: self.cursor_position,
                        button: btn,
                    },
                };

                if let Some(tree) = &mut self.tree {
                    tree.dispatch_event(&widget_event, &mut self.event_ctx);
                    self.request_redraw();
                }
            }

            // ── Scroll events ────────────────────────────────────
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                };

                if let Some(tree) = &mut self.tree {
                    tree.dispatch_event(
                        &WidgetEvent::Scroll {
                            position: self.cursor_position,
                            delta_x: dx,
                            delta_y: dy,
                        },
                        &mut self.event_ctx,
                    );
                    self.request_redraw();
                }
            }

            // ── Keyboard events ──────────────────────────────────
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                match &event.logical_key {
                    // Character input → CharInput event (or KeyDown when a
                    // non-shift modifier is held, so the shortcut router
                    // sees Ctrl+letter combos before they reach Input).
                    WinitKey::Character(s) => {
                        // Ctrl+V (or Cmd+V on macOS) intercepts paste before
                        // it ever becomes a KeyDown: the clipboard text is
                        // injected as a sequence of CharInput events, which
                        // every focused Input / SecureInput already handles.
                        // Apps that need a custom Ctrl+V handler can opt out
                        // by disabling default paste at app build time (not
                        // yet wired — file an issue if you need it).
                        if is_paste_combo(s, self.event_ctx.modifiers) {
                            self.dispatch_paste();
                            return;
                        }
                        let events = translate_character(s, self.event_ctx.modifiers);
                        if !events.is_empty() {
                            if let Some(tree) = &mut self.tree {
                                for ev in events {
                                    tree.dispatch_event(&ev, &mut self.event_ctx);
                                }
                            }
                            self.request_redraw();
                        }
                    }

                    // Named keys → either a KeyDown event (most named
                    // keys) or, for Space, a CharInput so the rest of the
                    // pipeline treats it as a printable character.
                    WinitKey::Named(named) => {
                        if let Some(event) = translate_named_key(named) {
                            if let Some(tree) = &mut self.tree {
                                tree.dispatch_event(&event, &mut self.event_ctx);
                                self.request_redraw();
                            }
                        }
                    }

                    _ => {}
                }
            }

            // ── IME events ───────────────────────────────────────
            //
            // Composed text from an IME (Japanese, Chinese, Korean, …)
            // arrives as `Ime::Commit(text)` after the user finishes
            // composing. We splat each char into the existing
            // `CharInput` path so Input / SecureInput don't need a new
            // event variant. Preedit / Enabled / Disabled are ignored
            // for M1 — Preedit display is a polish item (the OS still
            // shows its native composition window in the meantime).
            WindowEvent::Ime(Ime::Commit(text)) => {
                if let Some(tree) = &mut self.tree {
                    for ch in text.chars() {
                        tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut self.event_ctx);
                    }
                    self.request_redraw();
                }
            }
            WindowEvent::Ime(_) => {
                // Preedit / Enabled / Disabled are ignored for M1. The OS
                // shows its native composition window during typing, and
                // the final Commit lands via the arm above.
            }

            // ── File drop (drag-and-drop from the OS) ─────────────
            //
            // winit delivers one `DroppedFile` per file with no drop
            // coordinates (and stops emitting `CursorMoved` during the
            // drag), so this routes to the tree's window-level file-drop
            // handler rather than hit-testing a position. Apps register a
            // handler via `WidgetTree::on_file_drop`; with none registered
            // this is a no-op.
            WindowEvent::DroppedFile(path) => {
                if let Some(tree) = &mut self.tree {
                    tree.dispatch_file_drop(&path, &mut self.event_ctx);
                    self.request_redraw();
                }
            }

            // ── Render ───────────────────────────────────────────
            WindowEvent::RedrawRequested => {
                if self.renderer.is_none() || self.tree.is_none() || self.paint_ctx.is_none() {
                    return;
                }

                let size = self.renderer.as_ref().unwrap().surface_size();
                let tree = self.tree.as_mut().unwrap();
                let paint_ctx = self.paint_ctx.as_mut().unwrap();

                // Pull the latest theme value from the reactive source
                // before laying out / painting. For `App::theme(Theme)`
                // this is a cheap clone of a static value; for signal-
                // or closure-driven sources it picks up any update
                // pushed since the last frame, making
                // `Signal<Theme>::set(...)` (or a derived `Reactive`
                // that depends on `system_theme_signal()`) visible on
                // the very next paint without per-widget rewiring.
                paint_ctx.theme = self.config.theme.get();

                // Apply any deferred initial focus before layout so widget
                // state set by FocusGained (cursor visibility, focus ring)
                // is reflected in this very first paint of the new tree.
                // Cheap when nothing is pending; covers both the boot path
                // and screen transitions whose build closure called
                // `tree.focus_initially(...)`.
                tree.flush_pending_focus(&mut self.event_ctx);

                // Layout pass — widgets report intrinsic size via their
                // `measure()` so `.center()` / gap / grow work without a
                // fixed-width wrapper around leaves.
                tree.compute_layout_with_measure(
                    size.0 as f32,
                    size.1 as f32,
                    &mut paint_ctx.text_engine,
                    &paint_ctx.theme,
                );

                paint_ctx.clear();
                tree.paint(paint_ctx);

                let current_ime_area = paint_ctx.ime_cursor_area();

                let renderer = self.renderer.as_mut().unwrap();
                match renderer.render(
                    paint_ctx.theme.colors.background,
                    &paint_ctx.rects,
                    &paint_ctx.glyphs,
                    &paint_ctx.secure_glyphs,
                    &paint_ctx.images,
                    paint_ctx.layer_starts(),
                ) {
                    Ok(()) => {}
                    Err(e) => {
                        log::error!("Render error: {:?}", e);
                    }
                }

                // Forward the focused text widget's caret rect to the OS
                // so the IME candidate / composition window anchors near
                // the cursor instead of defaulting to a screen corner.
                // Dedupe redundant pushes — most frames repaint without
                // the caret moving (mouse hover, theme reads, etc.).
                if current_ime_area != self.last_ime_cursor_area {
                    if let (Some(rect), Some(window)) = (current_ime_area, &self.window) {
                        window.set_ime_cursor_area(
                            rect.origin.x,
                            rect.origin.y,
                            rect.size.width,
                            rect.size.height,
                        );
                    }
                    self.last_ime_cursor_area = current_ime_area;
                }

                // Tier 2 IME bypass: drive `set_ime_allowed` from the
                // paint result so a focused SecureInput disconnects the
                // OS IME, and any other focus (or no focus) leaves it
                // alive. Deduped against the last value we pushed so
                // most frames are a no-op.
                let target_allowed = !paint_ctx.ime_suppressed();
                if self.last_ime_allowed != Some(target_allowed) {
                    if let Some(window) = &self.window {
                        window.set_ime_allowed(target_allowed);
                    }
                    self.last_ime_allowed = Some(target_allowed);
                }
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_named_key_routes_to_char_input() {
        // Regression for the silent Space drop: winit reports Space as
        // a named key, not a Character, so Input/SecureInput/Button/
        // Checkbox would never see it pre-fix. translate_named_key must
        // turn it into a CharInput { ch: ' ' } so the rest of the
        // pipeline treats it as a printable character.
        let event = translate_named_key(&WinitNamedKey::Space);
        assert!(
            matches!(event, Some(WidgetEvent::CharInput { ch: ' ' })),
            "Space must translate to CharInput {{ ch: ' ' }}, got {event:?}"
        );
    }

    #[test]
    fn dropped_file_counts_as_user_input() {
        // A drag-drop is deliberate user activity, so it must reset the
        // idle clock behind `FrameContext::idle` — otherwise an auto-lock
        // could fire while the user is dropping a file. OS-driven events
        // (focus, resize, theme) deliberately stay non-input so an
        // unattended session still locks.
        assert!(is_user_input(&WindowEvent::DroppedFile(
            std::path::PathBuf::from("image.png")
        )));
        assert!(!is_user_input(&WindowEvent::Focused(true)));
    }

    #[test]
    fn enter_named_key_routes_to_key_down() {
        // Companion test: every other named key still goes through
        // KeyDown — only Space is special-cased.
        let event = translate_named_key(&WinitNamedKey::Enter);
        assert!(matches!(
            event,
            Some(WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter)
            })
        ));
    }

    #[test]
    fn unmapped_named_key_returns_none() {
        // Modifier and function keys are intentionally unmapped — they
        // should pass straight through without spurious events.
        assert!(translate_named_key(&WinitNamedKey::Shift).is_none());
        assert!(translate_named_key(&WinitNamedKey::F1).is_none());
    }

    #[test]
    fn plain_letter_translates_to_char_input() {
        // Regression: typing 'l' with no modifier must still hit Input
        // as a literal character — A-11 must not break basic typing.
        let events = translate_character("l", Modifiers::default());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], WidgetEvent::CharInput { ch: 'l' }));
    }

    #[test]
    fn ctrl_letter_promotes_to_key_down() {
        // Core of A-11: Ctrl+L becomes a KeyDown so the shortcut router
        // can match it. Input::event's KeyDown arm doesn't handle
        // Character keys, so the literal 'l' never reaches the buffer.
        let events = translate_character("l", Modifiers::CTRL);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            WidgetEvent::KeyDown {
                key: Key::Character('l')
            }
        ));
    }

    #[test]
    fn shift_alone_stays_as_char_input() {
        // Shift+letter is just a capital letter — must keep flowing as
        // CharInput so typing capitalized text still works.
        let events = translate_character(
            "L",
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], WidgetEvent::CharInput { ch: 'L' }));
    }

    #[test]
    fn system_theme_signal_is_readable_and_writable() {
        // The reactive bridge that publishes `WindowEvent::ThemeChanged`
        // assumes `Signal<Option<SystemTheme>>` round-trips through the
        // runtime — exercise that contract here so a regression to the
        // SystemTheme derives (losing `Copy`, say) trips a unit test
        // rather than only surfacing inside a live window.
        let sig: Signal<Option<SystemTheme>> = Signal::new(None);
        assert_eq!(sig.get(), None);
        sig.set(Some(SystemTheme::Light));
        assert_eq!(sig.get(), Some(SystemTheme::Light));
        sig.set(Some(SystemTheme::Dark));
        assert_eq!(sig.get(), Some(SystemTheme::Dark));
    }

    #[test]
    fn ctrl_v_is_paste_combo() {
        // Pure Ctrl+V on Windows/Linux is the canonical paste binding —
        // event_loop intercepts before translate_character so the KeyDown
        // never actually fires.
        assert!(is_paste_combo("v", Modifiers::CTRL));
        assert!(is_paste_combo("V", Modifiers::CTRL));
    }

    #[test]
    fn cmd_v_is_paste_combo() {
        // macOS uses Cmd (= logo). Same intercept rule applies.
        assert!(is_paste_combo("v", Modifiers::LOGO));
        assert!(is_paste_combo("V", Modifiers::LOGO));
    }

    #[test]
    fn shift_modified_ctrl_v_is_not_paste() {
        // Shift+Ctrl+V is a distinct binding in many apps ("paste without
        // formatting", etc.) — make sure we leave it alone so users /
        // apps can wire it up via the shortcut router.
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_combo("v", mods));
    }

    #[test]
    fn ctrl_other_letter_is_not_paste() {
        // Only V / v counts — guards against typo regressions in the
        // matcher (e.g., accidentally pasting on Ctrl+B).
        assert!(!is_paste_combo("b", Modifiers::CTRL));
        assert!(!is_paste_combo("c", Modifiers::CTRL));
    }

    #[test]
    fn plain_v_is_not_paste() {
        // Sanity: regular typing of 'v' must not be hijacked.
        assert!(!is_paste_combo("v", Modifiers::default()));
        assert!(!is_paste_combo(
            "V",
            Modifiers {
                shift: true,
                ..Modifiers::default()
            }
        ));
    }

    #[test]
    fn multi_char_with_ctrl_emits_one_keydown_per_char() {
        // Dead-key compose or IME might in theory deliver a multi-char
        // string with Ctrl held. Each char becomes its own KeyDown so
        // the router has a chance to match every codepoint.
        let events = translate_character("ab", Modifiers::CTRL);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            WidgetEvent::KeyDown {
                key: Key::Character('a')
            }
        ));
        assert!(matches!(
            events[1],
            WidgetEvent::KeyDown {
                key: Key::Character('b')
            }
        ));
    }
}
