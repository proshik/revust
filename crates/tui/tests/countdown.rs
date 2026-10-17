use revust_core::{
    report::Report,
    runtime::events::{Event, Kind},
};
use revust_tui::{
    Model,
    navigation::{Action, Key},
};
use std::time::Duration;

#[test]
fn only_explicit_completion_starts_the_one_second_countdown() {
    let mut model = Model::new(vec![]).with_auto_exit(Duration::from_millis(1500));
    let mut done = Event::agent(Kind::AgentDone, "finder", "");
    done.at = "2026-10-10T00:00:00Z".into();
    model.apply(done).unwrap();
    assert_eq!(model.exit_remaining(), Duration::ZERO);
    assert_eq!(model.tick(), Action::None);
    model.key(Key::Other, |_, _| 1); // a key before completion does not disable configuration
    model.complete(Report::default());
    assert_eq!(model.exit_remaining(), Duration::from_millis(1500));
    assert_eq!(model.tick(), Action::None);
    assert_eq!(model.exit_remaining(), Duration::from_millis(500));
    assert_eq!(model.tick(), Action::Quit);
    assert_eq!(model.exit_remaining(), Duration::ZERO);
    assert_eq!(model.tick(), Action::None);
}

#[test]
fn every_key_cancels_countdown_even_when_it_has_no_navigation_action() {
    for key in [
        Key::Other,
        Key::Text("0"),
        Key::Escape,
        Key::Text("/"),
        Key::CtrlC,
        Key::Text("q"),
    ] {
        let mut model = Model::new(vec![]).with_auto_exit(Duration::from_secs(2));
        model.complete(Report::default());
        model.key(key, |_, _| 1);
        assert_eq!(model.exit_remaining(), Duration::ZERO, "{key:?}");
        assert_eq!(model.tick(), Action::None);
    }
}

#[test]
fn archive_and_zero_configuration_never_start_auto_exit() {
    let mut disabled = Model::new(vec![]);
    disabled.complete(Report::default());
    assert_eq!(disabled.tick(), Action::None);
    let mut archive = Model::from_archive(vec![], [], Report::default())
        .unwrap()
        .with_auto_exit(Duration::from_secs(1));
    assert_eq!(archive.exit_remaining(), Duration::ZERO);
    assert_eq!(archive.tick(), Action::None);
    archive.complete(Report::default());
    assert_eq!(archive.tick(), Action::None);
}
