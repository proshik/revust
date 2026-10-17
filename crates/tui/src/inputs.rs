//! Presentation of caller-owned input snapshots; no filesystem access.
use crate::{Model, navigation::InputDocument, status::Status, wrap};
impl Status {
    pub fn input_rows(
        &self,
        model: &Model,
        index: usize,
        mut markdown: impl FnMut(&InputDocument, usize) -> Vec<String>,
    ) -> Vec<String> {
        let Some(input) = model.inputs().get(index) else {
            return vec!["no such input".into()];
        };
        let mut rows = vec![crate::status::metadata(&input.path), String::new()];
        match (input.content.is_empty(), input.markdown) {
            (true, _) if input.notice.is_empty() => rows.push("(empty file)".into()),
            (_, true) => rows.extend(crate::markdown::render(
                &input.content,
                model.width(),
                0,
                || markdown(input, model.width()),
            )),
            _ => rows.extend(
                input
                    .content
                    .split('\n')
                    .flat_map(|line| wrap::hardwrap(&expand_tabs(line, 8), model.width())),
            ),
        }
        if !input.notice.is_empty() {
            if !input.content.is_empty() {
                rows.push(String::new());
            }
            rows.extend(wrap::wrap("", &input.notice, model.width()));
        }
        rows
    }
}
pub(crate) fn expand_tabs(line: &str, stop: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let mut result = String::new();
    let mut column = 0;
    for glyph in line.graphemes(true) {
        if glyph == "\t" {
            let padding = stop.max(1) - column % stop.max(1);
            result.extend(std::iter::repeat_n(' ', padding));
            column += padding;
        } else {
            let glyph = wrap::text(glyph);
            column += glyph.width();
            result.push_str(&glyph);
        }
    }
    result
}
