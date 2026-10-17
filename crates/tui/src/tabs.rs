//! Responsive tab labels built from the selectable pane inventory.
use crate::{
    Model,
    navigation::{Pane, tab_token},
    status::{Status, metadata},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;
impl Status {
    pub fn render_tabs(&self, model: &Model, buffer: &mut Buffer) {
        let area = buffer.area;
        for (index, row) in self
            .tab_rows(model)
            .into_iter()
            .take(area.height as usize)
            .enumerate()
        {
            row.render(
                Rect::new(area.x, area.y + index as u16, area.width, 1),
                buffer,
            );
        }
    }
    pub(crate) fn tab_rows(&self, model: &Model) -> [Line<'static>; 2] {
        let focused = model.pane();
        let mut tabs = Vec::new();
        if matches!(focused, Pane::Input(_)) {
            tabs.extend(model.inputs().iter().enumerate().map(|(index, input)| {
                (
                    Pane::Input(index),
                    format!("{} {}", token(index), metadata(&input.label)),
                )
            }));
        } else {
            tabs.push((Pane::Combined, "1 all".into()));
            tabs.extend(model.agents.iter().enumerate().map(|(index, agent)| {
                (
                    Pane::Agent(index),
                    format!("{} {}", token(index + 1), metadata(&agent.name)),
                )
            }));
            if model.findings.is_some() {
                tabs.push((Pane::Findings, "f findings".into()));
            }
        }
        let selected = tabs
            .iter()
            .position(|(pane, _)| *pane == focused)
            .unwrap_or(0);
        let mut left = selected;
        let mut right = selected + 1;
        // Grow a window around the selected label rather than compacting unrelated labels.
        let measure = |start: usize, end: usize| {
            tabs[start..end]
                .iter()
                .map(|(_, label)| label.width() + 5)
                .sum::<usize>()
        };
        loop {
            let before = (left > 0).then(|| left - 1);
            let after = (right < tabs.len()).then_some(right + 1);
            if let Some(start) = before.filter(|start| measure(*start, right) <= model.width()) {
                left = start;
            } else if let Some(end) = after.filter(|end| measure(left, *end) <= model.width()) {
                right = end;
            } else {
                break;
            }
        }
        let mut spans = Vec::new();
        if left > 0 {
            spans.push(Span::styled("… ", self.muted()));
        }
        for (pane, label) in &tabs[left..right] {
            let active = *pane == focused;
            spans.push(Span::styled(
                format!("{}{}  ", if active { "▸ " } else { "" }, label),
                if active {
                    self.style(25, 39, true)
                } else {
                    self.muted()
                },
            ));
        }
        if right < tabs.len() {
            spans.push(Span::styled("… ", self.muted()));
        }
        if !model.inputs().is_empty() {
            spans.push(Span::styled(
                if matches!(focused, Pane::Input(_)) {
                    "│ i back"
                } else {
                    "│ i inputs"
                },
                self.muted(),
            ));
        }
        [
            Line::from(spans),
            Line::styled("─".repeat(model.width()), self.muted()),
        ]
    }
}
fn token(index: usize) -> char {
    tab_token(index).unwrap_or(' ')
}
