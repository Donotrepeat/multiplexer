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
                        let chunk_size = area.height / total_panes as u16;
                        let remainder = area.height % total_panes as u16;

                        self.panes[0].render_pane(
                            frame,
                            Rect::new(area.x, area.y, area.width, chunk_size + remainder),
                            self.active == 0,
                        );
                        for i in 1..total_panes {
                            let pane_height = if i == total_panes - 1 {
                                chunk_size - remainder
                            } else {
                                chunk_size
                            };
                            let next_area = Rect::new(
                                area.x,
                                area.y + chunk_size * i as u16,
                                area.width,
                                pane_height,
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

    pub fn reshape(&mut self) {
        let (col, row) = size().unwrap();
        let pane_count = self.panes.len();

        log::debug!("screen reshape {row},{col}");
        match self.grid {
            Grid::HORIZONTALE => {
                let chunk_size = row.saturating_div(pane_count as u16);
                let remainder = row % pane_count as u16;

                for (i, term) in self.panes.iter_mut().enumerate() {
                    let pane_height = if i == 0 {
                        chunk_size + remainder
                    } else if i == pane_count - 1 {
                        chunk_size.saturating_sub(remainder)
                    } else {
                        chunk_size
                    };
                    log::debug!("screen reshape hor pane {i}: {pane_height},{col}");
                    term.resize(pane_height, col);
                }
            }
            Grid::SQUIRE => {}
            Grid::GOLDER => {}
            Grid::VERTICAL => {
                let chunk_size = col.saturating_div(pane_count as u16);

                for (i, term) in self.panes.iter_mut().enumerate() {
                    let pane_width = if i == pane_count - 1 {
                        chunk_size
                    } else {
                        chunk_size
                    };
                    log::debug!("screen reshape vor pane {i}: {row},{pane_width}");
                    term.resize(row, pane_width);
                }
            }
        }
    }
}
