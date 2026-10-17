//! Single-measure frame assembly into caller-owned cells.
use crate::{Model, navigation::Pane, status::Status};
use ratatui::{buffer::Buffer, layout::Rect, text::Line, widgets::Widget};
impl Status {
    pub fn frame(
        &self,
        model: &mut Model,
        buffer: &mut Buffer,
        mut pane: impl FnMut(&Model, Pane) -> Vec<Line<'static>>,
    ) {
        let area = buffer.area;
        buffer.reset();
        if area.is_empty() {
            return;
        }
        let mut content = Vec::new();
        model.resize(area.width as usize, area.height as usize, |m, p| {
            content = pane(m, p);
            content.len()
        });
        let selection = model.viewport(|_, _| content.len());
        let mut screen = self.status_rows(model);
        screen.extend(self.tab_rows(model));
        let padding = model.pane_height().saturating_sub(selection.len());
        screen.extend(
            content
                .into_iter()
                .skip(selection.start)
                .take(selection.len()),
        );
        screen.extend(std::iter::repeat_n(Line::default(), padding));
        let start = screen.len().saturating_sub(area.height as usize);
        for (y, line) in screen.into_iter().skip(start).enumerate() {
            // A callback may be provided by another caller. Preserve styles but
            // ensure it cannot inject active terminal controls through cell text.
            let safe = Line::from(
                line.spans
                    .into_iter()
                    .map(|span| {
                        ratatui::text::Span::styled(
                            crate::status::metadata(&span.content),
                            span.style,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .style(line.style);
            safe.render(Rect::new(area.x, area.y + y as u16, area.width, 1), buffer);
        }
    }
}
