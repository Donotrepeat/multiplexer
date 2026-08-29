use crate::app::pane::Pane;
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

    fn horizontal_rects(area: Rect, n: u16) -> Vec<Rect> {
        let mut rects = Vec::with_capacity(n as usize);
        let chunk = area.height / n;
        let remainder = area.height % n;
        let mut y = area.y;
        for i in 0..n {
            let height = chunk + u16::from(i < remainder);
            rects.push(Rect::new(area.x, y, area.width, height));
            y += height;
        }
        rects
    }
    // 2 -> two rows 1 terminal eacy row
    // 3 -> two rows 1-2 terminal on row
    // 4 -> two rows 2 terminal on a row
    // 5 -> two rows 2-3 terminals on a row
    // 9 -> 3 rows 3 terminals

    fn grid_rects(area: Rect, n: u16) -> Vec<Rect> {
        if n == 0 {
            return Vec::new();
        }
        let mut rects = Vec::with_capacity(n as usize);
        let mut columns = (n as f64).sqrt().ceil().max(1.0) as u16;
        let mut rows = n.div_ceil(columns);
        if rows < 2 {
            rows = 2;
        }
        columns = n.div_ceil(rows);
        let row_height = area.height / rows;
        let row_remainder = area.height % rows;
        let mut y = area.y;
        for row in 0..rows {
            let cells = columns.min(n - row * columns);
            let row_height = row_height + u16::from(row < row_remainder);
            let cell_width = area.width / cells;
            let cell_remainder = area.width % cells;
            let mut x = area.x;
            for c in 0..cells {
                let width = cell_width + u16::from(c < cell_remainder);
                rects.push(Rect::new(x, y, width, row_height));
                x += width;
            }
            y += row_height;
        }
        rects
    }

    fn vertical_rects(area: Rect, n: u16) -> Vec<Rect> {
        let mut rects = Vec::with_capacity(n as usize);
        let chunk = area.width / n;
        let remainder = area.width % n;
        let mut x = area.x;
        for i in 0..n {
            let width = chunk + u16::from(i < remainder);
            rects.push(Rect::new(x, area.y, width, area.height));
            x += width;
        }
        rects
    }

    pub fn draw_tab(&mut self, frame: &mut Frame) {
        let total_panes = self.panes.len() as u16;
        if total_panes == 0 {
            return;
        }
        let area = frame.area();
        let rects = match self.grid {
            Grid::VERTICAL => Self::vertical_rects(area, total_panes),
            Grid::SQUIRE => Self::grid_rects(area, total_panes),
            _ => Self::horizontal_rects(area, total_panes),
        };
        // The single source of truth for pane sizing: every pane's virtual
        // terminal is resized to its renderable rect (the block border takes
        // one cell on each side) before it is drawn.
        for (i, pane) in self.panes.iter_mut().enumerate() {
            if let Some(&rect) = rects.get(i) {
                pane.resize(
                    rect.height.saturating_sub(2).max(1),
                    rect.width.saturating_sub(2).max(1),
                );
                pane.render_pane(frame, rect, self.active == i);
            }
        }
    }
}
