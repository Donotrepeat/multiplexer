/// Compute the pane dimensions from the host terminal size, leaving one cell
/// on each side for the block border drawn around each pane.
pub fn initialize_pane_size(row: u16, col: u16) -> (u16, u16) {
    let pane_rows = row.saturating_sub(2);
    let pane_cols = col.saturating_sub(2);
    (pane_rows, pane_cols)
}
