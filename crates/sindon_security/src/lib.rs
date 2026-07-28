//! sindon_security — Core secret-handling primitives.
//!
//! The foundation of sindon's zeroize-first model. Every type here
//! guarantees contents are wiped on drop.
//!
//! - [`secure_string`], [`secure_buffer`]: owned buffers with
//!   `Drop`-time zeroization and closure-scoped access (`expose`)
//! - [`arena`]: `SecureArena` backs `mlock`'d allocations for reactive values
//! - [`constant_time`]: timing-safe equality
//! - [`hardening`]: process-level protections (core dump disable,
//!   ptrace prevention, Windows exploit mitigation)

pub mod arena;
pub mod constant_time;
pub mod hardening;
pub mod secure_buffer;
pub mod secure_string;

pub use arena::{ArenaError, ArenaSlot, DEFAULT_ARENA_CAPACITY, SecureArena};
pub use secure_buffer::SecureBuffer;
pub use secure_string::SecureString;

/// The `zeroize` crate this build links against, re-exported so downstream
/// code cannot version-skew against it.
///
/// `zeroize` is part of sindon's public surface, not an implementation detail:
/// `SecureString` and `SecureBuffer` implement `Zeroize` / `ZeroizeOnDrop`, and
/// `SecureSignal<T>` / `SecureMemo<T>` bound `T` on `Zeroize`. So storing a
/// secret type of your own in a secure signal means implementing a trait that
/// belongs to *this* copy of `zeroize`.
///
/// Reach it through here — `sindon::security::zeroize::Zeroize`, with the
/// `derive` feature already enabled — rather than adding a separate `zeroize`
/// dependency. A version that resolves to a second copy of the crate compiles
/// its own distinct `Zeroize` trait, and the bound then fails to be satisfied
/// by a type that visibly implements it.
pub use zeroize;
