use crate::node::*;
use slotmap::SlotMap;
use std::cell::{Cell, RefCell};

thread_local! {
    pub(crate) static RUNTIME: ReactiveRuntime = ReactiveRuntime::new();
}

pub(crate) struct ReactiveRuntime {
    nodes: RefCell<SlotMap<ReactiveId, ReactiveNode>>,
    observer: Cell<Option<ReactiveId>>,
    batch_depth: Cell<u32>,
    pending_effects: RefCell<Vec<ReactiveId>>,
    current_scope: Cell<Option<ReactiveId>>,
    is_flushing: Cell<bool>,
}

impl ReactiveRuntime {
    pub fn new() -> Self {
        Self {
            nodes: RefCell::new(SlotMap::with_key()),
            observer: Cell::new(None),
            batch_depth: Cell::new(0),
            pending_effects: RefCell::new(Vec::new()),
            current_scope: Cell::new(None),
            is_flushing: Cell::new(false),
        }
    }

    // ── Signal operations ──────────────────────────────────────────

    pub fn create_signal<T: 'static>(&self, value: T) -> ReactiveId {
        let scope = self.current_scope.get();
        let node = ReactiveNode {
            state: NodeState::Clean,
            kind: NodeKind::Signal {
                value: Box::new(value),
            },
            sources: Vec::new(),
            subscribers: Vec::new(),
            scope,
        };
        let id = self.nodes.borrow_mut().insert(node);
        self.register_in_scope(id);
        id
    }

    pub fn read_signal<T: 'static, R>(&self, id: ReactiveId, f: impl FnOnce(&T) -> R) -> R {
        self.track(id);
        let nodes = self.nodes.borrow();
        let node = &nodes[id];
        match &node.kind {
            NodeKind::Signal { value } => {
                let val = value.downcast_ref::<T>().expect("Signal type mismatch");
                f(val)
            }
            _ => panic!("Node is not a Signal"),
        }
    }

    pub fn write_signal<T: 'static>(&self, id: ReactiveId, new_value: T) {
        {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[id];
            match &mut node.kind {
                NodeKind::Signal { value } => {
                    *value = Box::new(new_value);
                }
                _ => panic!("Node is not a Signal"),
            }
        }
        self.notify_subscribers(id);
    }

    pub fn update_signal<T: 'static>(&self, id: ReactiveId, f: impl FnOnce(&mut T)) {
        {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[id];
            match &mut node.kind {
                NodeKind::Signal { value } => {
                    let val = value.downcast_mut::<T>().expect("Signal type mismatch");
                    f(val);
                }
                _ => panic!("Node is not a Signal"),
            }
        }
        self.notify_subscribers(id);
    }

    // ── Memo operations ────────────────────────────────────────────

    pub fn create_memo<T: PartialEq + 'static>(
        &self,
        compute: impl FnMut() -> T + 'static,
    ) -> ReactiveId {
        let eq_fn: crate::node::EqFn =
            Box::new(
                |a, b| match (a.downcast_ref::<T>(), b.downcast_ref::<T>()) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                },
            );

        let scope = self.current_scope.get();
        let node = ReactiveNode {
            state: NodeState::Dirty, // needs initial computation
            kind: NodeKind::Memo {
                compute: Some(Box::new(ComputeFn { f: compute })),
                value: None,
                eq_fn,
            },
            sources: Vec::new(),
            subscribers: Vec::new(),
            scope,
        };
        let id = self.nodes.borrow_mut().insert(node);
        self.register_in_scope(id);
        id
    }

    pub fn read_memo<T: 'static, R>(&self, id: ReactiveId, f: impl FnOnce(&T) -> R) -> R {
        self.update_if_needed(id);
        self.track(id);
        let nodes = self.nodes.borrow();
        let node = &nodes[id];
        match &node.kind {
            NodeKind::Memo { value, .. } => {
                let val = value.as_ref().expect("Memo should have value after update");
                let val = val.downcast_ref::<T>().expect("Memo type mismatch");
                f(val)
            }
            _ => panic!("Node is not a Memo"),
        }
    }

    // ── Effect operations ──────────────────────────────────────────

    pub fn create_effect(&self, callback: impl FnMut() + 'static) -> ReactiveId {
        let scope = self.current_scope.get();
        let node = ReactiveNode {
            state: NodeState::Dirty,
            kind: NodeKind::Effect {
                callback: Some(Box::new(callback)),
            },
            sources: Vec::new(),
            subscribers: Vec::new(),
            scope,
        };
        let id = self.nodes.borrow_mut().insert(node);
        self.register_in_scope(id);

        // Run immediately for initial execution
        self.run_effect(id);
        id
    }

    // ── Scope operations ───────────────────────────────────────────

    pub fn create_scope(&self) -> ReactiveId {
        let parent = self.current_scope.get();
        let node = ReactiveNode {
            state: NodeState::Clean,
            kind: NodeKind::Scope {
                children: Vec::new(),
                cleanups: Vec::new(),
            },
            sources: Vec::new(),
            subscribers: Vec::new(),
            scope: parent,
        };
        let id = self.nodes.borrow_mut().insert(node);

        // Register as child of parent scope
        if let Some(parent_id) = parent {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(parent_node) = nodes.get_mut(parent_id)
                && let NodeKind::Scope { children, .. } = &mut parent_node.kind
            {
                children.push(id);
            }
        }
        id
    }

    pub fn run_in_scope<R>(&self, scope_id: ReactiveId, f: impl FnOnce() -> R) -> R {
        let prev = self.current_scope.get();
        self.current_scope.set(Some(scope_id));
        let result = f();
        self.current_scope.set(prev);
        result
    }

    pub fn add_cleanup(&self, scope_id: ReactiveId, cleanup: impl FnOnce() + 'static) {
        let mut nodes = self.nodes.borrow_mut();
        if let Some(node) = nodes.get_mut(scope_id)
            && let NodeKind::Scope { cleanups, .. } = &mut node.kind
        {
            cleanups.push(Box::new(cleanup));
        }
    }

    pub fn dispose_scope(&self, scope_id: ReactiveId) {
        // 1. Collect children and cleanups
        let (children, cleanups) = {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(node) = nodes.get_mut(scope_id) {
                if let NodeKind::Scope { children, cleanups } = &mut node.kind {
                    (std::mem::take(children), std::mem::take(cleanups))
                } else {
                    return;
                }
            } else {
                return;
            }
        };

        // 2. Separate child scopes from other children
        let (child_scopes, other_children): (Vec<_>, Vec<_>) = {
            let nodes = self.nodes.borrow();
            children.into_iter().partition(|id| {
                nodes
                    .get(*id)
                    .is_some_and(|n| matches!(n.kind, NodeKind::Scope { .. }))
            })
        };

        // 3. Dispose child scopes recursively
        for child_scope in &child_scopes {
            self.dispose_scope(*child_scope);
        }

        // 4. Run cleanups in reverse order
        for cleanup in cleanups.into_iter().rev() {
            cleanup();
        }

        // 5. Remove pending effects belonging to this scope
        {
            let mut pending = self.pending_effects.borrow_mut();
            pending.retain(|id| !other_children.contains(id) && !child_scopes.contains(id));
        }

        // 6. Unsubscribe and remove all non-scope children
        self.remove_nodes(&other_children);

        // 7. Remove the child scope nodes (already disposed)
        {
            let mut nodes = self.nodes.borrow_mut();
            for id in child_scopes {
                nodes.remove(id);
            }
        }

        // 8. Remove the scope node itself
        {
            let mut nodes = self.nodes.borrow_mut();
            nodes.remove(scope_id);
        }
    }

    pub fn current_scope(&self) -> Option<ReactiveId> {
        self.current_scope.get()
    }

    // ── Secure signal operations ──────────────────────────────────

    pub fn create_secure_signal_node(
        &self,
        slot: sindon_security::ArenaSlot,
        drop_fn: unsafe fn(*mut u8),
    ) -> ReactiveId {
        let scope = self.current_scope.get();
        let node = ReactiveNode {
            state: NodeState::Clean,
            kind: NodeKind::SecureSignal { slot, drop_fn },
            sources: Vec::new(),
            subscribers: Vec::new(),
            scope,
        };
        let id = self.nodes.borrow_mut().insert(node);
        self.register_in_scope(id);
        id
    }

    /// Register a dependency (public version of `track` for external callers).
    pub fn track_read(&self, id: ReactiveId) {
        self.track(id);
    }

    /// Notify subscribers of a value change (public version of `notify_subscribers`).
    pub fn notify_write(&self, id: ReactiveId) {
        self.notify_subscribers(id);
    }

    // ── Secure memo operations ─────────────────────────────────────

    pub fn create_secure_memo_node(
        &self,
        compute: Box<dyn AnySecureCompute>,
        slot: sindon_security::ArenaSlot,
        drop_fn: unsafe fn(*mut u8),
    ) -> ReactiveId {
        let scope = self.current_scope.get();
        let node = ReactiveNode {
            state: NodeState::Dirty, // needs initial computation
            kind: NodeKind::SecureMemo {
                compute: Some(compute),
                slot,
                has_value: false,
                drop_fn,
            },
            sources: Vec::new(),
            subscribers: Vec::new(),
            scope,
        };
        let id = self.nodes.borrow_mut().insert(node);
        self.register_in_scope(id);
        id
    }

    /// Ensure a secure memo is up-to-date, track it, and return its arena slot.
    pub fn read_secure_memo(&self, id: ReactiveId) -> sindon_security::ArenaSlot {
        self.update_if_needed(id);
        self.track(id);
        let nodes = self.nodes.borrow();
        match &nodes[id].kind {
            NodeKind::SecureMemo { slot, .. } => *slot,
            _ => panic!("Node is not a SecureMemo"),
        }
    }

    // ── Batch operations ───────────────────────────────────────────

    pub fn start_batch(&self) {
        self.batch_depth.set(self.batch_depth.get() + 1);
    }

    pub fn end_batch(&self) {
        let depth = self.batch_depth.get();
        assert!(depth > 0, "end_batch called without matching start_batch");
        self.batch_depth.set(depth - 1);
        if depth == 1 {
            self.flush_effects();
        }
    }

    // ── Dependency tracking ────────────────────────────────────────

    fn track(&self, source_id: ReactiveId) {
        let observer_id = match self.observer.get() {
            Some(id) => id,
            None => return,
        };

        let mut nodes = self.nodes.borrow_mut();

        if let Some(source) = nodes.get_mut(source_id)
            && !source.subscribers.contains(&observer_id)
        {
            source.subscribers.push(observer_id);
        }

        if let Some(observer) = nodes.get_mut(observer_id)
            && !observer.sources.contains(&source_id)
        {
            observer.sources.push(source_id);
        }
    }

    fn notify_subscribers(&self, id: ReactiveId) {
        let subscribers = {
            let nodes = self.nodes.borrow();
            match nodes.get(id) {
                Some(node) => node.subscribers.clone(),
                None => return,
            }
        };

        for &sub_id in &subscribers {
            self.mark_dirty(sub_id);
        }

        if self.batch_depth.get() == 0 && !self.is_flushing.get() {
            self.flush_effects();
        }
    }

    fn mark_dirty(&self, id: ReactiveId) {
        let subscribers = {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(node) = nodes.get_mut(id) {
                node.state = NodeState::Dirty;
                node.subscribers.clone()
            } else {
                return;
            }
        };

        // Transitive subscribers become MaybeDirty
        for &sub_id in &subscribers {
            self.mark_maybe_dirty(sub_id);
        }

        // Queue effects
        {
            let nodes = self.nodes.borrow();
            if let Some(node) = nodes.get(id)
                && matches!(node.kind, NodeKind::Effect { .. })
            {
                let mut pending = self.pending_effects.borrow_mut();
                if !pending.contains(&id) {
                    pending.push(id);
                }
            }
        }
    }

    fn mark_maybe_dirty(&self, id: ReactiveId) {
        let subscribers = {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(node) = nodes.get_mut(id) {
                if node.state == NodeState::Clean {
                    node.state = NodeState::MaybeDirty;
                    node.subscribers.clone()
                } else {
                    // Already Dirty or MaybeDirty — no need to propagate further
                    return;
                }
            } else {
                return;
            }
        };

        for &sub_id in &subscribers {
            self.mark_maybe_dirty(sub_id);
        }

        // Queue effects
        {
            let nodes = self.nodes.borrow();
            if let Some(node) = nodes.get(id)
                && matches!(node.kind, NodeKind::Effect { .. })
            {
                let mut pending = self.pending_effects.borrow_mut();
                if !pending.contains(&id) {
                    pending.push(id);
                }
            }
        }
    }

    // ── Reactively algorithm: lazy evaluation ──────────────────────

    fn update_if_needed(&self, id: ReactiveId) {
        let state = {
            let nodes = self.nodes.borrow();
            nodes.get(id).map(|n| n.state)
        };

        match state {
            Some(NodeState::Clean) => return,
            Some(NodeState::MaybeDirty) => {
                let sources = {
                    let nodes = self.nodes.borrow();
                    nodes.get(id).map(|n| n.sources.clone()).unwrap_or_default()
                };

                for source_id in sources {
                    self.update_if_needed(source_id);

                    // After updating a source, our state may have been promoted to Dirty
                    let my_state = {
                        let nodes = self.nodes.borrow();
                        nodes.get(id).map(|n| n.state)
                    };
                    if my_state == Some(NodeState::Dirty) {
                        break;
                    }
                }

                // If still MaybeDirty after checking all sources → actually clean
                let my_state = {
                    let nodes = self.nodes.borrow();
                    nodes.get(id).map(|n| n.state)
                };
                if my_state == Some(NodeState::MaybeDirty) {
                    let mut nodes = self.nodes.borrow_mut();
                    if let Some(node) = nodes.get_mut(id) {
                        node.state = NodeState::Clean;
                    }
                    return;
                }
            }
            Some(NodeState::Dirty) => {}
            None => return,
        }

        // Node is Dirty — recompute if it's a Memo or SecureMemo
        let node_kind = {
            let nodes = self.nodes.borrow();
            nodes.get(id).map(|n| match &n.kind {
                NodeKind::Memo { .. } => 1,
                NodeKind::SecureMemo { .. } => 2,
                _ => 0,
            })
        };

        match node_kind {
            Some(1) => self.recompute_memo(id),
            Some(2) => self.recompute_secure_memo(id),
            _ => {}
        }
    }

    fn recompute_memo(&self, id: ReactiveId) {
        // 1. Take compute function out (releases borrow after block)
        let compute = {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[id];
            match &mut node.kind {
                NodeKind::Memo { compute, .. } => compute.take(),
                _ => return,
            }
        };

        let Some(mut compute_fn) = compute else {
            return;
        };

        // 2. Clear old dependency links
        self.clear_sources(id);

        // 3. Run computation with dependency tracking
        let prev_observer = self.observer.get();
        self.observer.set(Some(id));
        let new_value = compute_fn.run();
        self.observer.set(prev_observer);

        // 4. Compare with old value, store new value, put compute back
        let (changed, subscribers) = {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[id];
            match &mut node.kind {
                NodeKind::Memo {
                    value,
                    eq_fn,
                    compute: slot,
                } => {
                    let changed = match value {
                        Some(old) => !eq_fn(old.as_ref(), new_value.as_ref()),
                        None => true,
                    };
                    *value = Some(new_value);
                    *slot = Some(compute_fn);
                    node.state = NodeState::Clean;
                    (changed, node.subscribers.clone())
                }
                _ => unreachable!(),
            }
        };

        // 5. If value changed, promote direct subscribers from MaybeDirty → Dirty
        if changed {
            let mut nodes = self.nodes.borrow_mut();
            for &sub_id in &subscribers {
                if let Some(sub) = nodes.get_mut(sub_id)
                    && sub.state == NodeState::MaybeDirty
                {
                    sub.state = NodeState::Dirty;
                }
            }
        }
    }

    fn recompute_secure_memo(&self, id: ReactiveId) {
        // 1. Take compute function and slot info out
        let (compute, slot, has_value) = {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[id];
            match &mut node.kind {
                NodeKind::SecureMemo {
                    compute,
                    slot,
                    has_value,
                    ..
                } => (compute.take(), *slot, *has_value),
                _ => return,
            }
        };

        let Some(mut compute_fn) = compute else {
            return;
        };

        // 2. Clear old dependency links
        self.clear_sources(id);

        // 3. Run computation with dependency tracking
        //    The compute function handles arena read/compare/write internally
        let prev_observer = self.observer.get();
        self.observer.set(Some(id));
        let changed = compute_fn.recompute(slot, has_value);
        self.observer.set(prev_observer);

        // 4. Put compute back, update state
        let subscribers = {
            let mut nodes = self.nodes.borrow_mut();
            let node = &mut nodes[id];
            match &mut node.kind {
                NodeKind::SecureMemo {
                    compute: c,
                    has_value: hv,
                    ..
                } => {
                    *c = Some(compute_fn);
                    *hv = true;
                    node.state = NodeState::Clean;
                    node.subscribers.clone()
                }
                _ => unreachable!(),
            }
        };

        // 5. If value changed, promote subscribers
        if changed {
            let mut nodes = self.nodes.borrow_mut();
            for &sub_id in &subscribers {
                if let Some(sub) = nodes.get_mut(sub_id)
                    && sub.state == NodeState::MaybeDirty
                {
                    sub.state = NodeState::Dirty;
                }
            }
        }
    }

    fn run_effect(&self, id: ReactiveId) {
        // 1. Take callback out
        let callback = {
            let mut nodes = self.nodes.borrow_mut();
            match &mut nodes[id].kind {
                NodeKind::Effect { callback } => callback.take(),
                _ => return,
            }
        };

        let Some(mut callback_fn) = callback else {
            return;
        };

        // 2. Clear old dependency links
        self.clear_sources(id);

        // 3. Run with dependency tracking
        let prev_observer = self.observer.get();
        self.observer.set(Some(id));
        callback_fn();
        self.observer.set(prev_observer);

        // 4. Put callback back, mark clean
        {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(node) = nodes.get_mut(id) {
                if let NodeKind::Effect { callback } = &mut node.kind {
                    *callback = Some(callback_fn);
                }
                node.state = NodeState::Clean;
            }
        }
    }

    fn flush_effects(&self) {
        if self.is_flushing.get() {
            return;
        }
        self.is_flushing.set(true);

        loop {
            let pending = {
                let mut p = self.pending_effects.borrow_mut();
                if p.is_empty() {
                    break;
                }
                std::mem::take(&mut *p)
            };

            for effect_id in pending {
                // Resolve MaybeDirty → check sources
                self.update_if_needed(effect_id);

                let state = {
                    let nodes = self.nodes.borrow();
                    nodes.get(effect_id).map(|n| n.state)
                };

                if state == Some(NodeState::Dirty) {
                    self.run_effect(effect_id);
                }
            }
        }

        self.is_flushing.set(false);
    }

    // ── Internal helpers ───────────────────────────────────────────

    fn clear_sources(&self, id: ReactiveId) {
        let mut nodes = self.nodes.borrow_mut();
        let old_sources = if let Some(node) = nodes.get_mut(id) {
            std::mem::take(&mut node.sources)
        } else {
            return;
        };

        for source_id in old_sources {
            if let Some(source) = nodes.get_mut(source_id) {
                source.subscribers.retain(|s| *s != id);
            }
        }
    }

    fn register_in_scope(&self, id: ReactiveId) {
        let scope_id = match self.current_scope.get() {
            Some(s) => s,
            None => return,
        };

        let mut nodes = self.nodes.borrow_mut();
        if let Some(scope) = nodes.get_mut(scope_id)
            && let NodeKind::Scope { children, .. } = &mut scope.kind
        {
            children.push(id);
        }
    }

    fn remove_nodes(&self, ids: &[ReactiveId]) {
        let mut nodes = self.nodes.borrow_mut();
        for &id in ids {
            // Unsubscribe from all sources
            if let Some(node) = nodes.get(id) {
                let sources = node.sources.clone();
                for source_id in sources {
                    if let Some(source) = nodes.get_mut(source_id) {
                        source.subscribers.retain(|s| *s != id);
                    }
                }
            }
            // Remove subscribers pointing to this node
            if let Some(node) = nodes.get(id) {
                let subscribers = node.subscribers.clone();
                for sub_id in subscribers {
                    if let Some(sub) = nodes.get_mut(sub_id) {
                        sub.sources.retain(|s| *s != id);
                    }
                }
            }
            nodes.remove(id);
        }
    }
}
