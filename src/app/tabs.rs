use crate::app::pane::Pane;

pub enum Grid {
    HORIZONTALE,
    VERTICAL,
    SQUIRE,
    GOLDER,
}

pub struct Tab {
    pub panes: Vec<Pane>,
    pub active: usize,
    pub grid: Grid,
}

impl Tab {
    pub fn new(row: u16, coll: u16) -> Self {
        Self {
            panes: vec![Pane::new(row, coll).unwrap()],
            active: 1,
            grid: Grid::HORIZONTALE,
        }
    }
}
