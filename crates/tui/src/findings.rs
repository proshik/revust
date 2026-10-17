//! Immutable display inventory with an explicit query editor state.
use revust_core::{report::Report, runtime::data::Finding};
mod render;
pub use render::Part;
pub enum FilterKey<'a> {
    Text(&'a str),
    Backspace,
    Enter,
    Escape,
    Other,
}
enum Editor {
    Browsing(String),
    Editing(String),
}
pub struct Findings {
    rows: Vec<Finding>,
    search: Vec<String>,
    editor: Editor,
    needle: String,
}
impl Findings {
    pub fn new(report: &Report) -> Self {
        let mut buckets: [Vec<Finding>; 4] = Default::default();
        for finding in &report.findings {
            let severity = ["critical", "major", "minor"]
                .iter()
                .position(|label| *label == finding.severity)
                .unwrap_or(3);
            buckets[severity].push(finding.clone());
        }
        let rows: Vec<_> = buckets.into_iter().flatten().collect();
        let search = rows
            .iter()
            .map(|row| {
                [row.title.as_str(), row.file.as_str(), row.body.as_str()]
                    .join(" ")
                    .to_lowercase()
            })
            .collect();
        Self {
            rows,
            search,
            editor: Editor::Browsing(String::new()),
            needle: String::new(),
        }
    }
    /// Identity is independent of a query and never changes saved categories.
    pub fn visible(&self) -> impl Iterator<Item = (usize, &Finding)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(index, _)| self.search[*index].contains(&self.needle))
    }
    pub fn total(&self) -> usize {
        self.rows.len()
    }
    pub fn query(&self) -> &str {
        match &self.editor {
            Editor::Browsing(query) | Editor::Editing(query) => query,
        }
    }
    pub fn typing(&self) -> bool {
        matches!(self.editor, Editor::Editing(_))
    }
    pub fn start_filter(&mut self) {
        self.editor = Editor::Editing(self.query().to_owned());
    }
    /// Interrupts are routed by the model before reaching the editor.
    pub fn edit(&mut self, key: FilterKey<'_>) -> bool {
        let Editor::Editing(query) = &mut self.editor else {
            return false;
        };
        match key {
            FilterKey::Text(text) => query.push_str(text),
            FilterKey::Backspace => {
                query.pop();
            }
            FilterKey::Enter | FilterKey::Escape => {
                let accepted = if matches!(key, FilterKey::Enter) {
                    std::mem::take(query)
                } else {
                    String::new()
                };
                self.editor = Editor::Browsing(accepted);
            }
            FilterKey::Other => {}
        }
        self.needle = self.query().to_lowercase();
        true
    }
}
