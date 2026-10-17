use revust_core::{
    report::Report,
    runtime::events::{Event, Kind},
};
use revust_tui::{
    Model,
    navigation::{Action, Key},
    updates::Updates,
};
use std::sync::mpsc;

fn event(text: &str) -> Event {
    Event {
        at: "2026-10-10T12:00:00Z".into(),
        ..Event::agent(Kind::AgentActivity, "a", text)
    }
}

#[test]
fn closed_event_stream_is_not_completion_and_final_report_is_applied_once() {
    let (events, rx) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let mut updates = Updates::new(rx, done);
    let mut model = Model::new(vec![]);
    events.send(event("last saved activity")).unwrap();
    drop(events);
    assert!(updates.poll(&mut model).unwrap());
    assert!(model.report.is_none());
    assert!(!updates.complete());
    assert_eq!(model.key(Key::Text("q"), |_, _| 0), Action::None);
    finished.send(Ok(Report::default())).unwrap();
    drop(finished);
    assert!(updates.poll(&mut model).unwrap());
    assert!(updates.complete());
    assert!(model.report.is_some());
    assert_eq!(model.agents[0].last, "last saved activity");
    assert_eq!(model.key(Key::Text("q"), |_, _| 0), Action::Quit);
    assert!(!updates.poll(&mut model).unwrap());
}

#[test]
fn queued_events_are_drained_in_bounded_batches_before_report_completion() {
    let (events, rx) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    for i in 0..3000 {
        events.send(event(&format!("line{i}"))).unwrap();
    }
    drop(events);
    finished.send(Ok(Report::default())).unwrap();
    drop(finished);
    let mut updates = Updates::new(rx, done);
    let mut model = Model::new(vec![]);
    updates.poll(&mut model).unwrap();
    assert!(!model.combined.is_empty());
    assert!(model.combined.len() < 2000);
    assert!(!updates.complete());
    assert!(model.report.is_none());
    for _ in 0..100 {
        if updates.complete() {
            break;
        }
        updates.poll(&mut model).unwrap();
    }
    assert!(updates.complete());
    assert_eq!(model.combined.len(), 2000);
    assert_eq!(model.combined.back().unwrap().text, "line2999");
}

#[test]
fn missing_failed_or_invalid_observation_never_becomes_a_successful_report() {
    let (events, rx) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    drop(events);
    drop(finished);
    let mut model = Model::new(vec![]);
    assert!(
        Updates::new(rx, done)
            .poll(&mut model)
            .unwrap_err()
            .contains("without completion")
    );
    assert!(model.report.is_none());
    let (_events, rx) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    finished.send(Err("archive write failed".into())).unwrap();
    assert_eq!(
        Updates::new(rx, done).poll(&mut model).unwrap_err(),
        "archive write failed"
    );
    assert!(model.report.is_none());
    let (events, rx) = mpsc::channel();
    let (_finished, done) = mpsc::channel();
    let mut invalid = event("invalid");
    invalid.at = "bad".into();
    events.send(invalid).unwrap();
    assert!(
        Updates::new(rx, done)
            .poll(&mut model)
            .unwrap_err()
            .contains("invalid event time")
    );
    assert!(model.combined.is_empty());
}
