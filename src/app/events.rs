/// Stable identity for a pane, independent of its position in the tab list
/// (which shifts as tabs and panes are added or removed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

/// Message from a pane's background threads to the UI loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneEvent {
    /// The pane's screen changed and needs a redraw. Coalesced by the
    /// session: at most one is pending per pane at a time.
    Output(PaneId),
    /// The pane's child exited (its reader hit EOF).
    Exited(PaneId),
    /// The foreground program set the pane's window/icon title.
    Title(PaneId, String),
}
