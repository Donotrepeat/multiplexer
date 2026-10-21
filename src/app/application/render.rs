use crate::app::application::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthChar;

impl App {
    pub(super) fn draw(&mut self, frame: &mut Frame) {
        let (bar_area, content_area) = split_areas(frame.area());
        self.draw_bar(frame, bar_area);
        self.tabs[self.active_tab].draw_tab(frame, content_area);
        if self.show_pop_up {
            self.draw_pup_up(frame);
        }
    }

    fn draw_bar(&self, frame: &mut Frame, area: Rect) {
        let budgets = label_budgets(area.width as usize, self.tabs.len());
        let spans: Vec<Span> = self
            .tabs
            .iter()
            .zip(budgets)
            .enumerate()
            .map(|(i, (tab, budget))| {
                let label = format!("{}:{}", i + 1, tab.panes[tab.active].title);
                let style = if i == self.active_tab {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Span::styled(fit_label(&label, budget), style)
            })
            .collect();

        frame.render_widget(Line::from(spans), area);
    }

    fn draw_pup_up(&self, frame: &mut Frame) {
        let area = frame
            .area()
            .centered(Constraint::Percentage(50), Constraint::Percentage(70));
        frame.render_widget(Clear, area);
        let paragraph = Paragraph::new(self.config.keybindings.lines())
            .block(Block::bordered().title("key bindings"));
        frame.render_widget(paragraph, area);
    }
}

/// Split the screen into the one-row tab bar and the content area below it.
pub(super) fn split_areas(area: Rect) -> (Rect, Rect) {
    let areas = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    (areas[0], areas[1])
}

/// Split the bar's width evenly across `n` tabs. Earlier tabs absorb the
/// remainder, so every column of the bar is assigned to exactly one tab.
pub(super) fn label_budgets(width: usize, n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let base = width / n;
    let extra = width % n;
    (0..n).map(|i| base + usize::from(i < extra)).collect()
}

/// Truncate `s` to `budget` display columns, padding it with spaces so each
/// tab occupies exactly its slot. Wide characters are never split: a glyph
/// that would cross the boundary is dropped and the slot padded instead.
/// Control characters (an OSC title can carry them) are dropped rather than
/// rendered into the bar.
pub(super) fn fit_label(s: &str, budget: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        if c.is_control() {
            continue;
        }
        let width = c.width().unwrap_or(0);
        if used + width > budget {
            break;
        }
        used += width;
        out.push(c);
    }
    out.extend(std::iter::repeat_n(' ', budget - used));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn budgets_cover_the_full_width() {
        assert_eq!(label_budgets(10, 3), vec![4, 3, 3]);
        assert_eq!(label_budgets(9, 3), vec![3, 3, 3]);
        assert_eq!(label_budgets(0, 3), vec![0, 0, 0]);
        assert_eq!(label_budgets(10, 0), Vec::<usize>::new());
    }

    #[test]
    fn fit_label_truncates_and_pads_by_display_width() {
        assert_eq!(fit_label("1:abcdef", 4), "1:ab");
        assert_eq!(fit_label("1:ab", 6), "1:ab  ");
        assert_eq!(fit_label("1:日本語", 6), "1:日本");
        assert_eq!(fit_label("1:日本語", 5), "1:日 ");
        // A control character in an OSC title is dropped, not counted as a
        // zero-width glyph that would render as garbage in the bar.
        assert_eq!(fit_label("1:a\x0bb", 5), "1:ab ");
    }

    #[test]
    fn narrow_bar_keeps_every_tab_visible() {
        let labels = ["1:this title is far too long", "2:a", "3:b", "4:c", "5:d"];
        let fitted: Vec<String> = label_budgets(40, labels.len())
            .into_iter()
            .zip(labels)
            .map(|(budget, label)| fit_label(label, budget))
            .collect();

        assert_eq!(fitted.len(), 5);
        assert!(fitted[0].starts_with("1:this"));
        assert!(fitted.iter().all(|label| !label.trim().is_empty()));
        assert_eq!(fitted.iter().map(|label| label.width()).sum::<usize>(), 40);
    }
}
