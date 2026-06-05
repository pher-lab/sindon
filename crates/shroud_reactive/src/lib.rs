//! shroud_reactive — Fine-grained reactive system (SolidJS-style signals).
//!
//! Provides `Signal<T>`, `Memo<T>`, `Effect`, and `Scope` with automatic
//! dependency tracking and the Reactively lazy-evaluation algorithm.
//!
//! For sensitive data, `SecureSignal<T>` and `SecureMemo<T>` store values
//! in a mlock'd arena with automatic zeroization.

pub mod animation;
pub mod batch;
pub mod effect;
pub mod memo;
pub mod node;
pub mod reactive;
pub(crate) mod runtime;
pub mod scope;
pub mod secure_memo;
pub mod secure_signal;
pub mod signal;

pub use animation::{Animated, Easing};
pub use batch::batch;
pub use effect::Effect;
pub use memo::Memo;
pub use node::ReactiveId;
pub use reactive::Reactive;
pub use scope::{Scope, on_cleanup};
pub use secure_memo::SecureMemo;
pub use secure_signal::SecureSignal;
pub use signal::Signal;
