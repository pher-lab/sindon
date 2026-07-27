use sindon_security::ArenaSlot;
use slotmap::new_key_type;
use std::any::Any;

new_key_type! {
    /// Unique identifier for a node in the reactive graph.
    pub struct ReactiveId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    /// Value is up to date.
    Clean,
    /// A source may have changed — needs verification before use.
    MaybeDirty,
    /// A source definitely changed — needs recomputation.
    Dirty,
}

pub(crate) struct ReactiveNode {
    pub state: NodeState,
    pub kind: NodeKind,
    pub sources: Vec<ReactiveId>,
    pub subscribers: Vec<ReactiveId>,
    /// Owning scope, populated for future node-level cleanup dispatch.
    #[allow(dead_code)]
    pub scope: Option<ReactiveId>,
}

pub(crate) type EqFn = Box<dyn Fn(&dyn Any, &dyn Any) -> bool>;

pub(crate) enum NodeKind {
    Signal {
        value: Box<dyn Any>,
    },
    Memo {
        compute: Option<Box<dyn AnyCompute>>,
        value: Option<Box<dyn Any>>,
        eq_fn: EqFn,
    },
    Effect {
        callback: Option<Box<dyn FnMut()>>,
    },
    Scope {
        children: Vec<ReactiveId>,
        cleanups: Vec<Box<dyn FnOnce()>>,
    },
    /// Secure signal — value stored externally in the arena.
    #[allow(dead_code)]
    SecureSignal {
        /// Arena slot; the parallel `SecureSignal` handle owns the live
        /// reference — kept here for future node-level zeroize dispatch.
        slot: ArenaSlot,
        /// Type-erased drop: calls `T::zeroize()` then `drop_in_place`.
        drop_fn: unsafe fn(*mut u8),
    },
    /// Secure memo — cached derivation stored in the arena.
    SecureMemo {
        compute: Option<Box<dyn AnySecureCompute>>,
        slot: ArenaSlot,
        has_value: bool,
        #[allow(dead_code)]
        drop_fn: unsafe fn(*mut u8),
    },
}

/// Type-erased computation for Memo nodes.
pub(crate) trait AnyCompute {
    fn run(&mut self) -> Box<dyn Any>;
}

pub(crate) struct ComputeFn<F> {
    pub f: F,
}

impl<T: 'static, F: FnMut() -> T> AnyCompute for ComputeFn<F> {
    fn run(&mut self) -> Box<dyn Any> {
        Box::new((self.f)())
    }
}

/// Type-erased computation for SecureMemo nodes.
///
/// Runs the compute function, compares with the arena-stored value,
/// writes the new value if different, and returns whether the value changed.
pub(crate) trait AnySecureCompute {
    fn recompute(&mut self, slot: ArenaSlot, has_value: bool) -> bool;
}
