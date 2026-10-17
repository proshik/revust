use ratatui::style::Modifier;
use revust_core::{report::Report, runtime::data::Finding};
use revust_tui::{
    Model,
    navigation::{InputDocument, Pane},
    status::Status,
};

#[test]
fn markdown_is_rendered_as_styled_cells_with_readable_blocks_and_links() {
    let model = Model::new(vec![]).with_inputs(vec![InputDocument {
        label: "scope".into(), path: "scope.md".into(), markdown: true, notice: String::new(),
        content: "# Heading\n\n**bold** and `code`\n\n- [x] done\n\n- next\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n\n[link](https://example.invalid)\n".into(),
    }]);
    let rows = Status {
        colors: true,
        ..Default::default()
    }
    .pane(&model, Pane::Input(0));
    let text = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Heading",
        "bold",
        "code",
        "[x]",
        "done",
        "A",
        "B",
        "let x = 1;",
        "https://example.invalid",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains('\x1b'));
    assert!(rows.iter().any(|r| {
        let text = r.to_string();
        text.contains('•') && text.contains("done")
    }));
    assert!(
        rows.iter()
            .flat_map(|r| &r.spans)
            .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD))
    );
}

#[test]
fn panes_preserve_unicode_and_filtered_findings_without_ansi_payloads() {
    let mut model = Model::new(vec![]);
    model.complete(revust_core::report::Report {
        findings: serde_json::from_value(serde_json::json!([
            {"title":"keep", "severity":"major", "body":"中文 👩‍💻 é", "fix":"use `new()`"},
            {"title":"hide", "body":"hidden text"}
        ]))
        .unwrap(),
        ..Default::default()
    });
    let findings = model.findings.as_mut().unwrap();
    findings.start_filter();
    findings.edit(revust_tui::findings::FilterKey::Text("keep"));
    model.resize(24, 20, |_, _| 0);
    let rows = Status::default().pane(&model, Pane::Findings);
    let text = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("keep"));
    assert!(!text.contains("hidden text"));
    assert!(text.contains("👩‍💻"));
    assert!(text.contains("fix:"));
    assert!(!text.contains('\x1b'));
    assert!(rows.iter().all(|r| r.width() <= 24));
}

#[test]
fn legacy_plain_boundaries_neutralize_terminal_commands_without_mutating_inputs() {
    use revust_tui::{findings::Findings, navigation::InputDocument};
    let hostile = "title\x1b]52;c;secret\x07\r\u{009b}2J";
    let model = Model::new(vec![]).with_inputs(vec![InputDocument {
        path: hostile.into(),
        content: hostile.into(),
        markdown: true,
        ..Default::default()
    }]);
    let status = Status::default();
    let rows = status.input_rows(&model, 0, |_, _| vec![hostile.into()]);
    assert!(rows.iter().all(|r| !r.chars().any(|c| c.is_control())));
    assert_eq!(model.inputs()[0].content, hostile);
    let report = Report {
        findings: vec![Finding {
            title: hostile.into(),
            body: "body".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let rows = Findings::new(&report).rows(80, |_, _, _, _, _| vec![hostile.into()]);
    assert!(rows.iter().all(|r| !r.chars().any(|c| c.is_control())));
    assert_eq!(report.findings[0].title, hostile);
}
