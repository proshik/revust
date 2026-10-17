use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};
use revust_core::report::Report;
use revust_tui::{
    Model,
    navigation::{Key, Pane},
    status::Status,
};

fn row(buffer: &Buffer, y: u16) -> String {
    (buffer.area.x..buffer.area.right())
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn frame_uses_current_width_scroll_and_styles_and_clears_previous_contents() {
    let status = Status::default();
    let mut model = Model::new(vec![]);
    let area = Rect::new(3, 4, 20, 10);
    let mut buffer = Buffer::empty(area);
    for cell in &mut buffer.content {
        cell.set_symbol("x").set_bg(Color::Red);
    }
    let mut calls = 0;
    status.frame(&mut model, &mut buffer, |m, pane| {
        calls += 1;
        assert_eq!(m.width(), 20);
        assert_eq!(pane, Pane::Combined);
        (0..8)
            .map(|i| Line::styled(format!("row{i}"), Style::default().fg(Color::Green)))
            .collect()
    });
    assert_eq!(calls, 1);
    assert_eq!(row(&buffer, 10).trim_end(), "row4");
    assert_eq!(row(&buffer, 13).trim_end(), "row7");
    assert_eq!(buffer[(3, 10)].fg, Color::Green);
    assert_eq!(buffer[(19, 10)].bg, Color::Reset);
    model.key(Key::Home, |_, _| 8);
    status.frame(&mut model, &mut buffer, |_, _| {
        (0..8).map(|i| Line::raw(format!("row{i}"))).collect()
    });
    assert_eq!(row(&buffer, 10).trim_end(), "row0");
    status.frame(&mut model, &mut buffer, |_, _| vec![Line::raw("short")]);
    assert_eq!(row(&buffer, 10).trim_end(), "short");
    assert!(row(&buffer, 11).trim().is_empty());
    assert!(row(&buffer, 13).trim().is_empty());
}

#[test]
fn very_short_terminal_keeps_bottom_rows_like_reference_terminal_renderer() {
    let mut model = Model::new(vec![]);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 1));
    Status::default().frame(&mut model, &mut buffer, |_, _| vec![Line::raw("detail")]);
    assert_eq!(row(&buffer, 0).trim_end(), "detail");
    let mut empty = Buffer::empty(Rect::new(0, 0, 0, 0));
    Status::default().frame(&mut model, &mut empty, |_, _| {
        panic!("zero-area frame should not render")
    });
}

#[test]
fn completion_and_resize_measure_findings_before_selecting_the_viewport() {
    let mut model = Model::new(vec![]);
    model.complete(Report::default());
    let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 9));
    Status::default().frame(&mut model, &mut buffer, |m, p| {
        assert_eq!(m.width(), 24);
        assert_eq!(p, Pane::Findings);
        (0..10).map(|i| Line::raw(format!("finding{i}"))).collect()
    });
    assert_eq!(row(&buffer, 6).trim_end(), "finding0");
    assert_eq!(row(&buffer, 8).trim_end(), "finding2");
}

#[test]
fn caller_styled_spans_cannot_emit_active_terminal_controls() {
    let mut model = Model::new(vec![]);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 10));
    Status::default().frame(&mut model, &mut buffer, |_, _| {
        vec![Line::styled(
            "file\x1b]52;c;payload\x07",
            Style::default().fg(Color::Green),
        )]
    });
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| !cell.symbol().chars().any(|ch| ch.is_control()))
    );
    assert!(buffer.content.iter().any(|cell| cell.fg == Color::Green));
}
