use revust_core::{
    prompts::Agent,
    report::Report,
    runtime::{
        data::Finding,
        events::{Event, Kind},
    },
};
use revust_tui::{Model, State};

fn event(kind: Kind, agent: &str, text: &str, second: u32) -> Event {
    let mut e = Event::agent(kind, agent, text);
    e.at = format!("2026-10-10T12:{:02}:{:02}Z", second / 60, second % 60);
    e
}
fn roster() -> Vec<Agent> {
    ["a", "b"]
        .into_iter()
        .map(|name| Agent {
            name: name.into(),
            lenses: vec!["bugs".into()],
            runner: revust_core::prompts::runner::Runner::parse("claude/fixture").unwrap(),
            color: "6".into(),
            color_name: String::new(),
        })
        .collect()
}
fn findings(agent: &str, severity: &str, second: u32) -> Event {
    let mut e = event(Kind::Findings, agent, "", second);
    e.findings = vec![Finding {
        severity: severity.into(),
        ..Default::default()
    }];
    e
}
#[test]
fn lifecycle_preserves_roster_and_distinguishes_stage_replacement_from_source_counts() {
    let roster = roster();
    let a = &roster[0].name;
    let b = &roster[1].name;
    let mut model = Model::new(roster.clone());
    assert_eq!(model.agents.len(), roster.len());
    assert_eq!(model.agents[0].state, State::Waiting);
    let mut stage = Event::stage("find");
    stage.at = "2026-10-10T12:00:00Z".into();
    model.apply(stage).unwrap();
    model
        .apply(event(Kind::AgentStarted, a, "bugs", 0))
        .unwrap();
    model.apply(findings(a, "major", 1)).unwrap();
    model.apply(findings(b, "invented", 2)).unwrap();
    assert_eq!(model.found.total, 2);
    assert_eq!(model.found.major, 1);
    assert_eq!(model.combined.len(), 2); // stage and started, no duplicate finding count lines
    model.apply(findings("synthesis", "minor", 3)).unwrap();
    assert_eq!(model.found.total, 1);
    assert_eq!(model.found.major, 0);
    assert_eq!(model.found.minor, 1);
    assert_eq!(model.agents.last().unwrap().name, "synthesis");
    assert!(model.agents.last().unwrap().roster.is_none());
    model.apply(event(Kind::RateLimit, a, "wait", 4)).unwrap();
    assert_eq!(model.agents[0].state, State::Limited);
    model
        .apply(event(Kind::AgentRetried, a, "again", 5))
        .unwrap();
    assert_eq!(model.agents[0].state, State::Retrying);
    model
        .apply(event(Kind::AgentDegraded, a, "failed", 6))
        .unwrap();
    model
        .apply(event(Kind::Dropped, "synthesis", "dropped 1 candidate", 7))
        .unwrap();
    assert_eq!(model.agents[0].state, State::Degraded);
    assert!(model.report.is_none());
    let report = Report {
        findings: findings(a, "critical", 8).findings,
        ..Default::default()
    };
    model.complete(report);
    assert_eq!(model.found.total, 1);
    assert_eq!(model.found.critical, 1);
    assert!(model.report.is_some());
}
#[test]
fn heartbeat_tracks_latest_activity_and_elapsed_uses_events_not_wall_clock() {
    let mut model = Model::new(vec![]);
    for e in [
        event(Kind::AgentStarted, "a", "", 0),
        event(Kind::AgentProgress, "a", "old", 1),
        event(Kind::AgentProgress, "a", "new", 2),
        event(Kind::AgentActivity, "a", "narration", 5),
        event(Kind::AgentProgress, "a", "", 10),
        event(Kind::AgentProgress, "a", "", 15),
    ] {
        model.apply(e).unwrap();
    }
    assert_eq!(model.agents[0].lines.len(), 3);
    assert_eq!(model.agents[0].lines.back().unwrap().text, "new");
    model
        .apply(event(Kind::AgentDone, "a", "finished", 20))
        .unwrap();
    model.apply(event(Kind::AgentStarted, "b", "", 30)).unwrap();
    assert_eq!(
        model.agents[0]
            .elapsed(model.now.unwrap())
            .unwrap()
            .num_seconds(),
        20
    );
    assert!(model.agents[1].elapsed(model.now.unwrap()).is_none());
    model
        .apply(event(Kind::AgentState, "a", "late activity", 31))
        .unwrap();
    assert_eq!(model.agents[0].state, State::Running);
    assert_eq!(
        model.agents[1]
            .elapsed(model.now.unwrap())
            .unwrap()
            .num_seconds(),
        1
    );
    let old = model.now;
    model
        .apply(event(Kind::AgentActivity, "b", "out of order", 29))
        .unwrap();
    assert_eq!(model.now, old);
}
#[test]
fn logs_are_bounded_and_archive_replay_requires_explicit_report() {
    let mut events = vec![];
    for i in 0..2005 {
        events.push(event(Kind::AgentActivity, "a", &format!("line {i}"), i));
    }
    let mut live = Model::new(vec![]);
    for e in events.iter().cloned() {
        live.apply(e).unwrap();
    }
    assert_eq!(live.combined.len(), 2000);
    assert_eq!(live.agents[0].lines.len(), 2000);
    assert_eq!(live.combined.front().unwrap().text, "line 5");
    assert!(live.report.is_none());
    for i in 0..10 {
        let e = event(Kind::AgentActivity, "b", &format!("other {i}"), 2010 + i);
        live.apply(e.clone()).unwrap();
        events.push(e);
    }
    assert_eq!(live.combined.front().unwrap().text, "line 15");
    assert_eq!(live.agents[0].lines.front().unwrap().text, "line 5");
    assert_eq!(live.agents[1].lines.len(), 10);
    let archive = Model::from_archive(vec![], events, Report::default()).unwrap();
    assert_eq!(archive.stage, "archive");
    assert!(archive.report.is_some());
    assert_eq!(archive.combined, live.combined);
    let mut invalid = event(Kind::AgentDone, "a", "", 0);
    invalid.at = "invalid".into();
    assert!(live.apply(invalid).is_err());
    assert_eq!(live.agents[0].state, State::Running);
}

#[test]
fn durable_completion_freezes_model_and_report_even_if_late_events_arrive() {
    let mut model = Model::new(roster());
    model.apply(event(Kind::AgentStarted, "a", "", 0)).unwrap();
    model.complete(Report {
        findings: vec![Finding {
            title: "saved".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let before = model.combined.clone();
    let observed = model.now;
    model
        .apply(event(
            Kind::AgentActivity,
            "late",
            "discarded observation",
            40,
        ))
        .unwrap();
    model.complete(Report::default());
    assert_eq!(model.combined, before);
    assert_eq!(model.now, observed);
    assert_eq!(model.agents.len(), 2);
    assert_eq!(model.report.as_ref().unwrap().findings[0].title, "saved");
}

#[test]
fn saved_unknown_events_preserve_identity_and_time_without_claiming_success() {
    let events = [
        serde_json::json!({"kind":"future_kind","agent":"future source","at":"2026-10-10T12:00:00Z"}),
        serde_json::json!({"kind":"future_kind","agent":"future source","at":"2026-10-10T12:00:09Z"}),
    ].into_iter().map(|value|serde_json::from_value(value).unwrap());
    let model = Model::from_saved_events(vec![], events, Report::default()).unwrap();
    assert_eq!(model.agents.len(), 1);
    assert_eq!(model.agents[0].name, "future source");
    assert!(model.agents[0].roster.is_none());
    assert_eq!(model.agents[0].state, State::Running);
    assert!(model.agents[0].lines.is_empty());
    assert!(model.combined.is_empty());
    assert_eq!(
        model.agents[0]
            .elapsed(model.now.unwrap())
            .unwrap()
            .num_seconds(),
        9
    );
    assert_eq!(model.stage, "archive");
    let bad = [
        serde_json::from_value(serde_json::json!({"kind":"future_kind","at":"invalid"})).unwrap(),
    ];
    assert!(Model::from_saved_events(vec![], bad, Report::default()).is_err());
}
