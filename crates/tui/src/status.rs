//! Typed lifecycle and frozen requested-runner presentation.
use crate::{Model, State, navigation::Pane};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};
use revust_core::config::LegacyDuration;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
#[derive(Default)]
pub struct Status {
    pub task: String,
    pub run: String,
    pub profile: String,
    pub colors: bool,
    pub dark: bool,
}
impl Status {
    pub(crate) fn style(&self, light: u8, dark: u8, bold: bool) -> Style {
        if !self.colors {
            return Style::default();
        }
        let color = Color::Indexed(if self.dark { dark } else { light });
        let style = Style::default().fg(color);
        if bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    }
    pub(crate) fn muted(&self) -> Style {
        self.style(247, 242, false)
    }
    fn header(&self, model: &Model) -> Line<'static> {
        let mut fields = vec!["revust".to_owned()];
        if matches!(model.pane(), Pane::Input(_)) {
            fields.push("inputs".into());
            if !self.task.is_empty() && !self.run.is_empty() {
                fields.push(format!("{}/{}", metadata(&self.task), metadata(&self.run)));
            }
        }
        if !self.profile.is_empty() {
            fields.push(metadata(&self.profile));
        }
        if !model.stage.is_empty() {
            fields.push(metadata(&model.stage));
        }
        if model.found.total > 0 {
            fields.push(format!(
                "{} findings ({} critical, {} major, {} minor)",
                model.found.total, model.found.critical, model.found.major, model.found.minor
            ));
        }
        let outcome = if let Some(report) = &model.report {
            let label = if report.exit_code() == 2 {
                "incomplete"
            } else {
                "complete"
            };
            if model.exit_remaining().is_zero() {
                format!("{label} · q to quit")
            } else {
                format!(
                    "{label}, closing in {}s · any key to stay",
                    model.exit_remaining().as_secs().max(1)
                )
            }
        } else {
            "ctrl+c to quit".to_owned()
        };
        // Keep the outcome and tool identity as metadata is progressively omitted.
        while fields.len() > 1
            && format!("{} · {outcome}", fields.join(" · ")).width() > model.width()
        {
            fields.remove(1);
        }
        Line::from(vec![
            Span::styled(fields.join(" · "), self.style(25, 39, true)),
            Span::styled(format!(" · {outcome}"), self.muted()),
        ])
    }
    pub fn render(&self, model: &Model, buffer: &mut Buffer) {
        let area = buffer.area;
        for (index, row) in self
            .status_rows(model)
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
    pub(crate) fn status_rows(&self, model: &Model) -> Vec<Line<'static>> {
        let names = model
            .agents
            .iter()
            .map(|a| metadata(&a.name).width())
            .max()
            .unwrap_or(0)
            .clamp(5, 20);
        let requested = model
            .agents
            .iter()
            .filter_map(|a| model.runner(a))
            .map(|r| metadata(&r.model).width())
            .max()
            .unwrap_or(5)
            .clamp(5, 32);
        let model_cells = requested.min(
            model
                .width()
                .saturating_sub(names + 42 + if model.width() >= 100 { 16 } else { 0 })
                .max(5),
        );
        let rule = Line::styled("─".repeat(model.width()), self.muted());
        let mut rows = vec![
            self.header(model),
            rule.clone(),
            Line::styled(
                format!(
                    "{:<names$}  {:<9}  {:>7}  {:<8}  {:<model_cells$}  {:<6}  ACTIVITY",
                    "AGENT", "STATE", "TIME", "EXECUTOR", "MODEL", "EFFORT"
                ),
                self.muted(),
            ),
        ];
        for agent in &model.agents {
            let (state, style) = match agent.state {
                State::Waiting => ("waiting", self.muted()),
                State::Running => ("running", self.style(25, 39, false)),
                State::Retrying => ("retrying", self.style(130, 214, false)),
                State::Limited => ("limited", self.style(130, 214, false)),
                State::Done => ("done", self.style(28, 42, false)),
                State::Degraded => ("degraded", self.style(124, 203, false)),
            };
            let elapsed = model
                .now
                .and_then(|now| agent.elapsed(now))
                .map(|d| LegacyDuration(d.num_seconds().saturating_mul(1_000_000_000)).to_string())
                .unwrap_or_else(|| "-".into());
            let requested = model.runner(agent);
            let data = [
                (agent.name.as_str(), names),
                (state, 9),
                (&elapsed, 7),
                (requested.map_or("", |r| r.executor.as_str()), 8),
                (requested.map_or("", |r| r.model.as_str()), model_cells),
                (requested.map_or("", |r| r.effort.as_str()), 6),
            ];
            let mut spans = Vec::new();
            for (index, (value, width)) in data.into_iter().enumerate() {
                let row_style = if index == 0 && self.colors {
                    let color = agent
                        .roster
                        .as_ref()
                        .map(|r| r.color.clone())
                        .unwrap_or_else(|| crate::colors::derived(&agent.name));
                    let parsed = color
                        .strip_prefix('#')
                        .filter(|s| s.len() == 6)
                        .and_then(|s| u32::from_str_radix(s, 16).ok())
                        .map(|rgb| Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
                        .or_else(|| {
                            color
                                .parse::<u8>()
                                .ok()
                                .filter(|i| *i < 16)
                                .map(Color::Indexed)
                        })
                        .unwrap_or(Color::Reset);
                    Style::default().fg(parsed)
                } else {
                    style
                };
                spans.push(Span::styled(cell(value, width), row_style));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::raw(metadata(&agent.last)));
            rows.push(Line::from(spans));
        }
        rows.push(rule);
        rows
    }
}
fn cell(value: &str, columns: usize) -> String {
    let clean = metadata(if value.is_empty() { "-" } else { value });
    let clipped = clean.width() > columns;
    let limit = columns.saturating_sub(usize::from(clipped));
    let mut result = String::new();
    let mut used = 0;
    for glyph in clean.graphemes(true) {
        if used + glyph.width() > limit {
            break;
        }
        result.push_str(glyph);
        used += glyph.width();
    }
    if clipped && columns > 0 {
        result.push('…');
        used += 1;
    }
    result.extend(std::iter::repeat_n(' ', columns.saturating_sub(used)));
    result
}
pub(crate) fn metadata(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}
