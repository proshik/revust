use revust_core::{
    report::Report,
    runtime::{
        data::Finding,
        events::{Event, Kind},
    },
};
use revust_tui::{
    Model,
    navigation::{Action, InputDocument, Key, Pane, tab_token},
};

// Controlled renderer: lengths deliberately differ between panes and filtering
// changes the report's height. Terminal rendering has its own qualification gate.
fn lines(model: &Model, pane: Pane) -> usize {
    match pane {
        Pane::Combined => 100,
        Pane::Agent(_) => 80,
        Pane::Findings => model.findings.as_ref().unwrap().visible().count() * 40,
        Pane::Input(index) => 120 + index * 10,
    }
}
fn model(agents: usize) -> Model {
    let mut model = Model::new(vec![]).with_inputs(vec![InputDocument::default(); 2]);
    for index in 0..agents {
        let mut event = Event::agent(Kind::AgentStarted, &format!("agent-{index}"), "");
        event.at = "2026-10-10T00:00:00Z".into();
        model.apply(event).unwrap();
    }
    model
}
fn report() -> Report {
    Report {
        findings: vec![
            Finding {
                title: "q j".into(),
                ..Finding::default()
            },
            Finding {
                title: "second".into(),
                ..Finding::default()
            },
        ],
        ..Report::default()
    }
}

#[test]
fn tabs_do_not_wrap_and_panes_beyond_tokens_remain_reachable() {
    let mut m = model(30);
    assert_eq!(m.key(Key::Text("q"), lines), Action::None);
    m.key(Key::Left, lines);
    assert_eq!(m.pane(), Pane::Combined);
    for (index, token) in "123456789abcdemnoprstuvwxyz".chars().enumerate() {
        assert_eq!(tab_token(index), Some(token));
        m.key(Key::Text(&token.to_string()), lines);
        assert_eq!(
            m.pane(),
            if index == 0 {
                Pane::Combined
            } else {
                Pane::Agent(index - 1)
            }
        );
    }
    assert_eq!(tab_token(27), None);
    for _ in 0..10 {
        m.key(Key::Tab, lines);
    }
    assert_eq!(m.pane(), Pane::Agent(29));
    for key in [Key::Right, Key::Text("l"), Key::Text("0"), Key::Text("zz")] {
        m.key(key, lines);
    }
    assert_eq!(m.pane(), Pane::Agent(29));
    for key in [Key::BackTab, Key::Left, Key::Text("h")] {
        m.key(key, lines);
    }
    assert_eq!(m.pane(), Pane::Agent(26));
    m.key(Key::Text("f"), lines);
    assert_eq!(m.pane(), Pane::Agent(26));
    m.complete(report());
    assert_eq!(m.pane(), Pane::Findings);
    assert_eq!(m.key(Key::Text("q"), lines), Action::Quit);
    assert_eq!(m.key(Key::Escape, lines), Action::None);
}

#[test]
fn scrolling_uses_rendered_rows_and_clamps_after_resize() {
    let mut m = model(2);
    assert_eq!(m.viewport(lines), 78..100); // 30 - 6 - 2 = 22 rows
    for key in [Key::Up, Key::Text("k")] {
        m.key(key, lines);
    }
    assert_eq!(m.viewport(lines), 76..98);
    for key in [Key::PageUp, Key::CtrlB] {
        m.key(key, lines);
    }
    assert_eq!(m.viewport(lines), 32..54);
    for key in [Key::PageDown, Key::CtrlF, Key::Down, Key::Text("j")] {
        m.key(key, lines);
    }
    assert_eq!(m.viewport(lines), 78..100);
    m.key(Key::Home, lines);
    assert_eq!(m.viewport(lines), 0..22);
    m.resize(12, 10, lines);
    assert_eq!(m.viewport(lines), 20..22); // preserve bottom-relative offset
    m.key(Key::Text("g"), lines);
    assert_eq!(m.viewport(lines), 0..2);
    m.resize(1, 1, lines);
    assert_eq!(m.pane_height(), 1);
    m.key(Key::Text("G"), lines);
    assert_eq!(m.viewport(lines), 99..100);
    m.resize(0, 0, lines);
    assert_eq!(m.width(), 100);
    assert_eq!(m.pane_height(), 22);
    m.resize(200, 200, lines);
    assert_eq!(m.viewport(lines), 0..100);
    m.key(Key::End, lines);
    m.key(Key::Text("2"), lines);
    assert_eq!(m.viewport(lines), 0..80);
}

#[test]
fn inputs_preserve_positions_and_completion_redirects_only_on_return() {
    let mut m = model(1);
    m.key(Key::Up, lines);
    let review = m.viewport(lines);
    m.key(Key::Text("i"), lines);
    assert_eq!(m.pane(), Pane::Input(0));
    assert_eq!(m.viewport(lines), 0..23);
    m.key(Key::Right, lines);
    m.key(Key::Down, lines);
    assert_eq!(m.viewport(lines), 1..24);
    m.key(Key::Text("f"), lines);
    assert_eq!(m.pane(), Pane::Input(1));
    m.key(Key::Escape, lines);
    assert_eq!(m.pane(), Pane::Combined);
    assert_eq!(m.viewport(lines), review);
    m.key(Key::Text("i"), lines);
    assert_eq!(m.pane(), Pane::Input(1));
    assert_eq!(m.viewport(lines), 1..24);
    m.complete(report());
    assert_eq!(m.pane(), Pane::Input(1));
    m.key(Key::Text("i"), lines);
    assert_eq!(m.pane(), Pane::Findings);
    assert_eq!(m.viewport(lines), 0..23);
    m.key(Key::Down, lines);
    m.key(Key::Text("i"), lines);
    m.key(Key::Text("f"), lines);
    assert_eq!(m.viewport(lines), 0..23);
    let mut empty = Model::new(vec![]);
    empty.key(Key::Text("i"), lines);
    assert_eq!(empty.pane(), Pane::Combined);
    let archive = Model::from_archive(vec![], [], report()).unwrap();
    assert_eq!(archive.pane(), Pane::Combined);
}

#[test]
fn filter_consumes_keys_resets_scroll_and_never_traps_interrupt() {
    let mut m = model(0);
    m.complete(report());
    m.key(Key::End, lines);
    m.key(Key::Text("/"), lines);
    assert_eq!(m.viewport(lines), 56..80); // opening editor alone does not move it
    assert_eq!(m.key(Key::Text("q j"), lines), Action::None);
    assert_eq!(m.viewport(lines), 0..24);
    assert_eq!(m.key(Key::CtrlC, lines), Action::Interrupt);
    assert!(m.findings.as_ref().unwrap().typing());
    m.key(Key::Tab, lines);
    assert_eq!(m.pane(), Pane::Findings);
    assert_eq!(m.findings.as_ref().unwrap().query(), "q j");
    m.key(Key::Backspace, lines);
    assert_eq!(m.findings.as_ref().unwrap().query(), "q ");
    m.key(Key::Enter, lines);
    m.key(Key::Text("1"), lines);
    assert_eq!(m.pane(), Pane::Combined);
    m.key(Key::Text("/"), lines);
    assert!(!m.findings.as_ref().unwrap().typing());
    m.key(Key::Text("f"), lines);
    m.key(Key::Text("/"), lines);
    m.key(Key::Escape, lines);
    assert_eq!(m.findings.as_ref().unwrap().query(), "");
    assert_eq!(m.viewport(lines), 0..24);
}

#[test]
fn derived_rows_cannot_move_selected_findings_or_overlay_during_observation() {
    let mut model = model(1);
    model.complete(report());
    assert_eq!(model.pane(), Pane::Findings);
    model.key(Key::Text("i"), lines);
    let mut late = Event::agent(Kind::AgentStarted, "verify new", "late");
    late.at = "2026-10-10T00:00:01Z".into();
    model.apply(late).unwrap();
    assert_eq!(model.pane(), Pane::Input(0));
    model.key(Key::Escape, lines);
    assert_eq!(model.pane(), Pane::Findings);
    assert_eq!(model.agents.len(), 1);
}

#[test]
fn live_stage_rows_preserve_selected_agent_and_overlay_scroll_positions() {
    let mut model = model(2);
    model.key(Key::Text("3"), lines);
    model.key(Key::Up, lines);
    let review = model.viewport(lines);
    model.key(Key::Text("i"), lines);
    model.key(Key::Right, lines);
    model.key(Key::Down, lines);
    let input = model.viewport(lines);
    let mut event = Event::agent(Kind::AgentStarted, "synthesis", "new stage");
    event.at = "2026-10-10T00:00:01Z".into();
    model.apply(event).unwrap();
    assert_eq!(model.pane(), Pane::Input(1));
    // The new status row reduces pane height without selecting another document.
    assert_eq!(model.viewport(lines).end, input.end);
    model.key(Key::Escape, lines);
    assert_eq!(model.pane(), Pane::Agent(1));
    assert_eq!(model.viewport(lines).end, review.end);
}
