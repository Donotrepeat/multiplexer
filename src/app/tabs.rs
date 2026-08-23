use crate::app::pane::Pane;
use crossterm::terminal::size;
use ratatui::{layout::Rect, Frame};
use strum::{EnumIter, IntoEnumIterator};

#[derive(EnumIter)]
pub enum Grid {
    HORIZONTALE,
    VERTICAL,
    SQUIRE,
    GOLDER,
}
impl Grid {
    pub fn next(&self) -> Self {
        let mut iter = Grid::iter();
        let current = std::mem::discriminant(self);
        loop {
            let variant = iter.next().unwrap();
            if std::mem::discriminant(&variant) == current {
                return iter.next().unwrap_or_else(|| Grid::iter().next().unwrap());
            }
        }
    }
}
pub struct Tab {
    pub panes: Vec<Pane>,
    pub active: usize,
    pub grid: Grid,
}

impl Tab {
    pub fn new(row: u16, coll: u16) -> Self {
        log::debug!("screen {row},{coll}");
        Self {
            panes: vec![Pane::new(row, coll).unwrap()],
            active: 0,
            grid: Grid::HORIZONTALE,
        }
    }

    pub fn draw_tab(&self, frame: &mut Frame) {
        let total_panes = self.panes.len();
        let area = frame.area();
        match total_panes {
            1 => {
                // Single pane - fill entire screen (current behavior)
                if let Some(pane) = self.panes.get(self.active) {
                    pane.render_pane(frame, area, true);
                }
            }
            2.. => {
                // Two panes - split horizontally
                match self.grid {
                    Grid::HORIZONTALE => {
                        let chunk_size = area.height.saturating_div(total_panes as u16);

                        self.panes[0].render_pane(frame, Rect::new(area.x, area.y, area.width, chunk_size), self.active == 0);
                        for i in 1..total_panes {
                            let next_area = Rect::new(
                                area.x,
                                area.y + chunk_size * i as u16,
                                area.width,
                                chunk_size,
                            );

                            self.panes[i].render_pane(frame, next_area, self.active == i);
                        }
                    }
                    Grid::SQUIRE => {}
                    Grid::GOLDER => {}
                    Grid::VERTICAL => {
                        let chunk_size = area.width.saturating_div(total_panes as u16);
                        let top_area = Rect::new(area.x, area.y, chunk_size, area.height);

                        self.panes[0].render_pane(frame, top_area, self.active == 0);
                        for i in 1..total_panes {
                            let next_area = Rect::new(
                                area.x + chunk_size * i as u16,
                                area.y,
                                chunk_size,
                                area.height,
                            );

                            self.panes[i].render_pane(frame, next_area, self.active == i);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    //TODO match the new shape, resize the panes and place them on there new spot
    pub fn reshape(&mut self) {
        let size_term = size().unwrap();
        let pane_count = self.panes.len();
        match self.grid {
            Grid::HORIZONTALE => {
                let new_rows = size_term.0.saturating_div(pane_count as u16);
                let new_colls = size_term.1 - 2;
                for term in &mut self.panes {
                    term.resize(new_rows, new_colls);
                }
            }
            Grid::SQUIRE => {}
            Grid::GOLDER => {}
            Grid::VERTICAL => {
                let new_rows = size_term.0 - 2;
                let new_colls = size_term.1.saturating_div(pane_count as u16);
                for term in &mut self.panes {
                    term.resize(new_rows, new_colls);
                }
            }
        }
    }
}
