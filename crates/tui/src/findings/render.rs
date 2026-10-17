//! Plain finding documents; native styled rendering lives in presentation.
use super::Findings;
use crate::{markdown, wrap};
use revust_core::report;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Body,
    Fix,
}
impl Findings {
    /// Callback identities refer to the stable severity-ordered observation.
    pub fn rows(
        &self,
        width: usize,
        mut document: impl FnMut(usize, Part, &str, usize, usize) -> Vec<String>,
    ) -> Vec<String> {
        let mut output = Vec::new();
        if self.typing() || !self.query().is_empty() {
            output.push(wrap::text(&format!(
                "filter: {}{}",
                self.query(),
                if self.typing() {
                    "_".into()
                } else {
                    format!(" ({} of {})", self.visible().count(), self.rows.len())
                }
            )));
        }
        if self.visible().next().is_none() {
            output.push(if self.rows.is_empty() {
                "no findings.".into()
            } else {
                format!("nothing matches {:?}.", self.query())
            });
            return output;
        }
        let mut severity = None;
        for (identity, finding) in self.visible() {
            if severity != Some(&finding.severity) {
                output.extend(wrap::wrap(
                    "",
                    &format!("## {}", report::heading(&finding.severity)),
                    width,
                ));
                severity = Some(&finding.severity);
            }
            output.extend(wrap::wrap(
                "",
                &format!("### {}  [{}]", finding.title, finding.confidence),
                width,
            ));
            output.extend(wrap::wrap("    ", &report::location(finding), width));
            for (part, source) in [
                (Part::Body, finding.body.clone()),
                (
                    Part::Fix,
                    if finding.fix.is_empty() {
                        String::new()
                    } else {
                        format!("fix:\n{}", finding.fix)
                    },
                ),
            ] {
                if source.is_empty() {
                    continue;
                }
                output.push(String::new());
                output.extend(markdown::render(&source, width, 2, || {
                    document(identity, part, &source, width, 2)
                }));
            }
            let metadata = [
                (!finding.sources.is_empty())
                    .then(|| format!("sources: {}", finding.sources.join(", "))),
                (!finding.lenses.is_empty())
                    .then(|| format!("lenses: {}", finding.lenses.join(", "))),
                (!finding.verdict.is_empty()).then(|| format!("verdict: {}", finding.verdict)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if !metadata.is_empty() {
                output.push(String::new());
                output.extend(wrap::wrap("    ", &metadata.join(" | "), width));
            }
            output.push(String::new());
        }
        output
    }
}
