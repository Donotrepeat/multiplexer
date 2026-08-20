use crate::app::pane::Pane;
use crossterm::terminal::size;
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
