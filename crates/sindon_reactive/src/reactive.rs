//! `Reactive<T>` — a value that is either static or produced on demand.
//!
//! Widget attributes that want to be signal-driven (e.g. `Button::label`,
//! `Container::background`) accept `impl Into<Reactive<T>>`. A literal value
//! becomes [`Reactive::Static`]; a [`Signal<T>`] / [`Memo<T>`] / closure
//! becomes [`Reactive::Dynamic`]. On every paint, dynamic variants
//! re-evaluate their closure so the widget sees the freshest value.
//!
//! Pull-based: there is no subscription. The widget tree re-reads every
//! `Reactive<T>` on each repaint; the surrounding event loop is responsible
//! for triggering redraws when state changes.
//!
//! # Examples
//!
//! ```
//! use sindon_reactive::{Reactive, Signal};
//!
//! // Static value
//! let r: Reactive<u32> = 42.into();
//! assert_eq!(r.get(), 42);
//!
//! // Signal-driven
//! let sig = Signal::new(7);
//! let r: Reactive<i32> = sig.into();
//! assert_eq!(r.get(), 7);
//! sig.set(9);
//! assert_eq!(r.get(), 9);
//!
//! // Closure-driven (explicit constructor — coherence prevents `From<F>`)
//! let r: Reactive<String> = Reactive::derive(|| format!("hi"));
//! assert_eq!(r.get(), "hi");
//! ```

use std::rc::Rc;

use crate::memo::Memo;
use crate::signal::Signal;

/// A value that is either static or produced by a closure on each read.
///
/// Static and Dynamic variants are interchangeable at call sites via
/// `impl Into<Reactive<T>>`, so widget builders can accept both literals and
/// reactive sources through a single method.
pub enum Reactive<T> {
    /// A plain value, cloned on each `get()`.
    Static(T),
    /// A closure re-evaluated on each `get()`. Wrapped in `Rc` so
    /// `Reactive<T>` itself is cheap to clone.
    Dynamic(Rc<dyn Fn() -> T>),
}

impl<T> Reactive<T> {
    /// Build a `Reactive::Dynamic` from a closure.
    ///
    /// Used when neither a literal nor a `Signal`/`Memo` conversion applies,
    /// e.g. when deriving a value from multiple signals:
    ///
    /// ```
    /// # use sindon_core::Color;
    /// # use sindon_reactive::{Reactive, Signal};
    /// # let enabled = Signal::new(true);
    /// let accent = Color::rgb(0.2, 0.5, 1.0);
    /// let bg = Reactive::derive(move || {
    ///     if enabled.get() { accent } else { Color::TRANSPARENT }
    /// });
    /// # assert_eq!(bg.get(), accent);
    /// ```
    pub fn derive(f: impl Fn() -> T + 'static) -> Self {
        Reactive::Dynamic(Rc::new(f))
    }

    /// Read the current value by reference, without cloning it.
    ///
    /// The borrowing counterpart to [`get`](Self::get), mirroring
    /// [`Signal::with`]. `Static` hands out a borrow of the held value;
    /// `Dynamic` still has to invoke the closure (which produces an owned
    /// value), so this only saves a clone on the static side — but that is
    /// exactly the side a per-frame `measure` / `paint` walks. Prefer it for
    /// container payloads such as `Reactive<Vec<String>>`, where `get()` would
    /// deep-clone the whole list on every read.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        match self {
            Reactive::Static(v) => f(v),
            Reactive::Dynamic(g) => f(&g()),
        }
    }
}

impl<T: Clone> Reactive<T> {
    /// Read the current value.
    ///
    /// For `Static` this clones the held value; for `Dynamic` this invokes
    /// the closure. Meant to be called during paint / layout passes.
    pub fn get(&self) -> T {
        match self {
            Reactive::Static(v) => v.clone(),
            Reactive::Dynamic(f) => f(),
        }
    }
}

// Manual Clone: Dynamic uses `Rc::clone` (cheap handle copy) so cloning a
// `Reactive` never re-runs a user closure or re-allocates its box.
impl<T: Clone> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        match self {
            Reactive::Static(v) => Reactive::Static(v.clone()),
            Reactive::Dynamic(f) => Reactive::Dynamic(Rc::clone(f)),
        }
    }
}

// ── Conversions ────────────────────────────────────────────────────────
//
// Note: we intentionally do NOT provide `impl<F: Fn() -> T> From<F>`:
// combined with `From<T>` below it would overlap (Rust can't disambiguate
// for a `T` that happens to be a closure type). Users with a bare closure
// call `Reactive::derive(|| ...)` instead.

/// A bare value becomes `Static`.
impl<T> From<T> for Reactive<T> {
    fn from(v: T) -> Self {
        Reactive::Static(v)
    }
}

/// A `Signal<T>` becomes `Dynamic`, re-read on each `get()`.
///
/// Bound is `T: Clone` rather than `T: Copy` so signals over non-`Copy`
/// payloads (notably `Signal<Theme>` for live theme swap) flow through
/// the same `.into()` site. Cloning happens once per paint frame; for
/// `Copy` types the clone collapses to a bitwise copy at codegen, so
/// this is a strict relaxation with no perf hit on the prior shape.
impl<T: Clone + 'static> From<Signal<T>> for Reactive<T> {
    fn from(s: Signal<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || s.get_clone()))
    }
}

/// A `Memo<T>` becomes `Dynamic`, cloning the cached value on each `get()`.
impl<T: Clone + PartialEq + 'static> From<Memo<T>> for Reactive<T> {
    fn from(m: Memo<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || m.with(|v| v.clone())))
    }
}
