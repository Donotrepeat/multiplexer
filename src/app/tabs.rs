use crate::app::pane::Pane;
use ratatui::{Frame, layout::Rect};
use strum::{EnumIter, IntoEnumIterator};

use anyhow::Result;
#[derive(EnumIter, Debug, Clone, Copy)]
pub enum Grid {
    Horizontal,
    Vertical,
    Square,
    Golden,
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
    pub fn new(row: u16, coll: u16) -> Result<Self> {
        log::debug!("screen {row},{coll}");
        let panes = vec![Pane::new(row, coll)?];

        Ok(Self {
            panes,
            active: 0,
            grid: Grid::Horizontal,
        })
    }
    fn split_axis(area: Rect, n: u16, vertical: bool) -> Vec<Rect> {
        if n == 0 {
            return Vec::new();
        }
        let mut rects = Vec::with_capacity(n as usize);
        let mut x = area.x;
        let mut y = area.y;
        let mut height = area.height;
        let mut width = area.width;
        let axis = if vertical { area.width } else { area.height };
        let chunk = axis / n;
        let remainder = axis % n;
        for i in 0..n {
            if vertical {
                width = chunk + u16::from(i < remainder);
            } else {
                height = chunk + u16::from(i < remainder);
            }
            rects.push(Rect::new(x, y, width, height));
            if vertical {
                x += width;
            } else {
                y += height
            }
        }
        rects
    }

    fn vertical_rects(area: Rect, n: u16) -> Vec<Rect> {
        Self::split_axis(area, n, true)
    }
    fn horizontal_rects(area: Rect, n: u16) -> Vec<Rect> {
        Self::split_axis(area, n, false)
    }
    fn grid_rects(area: Rect, n: u16) -> Vec<Rect> {
        if n == 0 {
            return Vec::new();
        }
        let mut rects = Vec::with_capacity(n as usize);
        let mut columns = (n as f64).sqrt().ceil().max(1.0) as u16;
        let mut rows = n.div_ceil(columns);
        if n > 1 && rows < 2 {
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

    fn golden_rects(area: Rect, n: u16) -> Vec<Rect> {
        let phi = 0.618;
        let mut regions: Vec<Rect> = Vec::with_capacity(n as usize);
        regions.push(Rect::new(area.x, area.y, area.width, area.height));
        let mut axis_h = false;
        for _k in 1..n {
            let rec = regions[_k as usize - 1];
            if axis_h {
                let kept_h = ((rec.height as f64) * phi).round() as u16;
                let kept_h = kept_h.max(1).min(rec.height.saturating_sub(1));
                regions[_k as usize - 1] = Rect::new(rec.x, rec.y, rec.width, kept_h);
                regions.push(Rect::new(
                    rec.x,
                    rec.y + kept_h,
                    rec.width,
                    rec.height - kept_h,
                ));
            } else {
                let kept_w = ((rec.width as f64) * phi).round() as u16;
                let kept_w = kept_w.max(1).min(rec.width.saturating_sub(1));
                regions[_k as usize - 1] = Rect::new(rec.x, rec.y, kept_w, rec.height);
                regions.push(Rect::new(
                    rec.x + kept_w,
                    rec.y,
                    rec.width - kept_w,
                    rec.height,
                ));
            }
            axis_h = !axis_h;
        }
        regions
    }

    pub fn draw_tab(&mut self, frame: &mut Frame) {
        let total_panes = self.panes.len() as u16;
        if total_panes == 0 {
            return;
        }
        let area = frame.area();
        let rects = match self.grid {
            Grid::Vertical => Self::vertical_rects(area, total_panes),
            Grid::Square => Self::grid_rects(area, total_panes),
            Grid::Golden => Self::golden_rects(area, total_panes),
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

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(2, 3, 80, 24);

    fn rects(grid: Grid, n: u16) -> Vec<Rect> {
        match grid {
            Grid::Horizontal => Tab::horizontal_rects(AREA, n),
            Grid::Vertical => Tab::vertical_rects(AREA, n),
            Grid::Square => Tab::grid_rects(AREA, n),
            Grid::Golden => Tab::golden_rects(AREA, n),
        }
    }

    fn assert_tiled(tile: &[Rect]) {
        for (i, r) in tile.iter().enumerate() {
            assert!(r.width > 0 && r.height > 0, "rect {i} empty: {r:?}");
            assert!(
                r.x >= AREA.x
                    && r.y >= AREA.y
                    && r.right() <= AREA.right()
                    && r.bottom() <= AREA.bottom(),
                "rect {i} escapes area: {r:?}"
            );
        }
        // Exact partition: rects are disjoint and cover the whole area.
        let area: u32 = tile.iter().map(|r| r.width as u32 * r.height as u32).sum();
        assert_eq!(
            area,
            u32::from(AREA.width) * u32::from(AREA.height),
            "rects do not tile the area exactly"
        );
        // Disjointness: no rect starts inside another.
        for (i, a) in tile.iter().enumerate() {
            for (j, b) in tile.iter().enumerate() {
                if i != j {
                    assert!(
                        a.x >= b.right()
                            || b.x >= a.right()
                            || a.y >= b.bottom()
                            || b.y >= a.bottom(),
                        "rects {i} and {j} overlap: {a:?} vs {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn split_zero_panes_is_empty() {
        assert!(rects(Grid::Horizontal, 0).is_empty());
        assert!(rects(Grid::Vertical, 0).is_empty());
        assert!(rects(Grid::Square, 0).is_empty());
    }

    #[test]
    fn horizontal_splits_exact() {
        // 24 / 3 = 8 rem 0: horizontal grid stacks panes vertically.
        assert_eq!(
            rects(Grid::Horizontal, 3),
            vec![
                Rect::new(2, 3, 80, 8),
                Rect::new(2, 11, 80, 8),
                Rect::new(2, 19, 80, 8),
            ]
        );
        // 24 / 4 = 6 rem 0: even split.
        assert_eq!(
            rects(Grid::Horizontal, 4),
            vec![
                Rect::new(2, 3, 80, 6),
                Rect::new(2, 9, 80, 6),
                Rect::new(2, 15, 80, 6),
                Rect::new(2, 21, 80, 6),
            ]
        );
    }
    #[test]
    fn vertical_splits_exact() {
        // 80 / 5 = 16 rem 0: vertical grid splits along the width axis.
        assert_eq!(
            rects(Grid::Vertical, 5),
            vec![
                Rect::new(2, 3, 16, 24),
                Rect::new(18, 3, 16, 24),
                Rect::new(34, 3, 16, 24),
                Rect::new(50, 3, 16, 24),
                Rect::new(66, 3, 16, 24),
            ]
        );
        // 80 / 7 = 11 rem 3.
        assert_eq!(
            rects(Grid::Vertical, 7),
            vec![
                Rect::new(2, 3, 12, 24),
                Rect::new(14, 3, 12, 24),
                Rect::new(26, 3, 12, 24),
                Rect::new(38, 3, 11, 24),
                Rect::new(49, 3, 11, 24),
                Rect::new(60, 3, 11, 24),
                Rect::new(71, 3, 11, 24),
            ]
        );
    }

    #[test]
    fn square_grid_exact() {
        // 5 panes: ceil(sqrt(5)) = 3 columns -> rows = ceil(5/3) = 2,
        // columns rebalanced to ceil(5/2) = 3. Row heights 24/2 = 12.
        // Row 0: 3 cells of width ceil(80/3) = 27, 27, 26.
        // Row 1: 2 cells of width 40, 40.
        assert_eq!(
            rects(Grid::Square, 5),
            vec![
                Rect::new(2, 3, 27, 12),
                Rect::new(29, 3, 27, 12),
                Rect::new(56, 3, 26, 12),
                Rect::new(2, 15, 40, 12),
                Rect::new(42, 15, 40, 12),
            ]
        );
        // 7 panes: columns = ceil(sqrt(7)) = 3 -> rows = ceil(7/3) = 3,
        // columns rebalanced to ceil(7/3) = 3. Heights 24/3 = 8.
        // Row 0 and 1: 3 cells (27, 27, 26); row 2: 1 cell of width 80.
        assert_eq!(
            rects(Grid::Square, 7),
            vec![
                Rect::new(2, 3, 27, 8),
                Rect::new(29, 3, 27, 8),
                Rect::new(56, 3, 26, 8),
                Rect::new(2, 11, 27, 8),
                Rect::new(29, 11, 27, 8),
                Rect::new(56, 11, 26, 8),
                Rect::new(2, 19, 80, 8),
            ]
        );
    }

    #[test]
    fn golden_splits_exact() {
        // Alternating width/height splits at phi = 0.618, rounding to nearest.
        assert_eq!(
            rects(Grid::Golden, 4),
            vec![
                Rect::new(2, 3, 49, 24),  // 80 * 0.618 = 49.44 -> 49
                Rect::new(51, 3, 31, 15), // 31-wide remnant, split on height: 24*0.618 = 14.83 -> 15
                Rect::new(51, 18, 19, 9), // 15-high remnant, split on width: 31*0.618 = 19.16 -> 19
                Rect::new(70, 18, 12, 9), // 19-wide remnant split: 19*0.618 = 11.74 -> 12
            ]
        );
    }

    #[test]
    fn all_grids_fill_area_for_n_1_to_8() {
        for n in 1..=8u16 {
            // Tiling grids must exactly partition the area.
            for grid in [Grid::Horizontal, Grid::Vertical, Grid::Square] {
                let tile = rects(grid, n);
                assert_eq!(tile.len(), n as usize, "{grid:?} n={n}");
                assert_tiled(&tile);
            }
            // Golden rects nest instead of tiling: just require non-empty
            // rects that stay inside the area.
            for r in rects(Grid::Golden, n) {
                assert!(r.width > 0 && r.height > 0);
                assert!(r.x >= AREA.x && r.y >= AREA.y);
                assert!(r.right() <= AREA.right() && r.bottom() <= AREA.bottom());
            }
        }
    }
}
