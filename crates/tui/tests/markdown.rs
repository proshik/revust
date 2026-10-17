use revust_tui::{Model, markdown, navigation::InputDocument, status::Status};
#[test]
fn oversized_and_zero_content_width_skip_parser_and_keep_literal_evidence() {
    let source = format!("{}\n**tail**\t`code`\n", "é".repeat(32768));
    let rows = markdown::render(&source, 70000, 0, || panic!("oversized parser dispatch"));
    assert_eq!(
        rows.concat(),
        source.replace('\n', "").replace('\t', "    ")
    );
    assert!(rows.last().unwrap().is_empty());
    assert_eq!(
        markdown::render(&"x".repeat(65536), 80, 2, || vec!["parsed".into()]),
        ["parsed"]
    );
    let rows = markdown::render("ab\tc", 2, 2, || panic!("no content cells"));
    assert_eq!(rows.concat().replace(' ', ""), "abc");
    assert!(rows.iter().all(|row| row.starts_with("  ")));
}
#[test]
fn document_callback_is_bounded_and_controls_cannot_enter_plain_rows() {
    assert_eq!(
        markdown::render("small", 80, 0, || vec!["a\x1b[2J\nb".into()]),
        ["a�[2J", "b"]
    );
    let source = "a\n".repeat(32769);
    let model = Model::new(vec![]).with_inputs(vec![InputDocument {
        content: source,
        markdown: true,
        ..Default::default()
    }]);
    let rows = Status::default().input_rows(&model, 0, |_, _| panic!("oversized parser dispatch"));
    assert_eq!(rows.iter().filter(|row| *row == "a").count(), 32769);
    assert!(rows.last().unwrap().is_empty());
}
