//! Nullable saved-format projection. Only declared record edges are traversed.
use serde_json::Value;

#[derive(Clone, Copy)]
enum Shape {
    Record(&'static [(&'static str, Shape)]),
    List(&'static Shape),
    Text,
}
const TEXTS: Shape = Shape::List(&Shape::Text);
const FINDING: Shape = Shape::Record(&[("sources", TEXTS), ("lenses", TEXTS)]);
const FINDINGS: Shape = Shape::List(&FINDING);
const SOURCE: Shape = Shape::Record(&[("lenses", TEXTS)]);
const SOURCES: Shape = Shape::List(&SOURCE);
const STAGES: Shape = Shape::List(&Shape::Record(&[]));
const STATUS: Shape = Shape::Record(&[("degraded", TEXTS), ("agents", SOURCES)]);
const STATS: Shape = Shape::Record(&[("stages", STAGES)]);
// A report, manifest, event or retry record enters at the root. Edges are the
// union of their declared nested fields; no unknown extension value is walked.
const ROOT: Shape = Shape::Record(&[
    ("scope", Shape::Record(&[])),
    ("sources", STATUS),
    ("stats", STATS),
    ("findings", FINDINGS),
    ("open_questions", FINDINGS),
    ("pre_existing", FINDINGS),
    ("immaterial", FINDINGS),
    ("roster", Shape::List(&SOURCE)),
    ("agents", SOURCES),
    ("prompts", Shape::List(&Shape::Record(&[]))),
    ("degraded", TEXTS),
    ("stages", STAGES),
]);

pub(crate) fn project(document: &mut Value) {
    let mut pending = vec![(document, ROOT)];
    while let Some((value, shape)) = pending.pop() {
        if value.is_null() {
            *value = match shape {
                Shape::Text => Value::String(String::new()),
                _ => Value::Object(Default::default()),
            };
        }
        match (value, shape) {
            (Value::Object(fields), Shape::Record(edges)) => {
                fields.retain(|_, value| !value.is_null());
                for (key, child) in fields {
                    if let Some((_, next)) = edges.iter().find(|(name, _)| key == name) {
                        pending.push((child, *next));
                    }
                }
            }
            (Value::Array(values), Shape::List(element)) => {
                pending.extend(values.iter_mut().map(|child| (child, *element)));
            }
            _ => (), // Keep corrupt non-null types for the typed decoder to reject.
        }
    }
}
