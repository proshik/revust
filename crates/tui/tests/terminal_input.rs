use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use revust_core::{report::Report, runtime::data::Finding};
use revust_tui::{
    Model,
    navigation::{Action, Pane},
};
use std::time::Duration;

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}
fn send(model: &mut Model, event: Event) -> Action {
    model.terminal_event(event, |_, _| 100)
}
fn complete() -> Model {
    let mut model = Model::new(vec![]).with_auto_exit(Duration::from_secs(5));
    model.complete(Report {
        findings: vec![Finding {
            title: "text".into(),
            ..Finding::default()
        }],
        ..Report::default()
    });
    model
}

#[test]
fn key_press_repeat_release_and_non_key_events_have_distinct_effects() {
    let mut model = complete();
    send(&mut model, Event::FocusLost);
    send(&mut model, Event::Resize(70, 20));
    send(
        &mut model,
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )),
    );
    assert_eq!(model.exit_remaining(), Duration::from_secs(5));
    assert_eq!(model.width(), 70);
    assert_eq!(model.pane_height(), 14);
    assert_eq!(
        send(&mut model, key(KeyCode::F(8), KeyModifiers::NONE)),
        Action::None
    );
    assert_eq!(model.exit_remaining(), Duration::ZERO);
    assert_eq!(
        send(
            &mut model,
            Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            ))
        ),
        Action::Quit
    );
    let mut live = Model::new(vec![]);
    assert_eq!(
        send(&mut live, key(KeyCode::Char('q'), KeyModifiers::NONE)),
        Action::None
    );
    assert_eq!(
        send(&mut live, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::Interrupt
    );
}

#[test]
fn pasted_commands_never_navigate_but_unicode_paste_and_alt_text_edit_filter() {
    let mut model = complete();
    assert_eq!(send(&mut model, Event::Paste("q".into())), Action::None);
    assert_eq!(model.pane(), Pane::Findings);
    send(&mut model, key(KeyCode::Char('/'), KeyModifiers::NONE));
    send(&mut model, Event::Paste("q界\ne\u{301}".into()));
    send(&mut model, key(KeyCode::Char('x'), KeyModifiers::ALT));
    send(&mut model, key(KeyCode::Backspace, KeyModifiers::NONE));
    send(&mut model, key(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(model.findings.as_ref().unwrap().query(), "q界\ne");
    assert_eq!(
        send(&mut model, key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::Interrupt
    );
    assert_eq!(model.findings.as_ref().unwrap().query(), "q界\ne");
}

#[test]
fn navigation_respects_modifiers_and_resizes_preserve_scroll_bounds() {
    let mut model = complete();
    send(&mut model, key(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(model.pane(), Pane::Combined);
    send(&mut model, key(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(model.pane(), Pane::Combined);
    send(&mut model, key(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(model.pane(), Pane::Findings);
    send(&mut model, key(KeyCode::End, KeyModifiers::NONE));
    send(&mut model, key(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert_eq!(model.viewport(|_, _| 100), 52..76);
    send(&mut model, Event::Resize(40, 10));
    assert_eq!(model.viewport(|_, _| 100), 72..76);
}
