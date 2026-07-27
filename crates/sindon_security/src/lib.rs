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
