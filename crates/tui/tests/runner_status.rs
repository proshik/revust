use ratatui::{buffer::Buffer, layout::Rect};
use revust_core::{
    prompts::{Agent, runner::Runner},
    report::Report,
    runtime::events::{Event, Kind},
};
use revust_tui::{Model, status::Status};

fn runner(executor: &str, model: &str, effort: &str) -> Runner {
    Runner {
        executor: executor.into(),
        model: model.into(),
        effort: effort.into(),
    }
}
fn started(name: &str) -> Event {
    let mut event = Event::agent(Kind::AgentStarted, name, "started");
    event.at = "2026-10-11T12:00:00Z".into();
    event
}
fn rows(model: &mut Model, width: u16) -> Vec<String> {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, 20));
    Status::default().frame(model, &mut buffer, |_, _| vec![]);
    (0..20)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}
fn roster() -> Vec<Agent> {
    vec![Agent {
        name: "finder".into(),
        runner: runner("claude", "sonnet", "low"),
        lenses: vec![],
        color: String::new(),
        color_name: String::new(),
    }]
}

#[test]
fn archived_table_shows_saved_runners_for_finder_synthesis_and_verify_groups() {
    let report: Report = serde_json::from_value(serde_json::json!({
        "stats":{"stages":[
            {"name":"synthesis","executor":"codex","model":"gpt-5","effort":"high"},
            {"name":"verify","executor":"opencode","model":"fixture/check"}
        ]}
    }))
    .unwrap();
    let mut model = Model::from_archive(
        roster(),
        [
            started("finder"),
            started("synthesis"),
            started("verify root"),
            started("verify src"),
        ],
        report,
    )
    .unwrap();
    let screen = rows(&mut model, 130);
    for label in ["EXECUTOR", "MODEL", "EFFORT"] {
        assert!(screen[2].contains(label), "{}", screen[2]);
    }
    for (row, executor, model, effort) in [
        (3, "claude", "sonnet", "low"),
        (4, "codex", "gpt-5", "high"),
        (5, "opencode", "fixture/check", "-"),
        (6, "opencode", "fixture/check", "-"),
    ] {
        for value in [executor, model, effort] {
            assert!(
                screen[row].split_whitespace().any(|s| s == value),
                "{}",
                screen[row]
            );
        }
    }
    let mut legacy =
        Model::from_archive(vec![], [started("synthesis")], Report::default()).unwrap();
    let screen = rows(&mut legacy, 130);
    assert_eq!(
        screen[3].split_whitespace().filter(|s| *s == "-").count(),
        4
    );
}

#[test]
fn live_table_uses_frozen_stage_settings_before_completion_and_after_resize() {
    let mut model = Model::new(roster()).with_stage_runners(
        runner("codex", "gpt-5", "high"),
        runner("opencode", "fixture/check", ""),
    );
    model.apply(started("synthesis")).unwrap();
    model.apply(started("verify root")).unwrap();
    let wide = rows(&mut model, 130);
    assert!(wide[4].contains("codex") && wide[4].contains("gpt-5") && wide[4].contains("high"));
    assert!(wide[5].contains("opencode") && wide[5].contains("fixture/check"));
    assert!(model.report.is_none());
    let narrow = rows(&mut model, 65);
    assert!(narrow[4].contains("running") && narrow[4].contains("codex"));
    assert!(narrow[5].contains("opencode"));
    assert_eq!(rows(&mut model, 130), wide);
}
