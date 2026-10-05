use std::sync::mpsc::Sender;

use crate::app::config::TerminalConfig;
use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::Pane;
use ratatui::{Frame, layout::Rect};

use anyhow::Result;

mod grid;

pub use grid::Grid;

pub struct Tab {
    pub panes: Vec<Pane>,
    pub active: usize,
    pub grid: Grid,
}

/// Index of the pane that should be active after removing `active` from a tab
/// that currently holds `len` panes. `len` is the pre-removal count.
fn active_after_removal(active: usize, len: usize) -> usize {
    let remaining = len.saturating_sub(1);
    if active >= remaining {
        remaining.saturating_sub(1)
    } else {
        active
    }
}

impl Tab {
    pub fn new(
        row: u16,
        col: u16,
        id: PaneId,
        tx: Sender<PaneEvent>,
        terminal: &TerminalConfig,
    ) -> Result<Self> {
        log::debug!("screen {row},{col}");
        let panes = vec![Pane::new(row, col, id, tx, terminal)?];

        Ok(Self {
            panes,
            active: 0,
            grid: Grid::Horizontal,
        })
    }

    pub fn update(&mut self, area: Rect) {
        if self.panes.is_empty() {
            return;
        }
        let rects = self.rects(area);
        // The single source of truth for pane sizing: every pane's virtual
        // terminal is resized to its renderable rect (the block border takes
        // one cell on each side). Running this for every tab — not just the
        // visible one — keeps background shells' sizes and SIGWINCH current.
        for (i, pane) in self.panes.iter_mut().enumerate() {
            if let Some(&rect) = rects.get(i) {
                pane.resize(
                    rect.height.saturating_sub(2).max(2),
                    rect.width.saturating_sub(2).max(2),
                );
            }
        }
    }

    pub fn draw_tab(&mut self, frame: &mut Frame, area: Rect) {
        if self.panes.is_empty() {
            return;
        }
        let rects = self.rects(area);
        for (i, pane) in self.panes.iter_mut().enumerate() {
            if let Some(&rect) = rects.get(i) {
                pane.render_pane(frame, rect, self.active == i);
            }
        }
    }

    /// Screen rectangles for each pane under the tab's current grid.
    fn rects(&self, area: Rect) -> Vec<Rect> {
        let total_panes = self.panes.len() as u16;
        match self.grid {
            Grid::Vertical => grid::vertical_rects(area, total_panes),
            Grid::Square => grid::grid_rects(area, total_panes),
            Grid::Golden => grid::golden_rects(area, total_panes),
            _ => grid::horizontal_rects(area, total_panes),
        }
    }

    pub fn del_pane(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        debug_assert!(self.active < self.panes.len(), "active pane out of bounds");
        let removed = self.active;
        let new_active = active_after_removal(self.active, self.panes.len());
        self.panes.remove(removed);
        self.active = new_active;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Hands out panes and tabs with distinct ids. The event receiver is
    /// dropped: tests do not observe events and sends failing is harmless.
    struct PaneMaker {
        next: u64,
        tx: Sender<PaneEvent>,
    }

    impl PaneMaker {
        fn new() -> Self {
            let (tx, _rx) = mpsc::channel();
            Self { next: 0, tx }
        }

        fn pane(&mut self) -> Result<Pane> {
            let id = PaneId(self.next);
            self.next += 1;
            Pane::new(4, 20, id, self.tx.clone(), &TerminalConfig::default())
        }

        fn tab(&mut self) -> Result<Tab> {
            let id = PaneId(self.next);
            self.next += 1;
            Tab::new(4, 20, id, self.tx.clone(), &TerminalConfig::default())
        }
    }

    #[test]
    fn update_resizes_every_pane_to_its_cell() -> Result<()> {
        let mut maker = PaneMaker::new();
        let mut tab = maker.tab()?;
        tab.panes.push(maker.pane()?);
        tab.panes.push(maker.pane()?);

        let area = Rect::new(0, 1, 41, 10);
        tab.update(area);

        let rects = grid::horizontal_rects(area, tab.panes.len() as u16);
        for (pane, rect) in tab.panes.iter().zip(rects) {
            let expected = (
                rect.height.saturating_sub(2).max(2),
                rect.width.saturating_sub(2).max(2),
            );
            assert_eq!(pane.size(), expected);
        }
        Ok(())
    }

    #[test]
    fn active_after_removal_is_in_bounds() {
        let cases = [
            (1usize, 0usize, 0usize),
            (2, 0, 0),
            (2, 1, 0),
            (3, 0, 0),
            (3, 1, 1),
            (3, 2, 1),
            (4, 3, 2),
        ];
        for (len, active, expected) in cases {
            let got = active_after_removal(active, len);
            assert_eq!(got, expected, "len={len} active={active}");
            assert!(
                got < len.saturating_sub(1).max(1),
                "len={len} active={active} -> {got} out of bounds"
            );
        }
    }

    #[test]
    fn del_pane_never_underflows_and_keeps_active_in_bounds() -> Result<()> {
        let mut maker = PaneMaker::new();

        // Deleting the only pane empties the tab without panicking.
        let mut single = maker.tab()?;
        single.del_pane();
        assert!(single.panes.is_empty());
        assert_eq!(single.active, 0);

        let mut tab = maker.tab()?;
        tab.panes.push(maker.pane()?);
        tab.panes.push(maker.pane()?);

        // Deleting a middle pane keeps the same index.
        tab.active = 1;
        tab.del_pane();
        assert_eq!(tab.panes.len(), 2);
        assert_eq!(tab.active, 1);

        // Deleting the last pane moves the index back one.
        tab.active = 1;
        tab.del_pane();
        assert_eq!(tab.panes.len(), 1);
        assert_eq!(tab.active, 0);

        Ok(())
    }
}
