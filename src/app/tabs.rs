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

    // Tiles the area into `n` horizontal strips of equal height, distributing
    // the leftover rows evenly to the top strips so the strips fill area with
    // no overlap and no gap.
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

    // Tiles the area into `n` vertical strips of equal width, distributing the
    // leftover columns evenly to the leftmost strips.
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
