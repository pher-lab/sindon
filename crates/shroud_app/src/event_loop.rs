use std::cell::{Cell, OnceCell, RefCell};
use std::sync::Arc;
use std::time::{Duration, Instant};

use accesskit_winit::{Adapter, Event as A11yEvent, WindowEvent as A11yWindowEvent};

use crate::a11y::{action_from_request, snapshot_to_tree_update};
use shroud_core::{Color, Colors, Point, Rect, Theme};
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
use winit::keyboard::{Key as WinitKey, KeyCode, NamedKey as WinitNamedKey, PhysicalKey};
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

thread_local! {
    /// Snapshot of the theme the running app resolved on the last paint.
    /// The event loop publishes into it every frame, right after pulling
    /// [`App::theme`]'s reactive source (see [`publish_active_theme`]), so
    /// during paint it always holds the theme the widgets are being drawn
    /// against. Read only through [`theme_value`] / [`theme_color`], never
    /// directly — those wrap it in a pull-based [`Reactive`] that matches
    /// how the rest of the framework consumes theme tokens.
    static ACTIVE_THEME: RefCell<Theme> = RefCell::new(Theme::default());
}

/// Publish the theme resolved for this frame so [`theme_color`] /
/// [`theme_value`] accessors read fresh tokens when the tree paints.
/// Called by the event loop at each paint, before the tree is painted.
fn publish_active_theme(theme: &Theme) {
    ACTIVE_THEME.with(|cell| *cell.borrow_mut() = theme.clone());
}

/// Bind a value derived from the active [`Theme`] into a [`Reactive`] that
/// tracks live theme swaps — without the app hand-rolling a
/// `Reactive::derive(|| my_theme().‹field›)` wrapper per token.
///
/// The framework already re-pulls every `Reactive` on each paint (that is
/// how [`App::theme`] repaints with new tokens), and it publishes the
/// frame's resolved theme just before painting. So a closure handed here
/// reads the same theme the widgets are drawn against, and any
/// [`App::theme`] swap becomes visible on the next repaint with no
/// per-widget rewiring — the same contract as `App::theme` itself.
///
/// Use this for tokens outside the color palette (e.g. `|t| t.hover.bg`,
/// `|t| t.spacing.md`); reach for [`theme_color`] for the common
/// palette-color case.
///
/// ```no_run
/// use shroud_app::theme_value;
/// // Track the hover background across a light/dark toggle:
/// let hover_bg = theme_value(|t| t.hover.bg);
/// ```
///
/// Reads the theme published on the most recent paint. Before the first
/// paint (or when called off the UI thread, which the pull model never
/// does) it reflects [`Theme::default`].
pub fn theme_value<T, F>(f: F) -> Reactive<T>
where
    T: Clone + 'static,
    F: Fn(&Theme) -> T + 'static,
{
    Reactive::derive(move || ACTIVE_THEME.with(|cell| f(&cell.borrow())))
}

/// Bind a color from the active [`Theme`]'s palette into a [`Reactive`]
/// that tracks live theme swaps. The ergonomic common case of
/// [`theme_value`], scoped to [`Colors`] so the closure reads
/// `|c| c.primary` instead of `|t| t.colors.primary`.
///
/// This collapses the boilerplate every app otherwise reinvents — a
/// `Reactive::derive(|| my_theme().colors.‹name›)` wrapper for each token
/// a panel needs. Hand the result straight to any widget builder that
/// takes `impl Into<Reactive<Color>>`:
///
/// ```no_run
/// use shroud_app::theme_color;
/// use shroud_widgets::Container;
/// let panel = Container::column().background(theme_color(|c| c.surface));
/// ```
pub fn theme_color<F>(f: F) -> Reactive<Color>
where
    F: Fn(&Colors) -> Color + 'static,
{
    theme_value(move |t| f(&t.colors))
}

/// Custom events sent to the event loop via [`AppHandle`].
///
/// Kept internal — users only interact with the thin [`AppHandle`] API.
/// Expand this enum if future phases need to carry data across the thread
/// boundary (e.g. batched signal updates produced off the UI thread).
#[derive(Debug)]
pub(crate) enum AppEvent {
    /// Request a redraw. Reactive values are re-read on paint, so this is
    /// how external producers (timer threads, async tasks, IPC) push UI
    /// updates without touching the widget tree directly.
    Wake,
    /// An event from the accessibility adapter (an AT connected and wants the
    /// initial tree, requested an action, or disconnected). Delivered through
    /// the same event-loop proxy the adapter is handed in `resumed`.
    Accessibility(A11yEvent),
}

impl From<A11yEvent> for AppEvent {
    fn from(event: A11yEvent) -> Self {
        AppEvent::Accessibility(event)
    }
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
    image_load_hardening: bool,
    capture_prevention: bool,
    /// Theme source. `Reactive::Static(t)` for the historical
    /// `App::theme(Theme::dark())` shape; `Reactive::Dynamic(...)`
    /// when the theme is a `Signal<Theme>` or a derived closure (for
    /// live theme swap). Pulled on every paint, so swapping in a new
    /// `Theme` value through the signal repaints with fresh tokens.
    theme: Reactive<Theme>,
    tick_interval: Duration,
    /// Fonts (TTF / OTF bytes) registered into the text engine once the
    /// window comes up, before the first paint. The canonical use is bundling
    /// an icon font via `App::font(include_bytes!(..))`; see
    /// [`TextEngine::load_font_data`](shroud_text::TextEngine::load_font_data).
    /// `'static` bytes (an `include_bytes!` slice) cost nothing to carry.
    fonts: Vec<std::borrow::Cow<'static, [u8]>>,
    /// Family that generic / unstyled text (`TextFamily::SansSerif`, the
    /// default of every widget) resolves to. `None` keeps cosmic-text's built-in
    /// generic (which mixes a Latin substitute with a separate CJK fallback);
    /// `Some(name)` pins one cohesive family via
    /// [`TextEngine::set_default_font_family`](shroud_text::TextEngine::set_default_font_family),
    /// applied once after `fonts` are registered and before the first paint.
    default_font_family: Option<String>,
    /// Whether to expose the UI to OS assistive technology (screen readers)
    /// via an `accesskit` adapter. On by default; the adapter activates
    /// lazily (only when an AT actually connects), so the steady-state cost
    /// of leaving it on is a single branch per frame.
    accessibility: bool,
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
            // On by default: unlike the extension-point policy above, the
            // `ProcessImageLoadPolicy` hardening only constrains where DLLs
            // load from (no remote / no low-IL / prefer System32) and leaves
            // the IME path untouched, so it is safe to apply in every app.
            image_load_hardening: true,
            capture_prevention: false,
            theme: Reactive::Static(Theme::default()),
            tick_interval: DEFAULT_TICK_INTERVAL,
            fonts: Vec::new(),
            default_font_family: None,
            accessibility: true,
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

    /// Register a font (TTF / OTF bytes) into the text engine at startup.
    ///
    /// The bytes are registered once the window comes up, before the first
    /// paint, so any widget can reference the font's families by name from the
    /// very first frame. Call it as many times as you have fonts.
    ///
    /// The canonical use is an **icon font**: bundle a monochrome icon `.ttf`
    /// with `include_bytes!`, then draw an icon as a single-glyph
    /// `TextWidget::new("\u{e801}").family(TextFamily::Named("My Icons"))` — the
    /// glyph rides the same shaping / atlas / tint path as text, so it scales
    /// and recolors like any label. The font's family name(s) are what
    /// [`TextEngine::load_font_data`](shroud_text::TextEngine::load_font_data)
    /// reports; check the font (or that return value) for the exact string.
    ///
    /// ```no_run
    /// use shroud_app::App;
    /// use shroud_widgets::tree::WidgetTree;
    ///
    /// // In a real app the bytes come from the binary:
    /// //   let icons = include_bytes!("../assets/icons.ttf");
    /// # let icons: &'static [u8] = &[];
    /// App::new()
    ///     .font(icons)
    ///     .run(|_scope| WidgetTree::new());
    /// ```
    pub fn font(mut self, data: impl Into<std::borrow::Cow<'static, [u8]>>) -> Self {
        self.config.fonts.push(data.into());
        self
    }

    /// Pin the font family that generic / unstyled text resolves to.
    ///
    /// Every widget's text defaults to `TextFamily::SansSerif`. Left to
    /// cosmic-text's built-in generic, that maps Latin runs to one substitute
    /// font and CJK runs to a *different* per-script fallback, so mixed
    /// Japanese-and-Latin UI renders in two clashing typefaces. Naming one
    /// family here that carries every script the app shows — a cohesive UI font
    /// like `"Yu Gothic UI"`, or a family you bundled with [`font`](Self::font)
    /// such as `"Noto Sans JP"` — makes unstyled text shape in that single face
    /// throughout. Explicit `.family(..)` on a widget (code monospace, the icon
    /// font) still wins; only the default is affected.
    ///
    /// Applied once after bundled [`font`](Self::font)s are registered and
    /// before the first paint. A name no installed / bundled font provides is
    /// ignored (text keeps the previous behavior), so it never fails hard.
    ///
    /// ```no_run
    /// use shroud_app::App;
    /// use shroud_widgets::tree::WidgetTree;
    ///
    /// App::new()
    ///     .default_font_family("Yu Gothic UI")
    ///     .run(|_scope| WidgetTree::new());
    /// ```
    pub fn default_font_family(mut self, name: impl Into<String>) -> Self {
        self.config.default_font_family = Some(name.into());
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
    /// worth losing IME support. For the IME-safe subset of DLL-injection
    /// hardening that *is* on by default, see
    /// [`Self::image_load_hardening`].
    pub fn exploit_mitigation(mut self, on: bool) -> Self {
        self.config.exploit_mitigation = on;
        self
    }

    /// Control the always-on, IME-safe DLL image-load hardening.
    ///
    /// - Windows: `SetProcessMitigationPolicy` with `ProcessImageLoadPolicy`
    ///   — rejects DLLs loaded from remote shares (`NoRemoteImages`) or with
    ///   a low integrity label (`NoLowMandatoryLabelImages`), and searches
    ///   System32 ahead of the application directory (`PreferSystem32Images`,
    ///   blunting DLL search-order hijacking). Unlike
    ///   [`Self::exploit_mitigation`] it does **not** touch the
    ///   extension-point / IME path, so it stays on even for apps that take
    ///   CJK text input.
    /// - Linux / macOS: no-op.
    ///
    /// Defaults to `true`. Flip to `false` only if the app deliberately
    /// loads DLLs from a layout the restricted loader search order would
    /// reject.
    pub fn image_load_hardening(mut self, on: bool) -> Self {
        self.config.image_load_hardening = on;
        self
    }

    /// Expose the UI to OS assistive technology (screen readers) via an
    /// `accesskit` adapter. **Defaults to `true`.**
    ///
    /// The adapter activates *lazily*: it does nothing until an assistive
    /// technology actually connects, at which point the framework starts
    /// publishing an accessibility tree built from the widget hierarchy
    /// (roles, names, states, focus). While no AT is connected the per-frame
    /// cost is a single `update_if_active` branch — so leaving accessibility on
    /// is effectively free for users who don't use a screen reader, which is
    /// why it is the default.
    ///
    /// Secret-bearing widgets (`SecureInput`, `SecureText`) are exposed as
    /// masked / protected nodes whose plaintext is never placed in the tree, so
    /// turning this on does not weaken the secret-aware guarantees.
    ///
    /// Flip to `false` for deployments that deliberately keep the process off
    /// the OS accessibility surface entirely — a kiosk, or a high-security
    /// context where even the (secret-free) UI structure should not be visible
    /// to UI Automation / AT-SPI clients.
    pub fn accessibility(mut self, on: bool) -> Self {
        self.config.accessibility = on;
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
        if self.config.image_load_hardening {
            if let Err(e) = hardening::enable_image_load_hardening() {
                log::warn!("Failed to enable image-load hardening: {}", e);
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
            adapter: None,
            tree: Some(tree),
            paint_ctx: None,
            event_ctx: EventContext::new(),
            cursor_position: Point::new(0.0, 0.0),
            frame_hook,
            next_tick: None,
            anim_wake_at: None,
            last_input: Instant::now(),
            last_ime_cursor_area: None,
            last_ime_allowed: None,
            fonts_loaded: false,
            redraw_pending_since: Cell::new(None),
            redraw_retry_count: Cell::new(0),
            perf_log: std::env::var("SHROUD_PERF")
                .ok()
                .and_then(|path| std::fs::File::create(path).ok())
                .map(std::io::BufWriter::new),
            perf_input: None,
            perf_start: Instant::now(),
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

/// Translate a winit `Ime` event into the widget events to dispatch, in order.
///
/// Pure (no `self`) so the platform→widget translation can be unit-tested
/// without a winit event loop. Mirrors [`translate_character`] /
/// [`translate_named_key`].
///
/// - `Preedit` → a single [`WidgetEvent::ImePreedit`] carrying the
///   composition text + caret range. The focused text widget renders it
///   inline; an empty preedit clears it.
/// - `Commit` → clear any active preedit first, then splat the committed
///   chars through the existing [`WidgetEvent::CharInput`] path (so widgets
///   need no commit-specific handling).
/// - `Enabled` / `Disabled` → clear any lingering preedit. Harmless no-op
///   when nothing is composing; on `Disabled` (focus loss / IME bypass) it
///   guarantees a half-composed string doesn't stick on screen.
fn translate_ime(ime: &Ime) -> Vec<WidgetEvent> {
    match ime {
        Ime::Preedit(text, cursor) => vec![WidgetEvent::ImePreedit {
            text: text.clone(),
            cursor: *cursor,
        }],
        Ime::Commit(text) => {
            let mut out = vec![WidgetEvent::ImePreedit {
                text: String::new(),
                cursor: None,
            }];
            out.extend(text.chars().map(|ch| WidgetEvent::CharInput { ch }));
            out
        }
        Ime::Enabled | Ime::Disabled => vec![WidgetEvent::ImePreedit {
            text: String::new(),
            cursor: None,
        }],
    }
}

struct ShroudEventLoop {
    config: AppConfig,
    handle: AppHandle,
    window: Option<PlatformWindow>,
    renderer: Option<Renderer>,
    /// OS accessibility adapter, created in `resumed` when
    /// [`AppConfig::accessibility`] is on. `None` when accessibility is
    /// disabled or the window isn't up yet. Fed every winit `WindowEvent`;
    /// emits [`AppEvent::Accessibility`] through the event-loop proxy when an
    /// AT connects / acts / disconnects, and is pushed a fresh tree each frame
    /// via `update_if_active` (a no-op while no AT is listening).
    adapter: Option<Adapter>,
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
    /// Earliest instant a widget asked to be repainted at via
    /// [`shroud_reactive::animation::request_frame_at`], captured after the
    /// last paint, or `None` when nothing wants a timed wake. This is the
    /// blinking caret's toggle: `about_to_wait` parks until this deadline and
    /// then requests one redraw, instead of pumping at frame rate the way an
    /// in-flight `Animated` (which votes via `frame_requested`) does. Cleared
    /// once the deadline fires; the repaint it triggers re-votes if the caret
    /// is still blinking.
    anim_wake_at: Option<Instant>,
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
    /// Whether `config.fonts` have been registered into the text engine yet.
    /// `resumed` can fire more than once (suspend/resume); without this guard
    /// each resume would re-register the same faces and bloat the font db.
    fonts_loaded: bool,
    /// Frame perf log, enabled by setting `SHROUD_PERF=<file path>` in the
    /// environment: one line per painted frame with phase timings (layout /
    /// paint / gpu), the frame's full-shape count/time from the text engine,
    /// and — when the frame follows an input event — the input→present
    /// latency. `None` (the normal case) costs nothing but the check.
    perf_log: Option<std::io::BufWriter<std::fs::File>>,
    /// Arrival time + short description of the oldest input event not yet
    /// answered by a painted frame, for the perf log's `input=` column.
    /// First-wins until a frame drains it, so a burst of IME events reports
    /// the worst latency of the batch. Only written when `perf_log` is on.
    perf_input: Option<(Instant, String)>,
    /// Session start, for the perf log's timestamp column.
    perf_start: Instant,
    /// When the oldest still-unserved `request_redraw` was issued, or `None`
    /// when every request has been answered by a `RedrawRequested`. Windows
    /// drops the WM_PAINT behind `request_redraw` while the IME opens its
    /// composition window — the first preedit of every composition session
    /// painted 40–360ms late, rescued only by the next keystroke or the
    /// frame-hook tick — so `about_to_wait` re-requests any redraw pending
    /// longer than [`REDRAW_WATCHDOG`]. `Cell` so `request_redraw` can stay
    /// `&self`.
    redraw_pending_since: Cell<Option<Instant>>,
    /// Consecutive watchdog re-requests that still produced no frame. Capped
    /// at [`REDRAW_WATCHDOG_RETRIES`] so a window that legitimately cannot
    /// paint (minimized, occluded) doesn't keep the loop waking at 50Hz —
    /// the next input or tick re-arms the watchdog naturally.
    redraw_retry_count: Cell<u8>,
}

/// How long a requested redraw may go unserved before the watchdog in
/// `about_to_wait` asks again. Longer than any healthy request→frame gap
/// (sub-millisecond normally; one vsync when animating), far shorter than the
/// IME-swallowed stalls it exists to fix. A false fire is a coalesced,
/// harmless duplicate WM_PAINT request.
const REDRAW_WATCHDOG: Duration = Duration::from_millis(20);

/// Give up re-requesting after this many unanswered watchdog fires until
/// something else (input, tick) requests a redraw again.
const REDRAW_WATCHDOG_RETRIES: u8 = 5;

impl ShroudEventLoop {
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
            // Arm the redraw watchdog (see `about_to_wait`). First-wins so
            // the deadline tracks the *oldest* unserved request.
            if self.redraw_pending_since.get().is_none() {
                self.redraw_pending_since.set(Some(Instant::now()));
                self.redraw_retry_count.set(0);
            }
        }
    }

    /// Arm the perf log's input→present timer (no-op unless `SHROUD_PERF` is
    /// on and nothing is already pending). Call this only on paths that
    /// dispatch an event *and* request a redraw — an input that legitimately
    /// paints nothing (a bare modifier press, say) must not arm the timer, or
    /// the next unrelated frame reports a phantom quarter-second latency.
    fn perf_mark_input(&mut self, desc: &str) {
        if self.perf_log.is_some() && self.perf_input.is_none() {
            self.perf_input = Some((Instant::now(), desc.to_string()));
        }
    }

    /// Write an event-*arrival* line to the perf log (no-op with logging
    /// off). Frame lines only show when a frame painted; arrival lines pin
    /// when the OS actually delivered the event, so a slow input can be
    /// attributed to late delivery vs. a late frame. Like the frame lines,
    /// this must never carry text — descriptions are kinds and lengths only.
    fn perf_event_line(&mut self, desc: &str) {
        if let Some(log) = self.perf_log.as_mut() {
            use std::io::Write;
            let _ = writeln!(
                log,
                "[{:9.1}] event={desc}",
                self.perf_start.elapsed().as_secs_f64() * 1e3
            );
            let _ = log.flush();
        }
    }

    /// Handle a paste combo. Text is preferred: clipboard text is replayed
    /// as a burst of `CharInput` events against the current focus. When the
    /// clipboard has no text but holds an image, the image is encoded to PNG
    /// and delivered to the screen's [`WidgetTree::on_image_paste`] handler
    /// (e.g. to insert it into the open document). Used by the Ctrl+V
    /// interceptor above.
    ///
    /// Every failure (clipboard unavailable, no text and no image, encode
    /// error) is silently ignored — paste is a best-effort UX, not a
    /// critical path.
    fn dispatch_paste(&mut self) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let clipboard = SecureClipboard::new();

        // Text first: the common case, and an image copy carries no text, so
        // a non-empty text read short-circuits the image path below.
        if let Ok(text) = clipboard.read() {
            if !text.is_empty() {
                for ch in text.chars() {
                    tree.dispatch_event(&WidgetEvent::CharInput { ch }, &mut self.event_ctx);
                }
                self.request_redraw();
                return;
            }
        }

        // No text — try an image. Encode the raw clipboard pixels to PNG so
        // the handler receives a self-describing blob (the same shape the
        // file-drop path hands over) rather than format-specific raw RGBA.
        if let Ok(img) = clipboard.read_image() {
            if let Ok(png) = shroud_render::encode_png(img.width, img.height, &img.rgba) {
                tree.dispatch_image_paste(&png, &mut self.event_ctx);
                self.request_redraw();
            }
        }
    }

    /// Write any text a widget queued during the last dispatch (via
    /// [`EventContext::write_clipboard`]) to the OS clipboard. Drives copy /
    /// cut from a focused `Input`. A fresh [`SecureClipboard`] with no
    /// auto-clear timer is used so ordinary copied text persists like any
    /// normal clipboard write; failures are silently ignored (clipboard
    /// access is best-effort).
    fn flush_clipboard_write(&mut self) {
        if let Some(text) = self.event_ctx.take_clipboard_write() {
            let mut clipboard = SecureClipboard::new();
            let _ = clipboard.write(&text);
        }
    }

    /// Handle an event surfaced by the accessibility adapter.
    ///
    /// `AccessibilityDeactivated` needs no teardown: the adapter simply goes
    /// dormant and `update_if_active` no-ops again.
    ///
    /// An `ActionRequested` the translation doesn't understand, or that names a
    /// node no widget answers for, is dropped silently — an AT is free to ask
    /// for anything, and a stale node id is normal (the tree may have rebuilt
    /// since the AT read the snapshot). The redraw is unconditional on a
    /// translated action rather than gated on the tree acting: even a refused
    /// action can have moved focus.
    fn handle_a11y_event(&mut self, event: A11yEvent) {
        match event.window_event {
            A11yWindowEvent::InitialTreeRequested => self.push_a11y_update(),
            A11yWindowEvent::ActionRequested(request) => {
                let Some((node_id, action)) = action_from_request(&request) else {
                    return;
                };
                if let Some(tree) = self.tree.as_mut() {
                    tree.perform_access_action(node_id, action, &mut self.event_ctx);
                    self.request_redraw();
                }
            }
            A11yWindowEvent::AccessibilityDeactivated => {}
        }
    }

    /// Publish the current widget tree to the accessibility adapter.
    ///
    /// A no-op unless an AT is connected: `update_if_active` runs the closure
    /// (which walks the tree and translates it) only while the adapter is
    /// active, so the snapshot cost is paid solely when a screen reader is
    /// listening. Bounds come from the last layout pass, so callers should
    /// invoke this after layout (the redraw path does).
    fn push_a11y_update(&mut self) {
        // Read the scale before borrowing the adapter / tree: the translation
        // converts the snapshot's logical bounds to the physical ones accesskit
        // expects, and it has to be the same factor this frame's layout ran
        // against or the screen reader's highlight lands off the widget.
        let scale = self
            .renderer
            .as_ref()
            .map(|r| r.scale_factor())
            .unwrap_or(1.0);
        let (Some(adapter), Some(tree)) = (self.adapter.as_mut(), self.tree.as_ref()) else {
            return;
        };
        adapter.update_if_active(|| snapshot_to_tree_update(&tree.accessibility_snapshot(), scale));
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
        // Redraw watchdog: if a `request_redraw` has gone unserved past
        // `REDRAW_WATCHDOG`, ask again and keep the loop waking until the
        // frame lands (or the retry cap gives up). This is what puts the
        // first preedit of an IME composition on screen in ~20ms instead of
        // whenever the next keystroke or tick happens to rescue it — Windows
        // swallows the WM_PAINT while the IME opens its composition window.
        let mut wake_at = self.next_tick;

        // Caret-blink / timed-wake: park until the voted toggle deadline, then
        // ask for a single repaint. If it's already due, request the redraw
        // now and clear it — the paint that follows re-votes the *next* toggle
        // (or, once the field blurs, votes nothing and the loop idles). Same
        // "ask again from inside about_to_wait" shape as the watchdog below.
        if let Some(at) = self.anim_wake_at {
            if Instant::now() >= at {
                self.anim_wake_at = None;
                self.request_redraw();
            } else {
                wake_at = Some(wake_at.map_or(at, |t| t.min(at)));
            }
        }

        if let Some(since) = self.redraw_pending_since.get() {
            let mut deadline = since + REDRAW_WATCHDOG;
            if Instant::now() >= deadline {
                if self.redraw_retry_count.get() >= REDRAW_WATCHDOG_RETRIES {
                    // Repeated asks going nowhere (minimized / occluded):
                    // stop waking; the next input or tick re-arms.
                    self.redraw_pending_since.set(None);
                    deadline = wake_at.unwrap_or_else(Instant::now);
                } else {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    self.redraw_retry_count
                        .set(self.redraw_retry_count.get() + 1);
                    let now = Instant::now();
                    self.redraw_pending_since.set(Some(now));
                    deadline = now + REDRAW_WATCHDOG;
                }
            }
            if self.redraw_pending_since.get().is_some() {
                wake_at = Some(wake_at.map_or(deadline, |t| t.min(deadline)));
            }
        }

        // While a frame hook is registered (or a redraw is pending), park
        // the loop until the earlier of the next scheduled tick and the
        // watchdog deadline. `next_tick` is anchored to prior fires, so this
        // does not drift on every event.
        if let Some(deadline) = wake_at {
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

        // Publish the OS caret-blink preference so focused text fields blink at
        // the system rate — and hold solid when the user turned blinking off
        // (an accessibility choice). No-op if the app set its own policy via
        // `set_caret_blink`. Read once here, like the theme snapshot above; the
        // rate rarely changes mid-session and isn't a winit event.
        let caret_blink = match shroud_platform::caret_blink_time() {
            Some(interval) => shroud_widgets::caret::CaretBlink::Interval(interval),
            None => shroud_widgets::caret::CaretBlink::Off,
        };
        shroud_widgets::caret::set_caret_blink_from_system(caret_blink);

        let window_arc = platform_window.arc();

        // Bring up the accessibility adapter (unless opted out). It shares the
        // app's event-loop proxy, so AT connect / action / disconnect events
        // arrive as `AppEvent::Accessibility` in `user_event`. Creation is
        // cheap and the adapter stays dormant until an AT actually connects.
        if self.config.accessibility {
            self.adapter = Some(Adapter::with_event_loop_proxy(
                event_loop,
                window_arc.as_ref(),
                self.handle.proxy.clone(),
            ));
        }

        let renderer = pollster::block_on(Renderer::new(Arc::clone(&window_arc)));

        let theme = self.config.theme.get();
        publish_active_theme(&theme);
        let mut paint_ctx = PaintContext::new(theme);

        // Register bundled fonts (e.g. an icon font) into the text engine
        // before the first layout/paint, so widgets can resolve their families
        // by name from frame one. Guarded so a second `resumed` (suspend/resume)
        // doesn't re-add the same faces.
        if !self.fonts_loaded {
            for data in &self.config.fonts {
                paint_ctx.text_engine.load_font_data(data);
            }
            // Remap the generic sans-serif to the app's chosen UI family *after*
            // the bundled faces are registered, so a bundled name resolves.
            // Kills the Latin/CJK two-typeface split for unstyled text.
            if let Some(family) = &self.config.default_font_family {
                paint_ctx.text_engine.set_default_font_family(family);
            }
            self.fonts_loaded = true;
        }

        // Now that the accessibility adapter (if any) has been constructed,
        // it is safe to show the window for the first time — accesskit_winit
        // requires the adapter to exist before the window is first made
        // visible. The window was created hidden in `PlatformWindow::new`.
        platform_window.show();

        self.window = Some(platform_window);
        self.renderer = Some(renderer);
        self.paint_ctx = Some(paint_ctx);

        window_arc.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Wake => self.request_redraw(),
            AppEvent::Accessibility(a11y) => self.handle_a11y_event(a11y),
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

        // Let the accessibility adapter observe every window event first (it
        // watches focus / activation to drive AT hand-shakes). Take the window
        // Arc out before borrowing the adapter mutably so the two field borrows
        // don't overlap. Cheap and a no-op while no AT is connected.
        if let Some(win) = self.window.as_ref().map(|w| w.arc()) {
            if let Some(adapter) = self.adapter.as_mut() {
                adapter.process_event(win.as_ref(), &event);
            }
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

            // Window gained focus — re-associate the IME context. Windows can
            // drop the association across focus transitions, so calling
            // `set_ime_allowed(true)` again here re-runs winit's IACE_DEFAULT
            // path, keeping IME available across focus loss/regain. It does not
            // force composition mode on (no `ImmSetOpenStatus`), so refocusing
            // no longer snaps a Japanese layout back to hiragana — the user's
            // last open status stands.
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
                self.perf_event_line("push-ime-allowed(true)");
            }

            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                self.request_redraw();
            }

            // ── Mouse events ─────────────────────────────────────
            WindowEvent::CursorMoved { position, .. } => {
                // winit reports the cursor in physical pixels, but the tree
                // hit-tests in logical ones. Convert at this single write site
                // so every reader of `cursor_position` is already in tree
                // space — the same discipline the clip / hover paths follow.
                let scale = self
                    .renderer
                    .as_ref()
                    .map(|r| r.scale_factor())
                    .unwrap_or(1.0);
                self.cursor_position =
                    Point::new(position.x as f32 / scale, position.y as f32 / scale);
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
                    // Lines are a count, not a length: one notch should move
                    // the same 40 logical pixels at any scale, so this arm is
                    // deliberately scale-free.
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    // A pixel delta (precision touchpads) is *physical*, and
                    // the tree scrolls in logical units.
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        let scale = self
                            .renderer
                            .as_ref()
                            .map(|r| r.scale_factor())
                            .unwrap_or(1.0);
                        (pos.x as f32 / scale, pos.y as f32 / scale)
                    }
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
                // Arrival marker for every pressed key (identity withheld —
                // modifiers and characters both log as `key`).
                self.perf_event_line("key");

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
                            self.perf_mark_input("paste");
                            self.dispatch_paste();
                            return;
                        }
                        let events = translate_character(s, self.event_ctx.modifiers);
                        if !events.is_empty() {
                            self.perf_mark_input("key");
                            if let Some(tree) = &mut self.tree {
                                for ev in events {
                                    tree.dispatch_event(&ev, &mut self.event_ctx);
                                }
                            }
                            // A focused Input may have asked to copy / cut its
                            // selection (Ctrl+C / Ctrl+X arrive on this path as
                            // KeyDown). Flush that to the OS clipboard now.
                            self.flush_clipboard_write();
                            self.request_redraw();
                        }
                    }

                    // Named keys → either a KeyDown event (most named
                    // keys) or, for Space, a CharInput so the rest of the
                    // pipeline treats it as a printable character.
                    WinitKey::Named(named) => {
                        // Keys the IME consumes arrive as logical `Process`
                        // with the physical identity intact. Log arrows only:
                        // during composition *every* eaten key (romaji letters
                        // included) is `Process`, so naming any other physical
                        // key would put typed plaintext in the perf log.
                        if matches!(named, WinitNamedKey::Process) && self.perf_log.is_some() {
                            let phys = match event.physical_key {
                                PhysicalKey::Code(KeyCode::ArrowLeft) => "ArrowLeft",
                                PhysicalKey::Code(KeyCode::ArrowRight) => "ArrowRight",
                                PhysicalKey::Code(KeyCode::ArrowUp) => "ArrowUp",
                                PhysicalKey::Code(KeyCode::ArrowDown) => "ArrowDown",
                                _ => "other",
                            };
                            self.perf_event_line(&format!("process-key({phys})"));
                        }
                        if let Some(event) = translate_named_key(named) {
                            self.perf_mark_input("key");
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
            // Composed text from an IME (Japanese, Chinese, Korean, …).
            // Once IME is app-driven (`set_ime_allowed(true)`) winit stops
            // drawing an inline composition string and expects us to render
            // the preedit ourselves — so we forward `Preedit` as an
            // `ImePreedit` event the focused text widget paints inline, and
            // splat the final `Commit` through the existing `CharInput`
            // path. See `translate_ime` for the full mapping.
            WindowEvent::Ime(ime) => {
                if self.perf_log.is_some() {
                    // Length only — the perf log must never carry preedit or
                    // committed text (it may be the user's plaintext).
                    let desc = match &ime {
                        Ime::Preedit(s, _) => format!("preedit({})", s.chars().count()),
                        Ime::Commit(s) => format!("commit({})", s.chars().count()),
                        Ime::Enabled => "ime-on".to_string(),
                        Ime::Disabled => "ime-off".to_string(),
                    };
                    self.perf_event_line(&desc);
                    // On Windows the composition keystroke arrives as
                    // KeyboardInput a moment before its Ime event, so a
                    // pending `key` is this same physical keystroke: keep its
                    // (earlier) timestamp but let the IME label win, otherwise
                    // every Japanese keystroke logs as a bare `key`.
                    if let Some((_, d)) = self.perf_input.as_mut() {
                        if d.as_str() == "key" {
                            *d = desc;
                        }
                    } else {
                        self.perf_input = Some((Instant::now(), desc));
                    }
                }
                if let Some(tree) = &mut self.tree {
                    for event in translate_ime(&ime) {
                        tree.dispatch_event(&event, &mut self.event_ctx);
                    }
                    self.request_redraw();
                }
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
                // The OS delivered a paint: every earlier `request_redraw`
                // is answered by this frame, so disarm the watchdog.
                self.redraw_pending_since.set(None);
                self.redraw_retry_count.set(0);

                if self.renderer.is_none() || self.tree.is_none() || self.paint_ctx.is_none() {
                    return;
                }
                let perf_frame_start = Instant::now();

                // Clear last frame's animation votes. Any in-flight
                // `Animated` value read during this frame's layout/paint
                // re-votes; we check the tally after rendering and schedule
                // one more redraw if anything is still moving.
                shroud_reactive::animation::reset_frame_request();

                // Push the window's current scale factor before laying out.
                // Reading it per frame instead of caching it at startup is
                // what makes dragging the window to a monitor with different
                // scaling work: winit re-reports the value and the next frame
                // lays out against the new logical size, with no other
                // plumbing involved.
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor())
                    .unwrap_or(1.0);
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_scale_factor(scale);
                }
                // Layout runs in logical pixels; only the surface itself is
                // physical.
                let size = self.renderer.as_ref().unwrap().logical_surface_size();
                let tree = self.tree.as_mut().unwrap();
                let paint_ctx = self.paint_ctx.as_mut().unwrap();
                // Before layout, which shapes text through `measure`: the text
                // engine rasterizes glyphs at this scale and caches the shaped
                // output under it.
                paint_ctx.text_engine.set_scale(scale);

                // Reset the shape tally so the perf log's `shapes=` column
                // counts this frame only. Cheap; runs even with logging off.
                let _ = paint_ctx.text_engine.take_shape_stats();

                // Pull the latest theme value from the reactive source
                // before laying out / painting. For `App::theme(Theme)`
                // this is a cheap clone of a static value; for signal-
                // or closure-driven sources it picks up any update
                // pushed since the last frame, making
                // `Signal<Theme>::set(...)` (or a derived `Reactive`
                // that depends on `system_theme_signal()`) visible on
                // the very next paint without per-widget rewiring.
                let theme = self.config.theme.get();
                // Mirror it into the process-wide snapshot so `theme_color`
                // / `theme_value` accessors read this frame's tokens when
                // the tree paints below.
                publish_active_theme(&theme);
                paint_ctx.theme = theme;

                // Apply any deferred initial focus before layout so widget
                // state set by FocusGained (cursor visibility, focus ring)
                // is reflected in this very first paint of the new tree.
                // Cheap when nothing is pending; covers both the boot path
                // and screen transitions whose build closure called
                // `tree.focus_initially(...)`.
                tree.flush_pending_focus(&mut self.event_ctx);

                // A screen swap (e.g. an auto-lock replacing the vault with the
                // lock screen) invalidates the shape cache: its glyph geometry
                // was derived from the old screen's text, which for a notes app
                // is the user's plaintext. Drop it before re-shaping the new
                // screen so nothing note-derived outlives the lock.
                if tree.take_root_replaced() {
                    paint_ctx.text_engine.clear_shape_cache();
                }

                // Layout pass — widgets report intrinsic size via their
                // `measure()` so `.center()` / gap / grow work without a
                // fixed-width wrapper around leaves.
                let perf_layout_start = Instant::now();
                tree.compute_layout_with_measure(
                    size.0,
                    size.1,
                    &mut paint_ctx.text_engine,
                    &paint_ctx.theme,
                );

                // A layer that popped since the last frame may have uncovered a
                // widget sitting under a cursor that never moves again — the
                // button whose click dismissed the menu, typically. Replay the
                // hover hit-test now that this frame's geometry is resolved, so
                // the paint below already carries the hover state. When the
                // chain changes, its handlers may have moved the tree (a hover
                // that opens a tooltip layer), so lay out once more before
                // painting; this costs a second pass only on the rare frame
                // where a pop actually changed what is hovered.
                if tree.resync_hover(&mut self.event_ctx) {
                    tree.compute_layout_with_measure(
                        size.0,
                        size.1,
                        &mut paint_ctx.text_engine,
                        &paint_ctx.theme,
                    );
                }

                let perf_layout_end = Instant::now();

                paint_ctx.clear();
                tree.paint(paint_ctx);
                let perf_paint_end = Instant::now();

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

                // One perf-log line per painted frame (`SHROUD_PERF`). `gpu`
                // covers encode + submit + present, so vsync waits land there;
                // `input=` reports event-arrival → present latency for the
                // first frame after an input event. Text never appears here —
                // only lengths and timings.
                if self.perf_log.is_some() {
                    let perf_gpu_end = Instant::now();
                    let (shapes, shape_ns) = paint_ctx.text_engine.take_shape_stats();
                    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1e3;
                    let input = match self.perf_input.take() {
                        Some((t, desc)) => format!(" input={desc}@{:.1}ms", ms(t, perf_gpu_end)),
                        None => String::new(),
                    };
                    if let Some(log) = self.perf_log.as_mut() {
                        use std::io::Write;
                        let _ = writeln!(
                            log,
                            "[{:9.1}] frame={:5.1} layout={:5.1} paint={:5.1} gpu={:5.1} shapes={} shape_ms={:.1}{}",
                            ms(self.perf_start, perf_gpu_end),
                            ms(perf_frame_start, perf_gpu_end),
                            ms(perf_layout_start, perf_layout_end),
                            ms(perf_layout_end, perf_paint_end),
                            ms(perf_paint_end, perf_gpu_end),
                            shapes,
                            shape_ns as f64 / 1e6,
                            input
                        );
                        let _ = log.flush();
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
                    self.perf_event_line(if target_allowed {
                        "push-ime-allowed(true)"
                    } else {
                        "push-ime-allowed(false)"
                    });
                }

                // Publish the freshly laid-out tree to any connected screen
                // reader. No-op while no AT is listening (lazy `update_if_active`).
                self.push_a11y_update();

                // Pump the next frame while any animation is mid-flight. An
                // in-flight `Animated::get` voted during paint above; if so,
                // request another redraw. Once every animation settles no
                // vote is cast, so the loop returns to idle with no
                // busy-looping at rest.
                if shroud_reactive::animation::frame_requested() {
                    self.request_redraw();
                }

                // Capture any *timed* wake voted this paint (the caret's next
                // blink toggle). Unlike the flag above we don't redraw now —
                // `about_to_wait` parks until the deadline and repaints then,
                // so a blinking caret costs two frames a second, not sixty.
                self.anim_wake_at = shroud_reactive::animation::frame_deadline();
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
    fn ime_preedit_translates_to_preedit_event() {
        // FW-1: a composition update becomes one ImePreedit carrying the text
        // and the IME caret range, which the focused widget renders inline.
        let events = translate_ime(&Ime::Preedit("か".to_string(), Some((3, 3))));
        assert_eq!(events.len(), 1);
        match &events[0] {
            WidgetEvent::ImePreedit { text, cursor } => {
                assert_eq!(text.as_str(), "か");
                assert_eq!(*cursor, Some((3, 3)));
            }
            other => panic!("expected ImePreedit, got {other:?}"),
        }
    }

    #[test]
    fn ime_commit_clears_preedit_then_emits_chars() {
        // A commit first clears any active preedit (so the underlined
        // composition vanishes), then splats the committed chars through the
        // existing CharInput path — one event per code point.
        let events = translate_ime(&Ime::Commit("ねこ".to_string()));
        assert_eq!(events.len(), 3, "one clear + one char per code point");
        match &events[0] {
            WidgetEvent::ImePreedit { text, cursor } => {
                assert!(text.is_empty(), "commit must lead with a preedit clear");
                assert_eq!(*cursor, None);
            }
            other => panic!("expected leading ImePreedit clear, got {other:?}"),
        }
        assert!(matches!(events[1], WidgetEvent::CharInput { ch: 'ね' }));
        assert!(matches!(events[2], WidgetEvent::CharInput { ch: 'こ' }));
    }

    #[test]
    fn ime_enabled_and_disabled_clear_any_preedit() {
        // Enabled / Disabled carry no text; both clear any lingering preedit so
        // a half-composed string can't stick when the IME toggles (e.g. a
        // SecureInput's Tier-2 bypass disables IME on focus).
        for ime in [Ime::Enabled, Ime::Disabled] {
            let events = translate_ime(&ime);
            assert_eq!(events.len(), 1);
            match &events[0] {
                WidgetEvent::ImePreedit { text, cursor } => {
                    assert!(text.is_empty());
                    assert_eq!(*cursor, None);
                }
                other => panic!("expected ImePreedit clear, got {other:?}"),
            }
        }
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
    fn ctrl_shift_letter_promotes_to_key_down() {
        // Ctrl+Shift+Z is Input's redo chord. It must promote to a KeyDown
        // like any other Ctrl combo even though Shift is also held —
        // `shift_alone_stays_as_char_input` only applies when Shift is the
        // *sole* modifier. `Input::event` matches the redo key
        // case-insensitively, so assert the promotion, not the char's case.
        let events = translate_character(
            "z",
            Modifiers {
                shift: true,
                ctrl: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            WidgetEvent::KeyDown {
                key: Key::Character(_)
            }
        ));
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
    fn theme_color_tracks_the_published_theme() {
        // theme_color(...) must read whatever the paint loop last published
        // via publish_active_theme, and reflect a *later* publish through
        // the SAME Reactive handle — that is the live-theme-swap contract
        // apps get for free instead of hand-rolling a
        // Reactive::derive(|| my_theme().colors.X) wrapper per token.
        let primary = theme_color(|c| c.primary);

        let mut a = Theme::default();
        a.colors.primary = Color::rgb(0.1, 0.2, 0.3);
        publish_active_theme(&a);
        assert_eq!(primary.get(), a.colors.primary);

        let mut b = Theme::default();
        b.colors.primary = Color::rgb(0.9, 0.8, 0.7);
        publish_active_theme(&b);
        // Same handle, new value — a swap needs no per-widget rewiring.
        assert_eq!(primary.get(), b.colors.primary);
    }

    #[test]
    fn theme_value_reaches_tokens_outside_the_palette() {
        // theme_value is the general primitive underneath theme_color: it
        // must reach tokens that aren't palette colors (or live outside
        // Colors), e.g. hover.bg — the case a color-only accessor can't
        // express, which is exactly why theme_value exists.
        let hover_bg = theme_value(|t| t.hover.bg);
        let mut t = Theme::default();
        t.hover.bg = Color::rgb(0.4, 0.4, 0.4);
        publish_active_theme(&t);
        assert_eq!(hover_bg.get(), t.hover.bg);
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
