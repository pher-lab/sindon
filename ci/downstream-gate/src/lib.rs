//! Intentionally empty.
//!
//! This crate exists to be *resolved*, not compiled: `cargo metadata` on its
//! manifest produces the dependency graph a downstream consumer of sindon would
//! get, which is what `ci/check-fork-propagation.sh` inspects. Cargo requires a
//! target to exist, so here it is.
