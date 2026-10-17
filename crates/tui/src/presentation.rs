//! Native Ratatui presentation. No ANSI conversion or terminal-host dependency.
use crate::{Model, navigation::Pane, status::Status};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

#[derive(Default)]
struct Document {
    rows: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
}
impl Document {
    fn text(&mut self, text: &str, style: Style) {
        for (i, line) in clean(text).replace('\t', "    ").split('\n').enumerate() {
            if i > 0 {
                self.flush();
            }
            if !line.is_empty() {
                self.spans.push(Span::styled(line.to_owned(), style));
            }
        }
    }
    fn flush(&mut self) {
        self.rows.push(Line::from(std::mem::take(&mut self.spans)));
    }
    fn block(&mut self) {
        if !self.spans.is_empty() {
            self.flush();
        }
    }
    fn blank(&mut self) {
        self.block();
        if self.rows.last().is_some_and(|r| !r.spans.is_empty()) {
            self.flush();
        }
    }
    fn finish(mut self, width: usize, indent: usize) -> Vec<Line<'static>> {
        self.block();
        wrap(self.rows, width, indent)
    }
}

// Wrap cells directly, keeping each Unicode grapheme and its style together.
fn wrap(rows: Vec<Line<'static>>, width: usize, indent: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let indent = indent.min(width.saturating_sub(1));
    let mut output = Vec::new();
    for row in rows {
        let mut spans = vec![Span::raw(" ".repeat(indent))];
        let mut used = indent;
        for span in row.spans {
            let mut part = String::new();
            for word in span.content.split_word_bounds() {
                if used > indent && used + word.width() > width && word.width() <= width - indent {
                    if !part.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut part), span.style));
                    }
                    output.push(Line::from(std::mem::replace(
                        &mut spans,
                        vec![Span::raw(" ".repeat(indent))],
                    )));
                    used = indent;
                    if word.chars().all(char::is_whitespace) {
                        continue;
                    }
                }
                for glyph in word.graphemes(true) {
                    let n = glyph.width();
                    if used > indent && used + n > width {
                        if !part.is_empty() {
                            spans.push(Span::styled(std::mem::take(&mut part), span.style));
                        }
                        output.push(Line::from(std::mem::replace(
                            &mut spans,
                            vec![Span::raw(" ".repeat(indent))],
                        )));
                        used = indent;
                    }
                    part.push_str(glyph);
                    used += n;
                }
            }
            if !part.is_empty() {
                spans.push(Span::styled(part, span.style));
            }
        }
        output.push(Line::from(spans));
    }
    output
}

impl Status {
    fn document(&self, source: &str, width: usize, indent: usize) -> Vec<Line<'static>> {
        let mut doc = Document::default();
        if source.len() > 64 << 10 {
            doc.text(source, Style::default());
            return doc.finish(width, indent);
        }
        let accent = self.style(25, 39, false);
        let mut style = Style::default();
        let mut stack = Vec::new();
        let mut lists: Vec<Option<u64>> = Vec::new();
        let mut links = Vec::new();
        for event in Parser::new_ext(
            source,
            Options::ENABLE_TABLES
                | Options::ENABLE_STRIKETHROUGH
                | Options::ENABLE_TASKLISTS
                | Options::ENABLE_DEFINITION_LIST,
        ) {
            match event {
                Event::Start(tag) => {
                    stack.push(style);
                    match tag {
                        Tag::Heading { level, .. } => {
                            doc.blank();
                            style = accent.add_modifier(Modifier::BOLD);
                            doc.text(&format!("{} ", "#".repeat(level as usize)), style);
                        }
                        Tag::Paragraph => (),
                        Tag::Strong => style = style.add_modifier(Modifier::BOLD),
                        Tag::Emphasis => style = style.add_modifier(Modifier::ITALIC),
                        Tag::Strikethrough => style = style.add_modifier(Modifier::CROSSED_OUT),
                        Tag::CodeBlock(_) => {
                            doc.blank();
                            style = accent;
                        }
                        Tag::BlockQuote(_) => {
                            doc.block();
                            doc.text("> ", self.muted());
                        }
                        Tag::List(start) => {
                            doc.block();
                            lists.push(start);
                        }
                        Tag::Item => {
                            doc.block();
                            doc.text(&"  ".repeat(lists.len().saturating_sub(1)), style);
                            let bullet = if let Some(Some(n)) = lists.last_mut() {
                                let b = format!("{n}. ");
                                *n += 1;
                                b
                            } else {
                                "• ".into()
                            };
                            doc.text(&bullet, accent);
                        }
                        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                            links.push(dest_url.to_string());
                            style = accent.add_modifier(Modifier::UNDERLINED);
                        }
                        Tag::Table(_) => doc.blank(),
                        Tag::TableRow | Tag::TableHead => doc.block(),
                        Tag::DefinitionListTitle => {
                            doc.block();
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        Tag::DefinitionListDefinition => {
                            doc.block();
                            doc.text("  ", style);
                        }
                        _ => (),
                    }
                }
                Event::End(tag) => {
                    match tag {
                        TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_)
                        | TagEnd::Table => doc.blank(),
                        TagEnd::Item
                        | TagEnd::TableRow
                        | TagEnd::TableHead
                        | TagEnd::DefinitionListDefinition => doc.block(),
                        TagEnd::TableCell => doc.text(" | ", self.muted()),
                        TagEnd::List(_) => {
                            lists.pop();
                            doc.blank();
                        }
                        TagEnd::Link | TagEnd::Image => {
                            if let Some(target) = links.pop() {
                                doc.text(&format!(" ({target})"), self.muted());
                            }
                        }
                        _ => (),
                    }
                    style = stack.pop().unwrap_or_default();
                }
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    doc.text(&text, style)
                }
                Event::Code(text) => doc.text(&text, accent),
                Event::SoftBreak => doc.text(" ", style),
                Event::HardBreak => doc.flush(),
                Event::Rule => {
                    doc.blank();
                    doc.text("────────", self.muted());
                    doc.blank();
                }
                Event::TaskListMarker(done) => doc.text(if done { "[x] " } else { "[ ] " }, accent),
                Event::FootnoteReference(text) => doc.text(&format!("[{text}]"), accent),
                Event::InlineMath(text) | Event::DisplayMath(text) => doc.text(&text, style),
            }
        }
        doc.finish(width, indent)
    }

    /// All detail panes return styled cells, ready for the shared terminal loop.
    pub fn pane(&self, model: &Model, pane: Pane) -> Vec<Line<'static>> {
        let width = model.width();
        let mut doc = Document::default();
        if matches!(pane, Pane::Findings | Pane::Combined)
            && let Some(outcome) = model
                .report
                .as_ref()
                .and_then(|report| report.outcome.as_ref())
            && outcome.incomplete()
        {
            doc.text(&outcome.diagnostic(), self.style(130, 214, false));
            doc.block();
        }
        match pane {
            Pane::Input(index) => {
                let Some(input) = model.inputs().get(index) else {
                    return vec![Line::raw("no such input")];
                };
                doc.text(&input.path, self.muted());
                doc.blank();
                let mut rows = doc.finish(width, 0);
                if input.content.is_empty() && input.notice.is_empty() {
                    rows.push(Line::styled("(empty file)", self.muted()));
                } else if input.markdown {
                    rows.extend(self.document(&input.content, width, 0));
                } else {
                    let mut plain = Document::default();
                    plain.text(&input.content, Style::default());
                    rows.extend(plain.finish(width, 0));
                }
                if !input.notice.is_empty() {
                    let mut notice = Document::default();
                    notice.text(&input.notice, self.style(130, 214, false));
                    rows.extend(notice.finish(width, 0));
                }
                rows
            }
            Pane::Findings => {
                let Some(findings) = &model.findings else {
                    return vec![Line::raw("review is running...")];
                };
                if findings.typing() {
                    doc.text(&format!("filter: {}_", findings.query()), self.muted());
                    doc.block();
                } else if !findings.query().is_empty() {
                    doc.text(&format!("filter: {}", findings.query()), self.muted());
                    doc.block();
                }
                let mut rows = doc.finish(width, 0);
                let mut previous = None;
                for (_, finding) in findings.visible() {
                    let mut heading = Document::default();
                    if previous != Some(&finding.severity) {
                        heading.text(
                            &revust_core::report::heading(&finding.severity),
                            self.style(130, 214, true),
                        );
                        heading.block();
                        previous = Some(&finding.severity);
                    }
                    heading.text(
                        &format!("{} [{}]", finding.title, finding.confidence),
                        self.style(25, 39, true),
                    );
                    heading.block();
                    heading.text(&revust_core::report::location(finding), self.muted());
                    heading.blank();
                    rows.extend(heading.finish(width, 0));
                    rows.extend(self.document(&finding.body, width, 2));
                    if !finding.fix.is_empty() {
                        rows.extend(self.document(&format!("fix:\n{}", finding.fix), width, 2));
                    }
                    let meta = [
                        (!finding.sources.is_empty())
                            .then(|| format!("sources: {}", finding.sources.join(", "))),
                        (!finding.lenses.is_empty())
                            .then(|| format!("lenses: {}", finding.lenses.join(", "))),
                        (!finding.verdict.is_empty())
                            .then(|| format!("verdict: {}", finding.verdict)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" | ");
                    let mut footer = Document::default();
                    footer.text(&meta, self.muted());
                    footer.blank();
                    rows.extend(footer.finish(width, 2));
                }
                if previous.is_none() {
                    rows.push(Line::raw(if findings.total() == 0 {
                        "no findings."
                    } else {
                        "nothing matches the filter."
                    }));
                }
                rows
            }
            Pane::Combined | Pane::Agent(_) => {
                let entries = if let Pane::Agent(i) = pane {
                    let Some(agent) = model.agents.get(i) else {
                        return vec![Line::raw("no such agent")];
                    };
                    &agent.lines
                } else {
                    &model.combined
                };
                for entry in entries {
                    doc.text(
                        &format!(
                            "{} ",
                            entry.at.with_timezone(&chrono::Local).format("%H:%M:%S")
                        ),
                        self.muted(),
                    );
                    if pane == Pane::Combined && !entry.agent.is_empty() {
                        doc.text(&format!("{}  ", entry.agent), self.style(25, 39, true));
                    }
                    doc.text(&entry.text, Style::default());
                    doc.block();
                }
                if entries.is_empty() && model.report.is_none() {
                    doc.text("waiting for the first agent...", self.muted());
                }
                doc.finish(width, 0)
            }
        }
    }
}
