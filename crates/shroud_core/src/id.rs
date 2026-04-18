use slotmap::new_key_type;

new_key_type! {
    /// Identifies a widget in the widget tree.
    pub struct WidgetId;

    /// Identifies a reactive node (signal, memo, or effect) in the runtime.
    pub struct NodeId;

    /// Identifies a reactive scope for lifetime management.
    pub struct ScopeId;
}
