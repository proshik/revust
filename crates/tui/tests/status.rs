use ratatui::{buffer::Buffer, layout::Rect};
use revust_core::{
    report::Report,
    runtime::events::{Event, Kind},
};
use revust_tui::{
    Model,
    navigation::{InputDocument, Key, Pane},
    status::Status,
};
fn screen(status: &Status, model: &mut Model, width: u16) -> String {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, 15));
    status.frame(model, &mut buffer, |m, p| status.pane(m, p));
    (0..15)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn widget_distinguishes_partial_agent_state_from_durable_completion_and_cleans_metadata() {
    let mut model = Model::new(vec![]);
    let mut event = Event::agent(Kind::AgentDegraded, "derived", "partial\x1b[2J");
    event.at = "2026-10-10T12:00:00Z".into();
    model.apply(event).unwrap();
    let status = Status {
        profile: "profile\x1b]52;c;x\x07".into(),
        ..Default::default()
    };
    let text = screen(&status, &mut model, 160);
    assert!(text.contains("degraded"));
    assert!(text.contains("partial"));
    assert!(text.contains("ctrl+c to quit"));
    assert!(!text.contains("complete"));
    assert!(!text.contains('\x1b'));
    assert!(!text.contains('\x07'));
    model.complete(Report::default());
    let text = screen(&status, &mut model, 160);
    assert!(text.contains("complete"));
    assert!(text.contains("q to quit"));
    assert!(text.contains("no findings."));
}
#[test]
fn selected_tab_remains_identifiable_and_input_documents_keep_whitespace_and_notice() {
    let inputs = (0..35)
        .map(|index| InputDocument {
            label: format!("input-{index}"),
            content: "界\tX\n  space\n".into(),
            notice: "truncated\x07".into(),
            ..Default::default()
        })
        .collect();
    let mut model = Model::new(vec![]).with_inputs(inputs);
    model.key(Key::Text("i"), |_, _| 0);
    for _ in 0..34 {
        model.key(Key::Right, |_, _| 0);
    }
    assert_eq!(model.pane(), Pane::Input(34));
    let status = Status::default();
    let text = screen(&status, &mut model, 40);
    assert!(text.contains("input-34"));
    assert!(text.contains("i back"));
    let rows = status.input_rows(&model, 34, |_, _| panic!("plain input"));
    assert!(rows.contains(&"界      X".into()));
    assert!(rows.contains(&"  space".into()));
    assert!(rows.iter().any(|row| row.contains("truncated�")));
    assert_eq!(
        status.input_rows(&model, 99, |_, _| unreachable!()),
        ["no such input"]
    );
}
#[test]
fn plain_logs_and_color_names_cannot_execute_controls() {
    let mut model = Model::new(vec![]);
    let mut event = Event::agent(Kind::AgentActivity, "untrusted\x1b[2J", "body\u{009b}2J");
    event.at = "2026-10-10T12:00:00Z".into();
    model.apply(event).unwrap();
    let rows = Status::default().log_rows(&model, Pane::Combined, |_| "time\x07".into());
    assert!(rows.iter().all(|row| !row.chars().any(|c| c.is_control())));
    assert_eq!(
        revust_tui::colors::paint("#000000\x1b", "name\x1b"),
        "name�"
    );
    assert!(revust_tui::colors::sequence("#123456").is_some());
    assert!(revust_tui::colors::sequence("#nothex").is_none());
}

#[test]
fn incomplete_reports_honor_auto_exit_and_remain_inspectable_after_a_key() {
    for state in [
        serde_json::json!("failed"),
        serde_json::json!({"stopped":"cancelled"}),
        serde_json::json!("complete"),
    ] {
        let report = Report::decode(
            &serde_json::json!({"outcome":{"state":state,"policy_failures":["synthetic failure"]}})
                .to_string(),
        )
        .unwrap();
        assert_eq!(report.exit_code(), 2);
        for seconds in [0, 1] {
            let mut model =
                Model::new(vec![]).with_auto_exit(std::time::Duration::from_secs(seconds));
            model.complete(report.clone());
            let text = screen(&Status::default(), &mut model, 160);
            assert!(text.contains("synthetic failure"), "{text}");
            assert!(!text.contains("· complete"), "{text}");
            if seconds == 0 {
                assert!(text.contains("incomplete · q to quit"), "{text}");
                assert_eq!(model.tick(), revust_tui::navigation::Action::None);
            } else {
                assert!(
                    text.contains("incomplete, closing in 1s · any key to stay"),
                    "{text}"
                );
                model.key(Key::Other, |_, _| 1);
                assert_eq!(model.tick(), revust_tui::navigation::Action::None);
                let text = screen(&Status::default(), &mut model, 160);
                assert!(text.contains("incomplete · q to quit"), "{text}");
                assert!(text.contains("synthetic failure"), "{text}");
            }
            assert!(model.exit_remaining().is_zero());
        }
    }
}
