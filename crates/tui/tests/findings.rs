use revust_core::{report::Report, runtime::data::Finding};
use revust_tui::{Model, findings::FilterKey};

fn finding(title: &str, severity: &str) -> Finding {
    Finding {
        title: title.into(),
        severity: severity.into(),
        ..Finding::default()
    }
}

#[test]
fn rendered_details_keep_separate_documents_and_stable_cache_keys_after_filtering() {
    use revust_tui::findings::Part;
    let report = Report {
        findings: vec![
            finding("minor", "minor"),
            Finding {
                file: "src/main.rs".into(),
                line: 3,
                end_line: 5,
                confidence: 92,
                body: "```rust\nunclosed".into(),
                fix: "- independent fix".into(),
                sources: vec!["a".into(), "b".into()],
                lenses: vec!["bugs".into()],
                verdict: "confirmed".into(),
                ..finding("**bold** `code`", "major")
            },
        ],
        ..Report::default()
    };
    let mut browser = revust_tui::findings::Findings::new(&report);
    let mut calls = Vec::new();
    let rows = browser.rows(100, |index, part, text, width, indent| {
        calls.push((index, part, text.to_owned(), width, indent));
        vec![format!("document:{index}:{part:?}")]
    });
    let text = rows.join("\n");
    for detail in [
        "## Major",
        "[92]",
        "src/main.rs:3-5",
        "document:0:Body",
        "document:0:Fix",
        "sources: a, b",
        "lenses: bugs",
        "verdict: confirmed",
    ] {
        assert!(text.contains(detail), "{text}");
    }
    assert!(text.find("Major").unwrap() < text.find("Minor").unwrap());
    assert_eq!(
        calls,
        [
            (0, Part::Body, "```rust\nunclosed".into(), 100, 2),
            (0, Part::Fix, "fix:\n- independent fix".into(), 100, 2)
        ]
    );
    browser.start_filter();
    browser.edit(FilterKey::Text("bold"));
    browser.edit(FilterKey::Enter);
    let mut keys = Vec::new();
    let rows = browser.rows(25, |index, part, _, _, _| {
        keys.push((index, part));
        vec![]
    });
    assert_eq!(rows[0], "filter: bold (1 of 2)");
    assert_eq!(keys, [(0, Part::Body), (0, Part::Fix)]);
    assert!(rows.iter().all(|row| !row.contains('\x1b')));
}

#[test]
fn unmatched_filter_is_inert_without_changing_the_search_text() {
    let mut browser = revust_tui::findings::Findings::new(&Report {
        findings: vec![finding("present", "major")],
        ..Report::default()
    });
    let query = "\x1b\u{a0}e\u{301}👩\u{200d}💻";
    browser.start_filter();
    browser.edit(FilterKey::Text(query));
    browser.edit(FilterKey::Enter);
    let rows = browser.rows(80, |_, _, _, _, _| panic!("no matching documents"));
    assert_eq!(browser.query(), query);
    assert!(rows[1].starts_with("nothing matches "));
    assert!(rows.iter().all(|row| !row.contains('\x1b')));
}

#[test]
fn completion_opens_stable_severity_order_without_changing_saved_categories() {
    let report = Report {
        findings: vec![
            finding("unknown", "oops"),
            finding("minor", "minor"),
            finding("major first", "major"),
            finding("critical", "critical"),
            finding("major second", "major"),
            finding("empty", ""),
        ],
        open_questions: vec![finding("question", "critical")],
        pre_existing: vec![finding("existing", "critical")],
        immaterial: vec![finding("immaterial", "critical")],
        ..Report::default()
    };
    let original = report.findings.clone();
    let mut model = Model::new(vec![]);
    assert!(model.findings.is_none());
    model.complete(report.clone());
    let browser = model.findings.as_ref().unwrap();
    assert_eq!(
        browser
            .visible()
            .map(|(_, f)| f.title.as_str())
            .collect::<Vec<_>>(),
        [
            "critical",
            "major first",
            "major second",
            "minor",
            "unknown",
            "empty"
        ]
    );
    assert_eq!(model.report.as_ref().unwrap().findings, original);
    assert_eq!(
        model.report.as_ref().unwrap().open_questions,
        report.open_questions
    );
    assert_eq!(
        model.report.as_ref().unwrap().pre_existing,
        report.pre_existing
    );
    assert_eq!(model.report.as_ref().unwrap().immaterial, report.immaterial);
    let archive = Model::from_archive(vec![], [], report).unwrap();
    assert_eq!(archive.findings.unwrap().visible().count(), 6);
}

#[test]
fn filter_searches_only_title_file_body_and_preserves_row_identity() {
    let report = Report {
        findings: vec![
            Finding {
                file: "src/ФАЙЛ.rs".into(),
                ..finding("İ ΟΣ", "minor")
            },
            Finding {
                body: "BODY needle".into(),
                ..finding("second", "major")
            },
            Finding {
                fix: "needle".into(),
                sources: vec!["needle".into()],
                lenses: vec!["needle".into()],
                verdict: "needle".into(),
                ..finding("third", "critical")
            },
        ],
        ..Report::default()
    };
    let mut model = Model::new(vec![]);
    model.complete(report);
    let browser = model.findings.as_mut().unwrap();
    for (query, index, title) in [
        ("NEEDLE", 1, "second"),
        ("файл", 2, "İ ΟΣ"),
        ("İ ΟΣ", 2, "İ ΟΣ"),
        ("second  BODY", 1, "second"),
    ] {
        browser.start_filter();
        browser.edit(FilterKey::Text(query));
        assert_eq!(
            browser
                .visible()
                .map(|(i, f)| (i, f.title.as_str()))
                .collect::<Vec<_>>(),
            [(index, title)]
        );
        browser.edit(FilterKey::Escape);
    }
    browser.start_filter();
    browser.edit(FilterKey::Text("absent"));
    assert_eq!(browser.visible().count(), 0);
    assert_eq!(browser.total(), 3);
    browser.edit(FilterKey::Escape);
    assert_eq!(browser.visible().count(), 3);
}

#[test]
fn filter_editor_accepts_navigation_letters_and_deletes_one_unicode_scalar() {
    let mut model = Model::new(vec![]);
    model.complete(Report::default());
    let browser = model.findings.as_mut().unwrap();
    assert!(!browser.edit(FilterKey::Text("q")));
    browser.start_filter();
    assert!(browser.edit(FilterKey::Text("q j / 👩‍💻e\u{301}")));
    browser.edit(FilterKey::Backspace);
    assert_eq!(browser.query(), "q j / 👩‍💻e");
    browser.edit(FilterKey::Backspace);
    browser.edit(FilterKey::Backspace);
    assert_eq!(browser.query(), "q j / 👩‍");
    assert!(browser.edit(FilterKey::Other));
    assert_eq!(browser.query(), "q j / 👩‍");
    browser.edit(FilterKey::Enter);
    assert!(!browser.typing());
    assert!(!browser.edit(FilterKey::Escape));
    assert_eq!(browser.query(), "q j / 👩‍");
    browser.start_filter();
    browser.edit(FilterKey::Escape);
    assert_eq!(browser.query(), "");
    assert!(!browser.typing());
    browser.start_filter();
    assert!(browser.edit(FilterKey::Backspace));
    assert_eq!(browser.total(), 0);
}
