//! Plain, inert terminal text measured in Unicode display cells.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Preserve document structure while making every terminal control inert.
pub fn text(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\t' => result.push_str("    "),
            '\n' => result.push('\n'),
            ch if ch.is_control() => result.push('�'),
            ch => result.push(ch),
        }
    }
    result
}
pub fn width(value: &str) -> usize {
    text(value)
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

pub(crate) fn hardwrap(value: &str, columns: usize) -> Vec<String> {
    let clean = text(value);
    let mut rows = Vec::new();
    for line in clean.split('\n') {
        let mut row = String::new();
        let mut cells = 0;
        for glyph in line.graphemes(true) {
            let size = glyph.width();
            if cells + size > columns.max(1) && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                cells = 0;
            }
            row.push_str(glyph);
            cells += size;
        }
        rows.push(row);
    }
    rows
}

/// Prefix the first line and align continuation lines. Wide graphemes remain
/// intact even when a caller supplies a one-cell viewport.
pub fn wrap(head: &str, body: &str, columns: usize) -> Vec<String> {
    let head = text(head).replace('\n', " ");
    let indent = " ".repeat(width(&head));
    let available = columns.saturating_sub(width(&head)).max(1);
    hardwrap(body, available)
        .into_iter()
        .enumerate()
        .map(|(index, row)| format!("{}{row}", if index == 0 { &head } else { &indent }))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graphemes_and_long_output_survive_wrapping() {
        let source = "界e\u{301}👩‍💻 ".repeat(10_000);
        let rows = wrap("", &source, 23);
        assert_eq!(rows.concat(), source);
        assert!(rows.iter().all(|row| width(row) <= 23));
        assert!(rows.iter().all(|row| !row.starts_with('\u{301}')));
    }
    #[test]
    fn commands_are_inert_and_newlines_keep_empty_rows() {
        assert_eq!(
            wrap("", "a\x1b]52;c;secret\x07\n", 100),
            ["a�]52;c;secret�", ""]
        );
        assert_eq!(wrap("x ", "ab界cd", 6), ["x ab界", "  cd"]);
    }
}
