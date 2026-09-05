use crate::app::pane::Pane;
use ratatui::{Frame, layout::Rect};
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

    fn golden_rects(area: Rect, n: u16) -> Vec<Rect> {
        let phi = 0.618;
        let mut regions: Vec<(u16, u16, u16, u16)> = Vec::with_capacity(n as usize);
        regions.push((area.x, area.y, area.width, area.height));
        let mut axis_h = false;
        for _k in 1..n {
            let (x, y, w, h) = regions[_k as usize - 1];
            if axis_h {
                let kept_h = ((h as f64) * phi).round() as u16;
                let kept_h = kept_h.max(1).min(h.saturating_sub(1));
                regions[_k as usize - 1] = (x, y, w, kept_h);
                regions.push((x, y + kept_h, w, h - kept_h));
            } else {
                let kept_w = ((w as f64) * phi).round() as u16;
                let kept_w = kept_w.max(1).min(w.saturating_sub(1));
                regions[_k as usize - 1] = (x, y, kept_w, h);
                regions.push((x + kept_w, y, w - kept_w, h));
            }
            axis_h = !axis_h;
        }
        regions
            .iter()
            .map(|&(x, y, w, h)| Rect::new(x, y, w, h))
            .collect()
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
            Grid::GOLDER => Self::golden_rects(area, total_panes),
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
                pane.sync_title();
                pane.render_pane(frame, rect, self.active == i);
            }
        }
    }

    pub fn del_pane(&mut self) {
        let new_active = if self.active == self.panes.len() - 1 {
            self.active - 1
        } else {
            self.active
        };

        self.panes.remove(self.active);
        self.active = new_active;
    }
}
