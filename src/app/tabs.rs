use crate::app::pane::Pane;
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
    pub fn reshape(&mut self, new_shape: Grid) {
        match new_shape {
            Grid::HORIZONTALE => {}
            Grid::SQUIRE => {}
            Grid::GOLDER => {}
            Grid::VERTICAL => {}
        }
        self.grid = new_shape;
    }
}
