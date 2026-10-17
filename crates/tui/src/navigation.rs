//! Typed pane selection and independent overlay positions.
use crate::{Model, findings::FilterKey};
use std::ops::Range;

#[derive(Clone, Debug, Default)]
pub struct InputDocument {
    pub label: String,
    pub path: String,
    pub content: String,
    pub notice: String,
    pub markdown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Combined,
    Agent(usize),
    Findings,
    Input(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    /// The terminal owner must cancel the owned review before leaving the UI.
    Interrupt,
}

#[derive(Clone, Copy, Debug)]
pub enum Key<'a> {
    Text(&'a str),
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    CtrlB,
    CtrlF,
    Home,
    End,
    Enter,
    Escape,
    Backspace,
    CtrlC,
    Other,
}

const TOKENS: &str = "123456789abcdemnoprstuvwxyz";
pub fn tab_token(index: usize) -> Option<char> {
    TOKENS.as_bytes().get(index).map(|&byte| char::from(byte))
}

#[derive(Clone, Copy)]
struct Cursor {
    pane: Pane,
    distance: usize,
    top: bool,
}
impl Cursor {
    fn new(pane: Pane) -> Self {
        Self {
            pane,
            distance: 0,
            top: matches!(pane, Pane::Input(_) | Pane::Findings),
        }
    }
}
pub(crate) struct View {
    cols: usize,
    rows: usize,
    review: Cursor,
    input: Cursor,
    overlay: bool,
    show_completed: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            cols: 100,
            rows: 30,
            review: Cursor::new(Pane::Combined),
            input: Cursor::new(Pane::Input(0)),
            overlay: false,
            show_completed: false,
        }
    }
}
impl View {
    fn cursor(&mut self) -> &mut Cursor {
        if self.overlay {
            &mut self.input
        } else {
            &mut self.review
        }
    }
    pub(crate) fn complete(&mut self, _: usize) {
        if self.overlay {
            self.show_completed = true;
        } else {
            self.review = Cursor::new(Pane::Findings);
        }
    }
    pub(crate) fn archive(&mut self) {
        self.review = Cursor::new(Pane::Combined);
    }
    fn close_overlay(&mut self) {
        self.overlay = false;
        if self.show_completed {
            self.review = Cursor::new(Pane::Findings);
            self.show_completed = false;
        }
    }
}

// Commands are decoded independently of the current pane; editor precedence and
// availability checks belong to the state machine below.
enum Command {
    Select(usize),
    Move(bool),
    Scroll(bool, bool),
    Top,
    Bottom,
    Inputs,
    Findings,
    Filter,
    Quit,
    None,
}
fn command(key: Key<'_>) -> Command {
    match key {
        Key::Tab | Key::Right | Key::Text("l") => Command::Move(true),
        Key::BackTab | Key::Left | Key::Text("h") => Command::Move(false),
        Key::Up | Key::Text("k") => Command::Scroll(true, false),
        Key::Down | Key::Text("j") => Command::Scroll(false, false),
        Key::PageUp | Key::CtrlB => Command::Scroll(true, true),
        Key::PageDown | Key::CtrlF => Command::Scroll(false, true),
        Key::Home | Key::Text("g") => Command::Top,
        Key::End | Key::Text("G") => Command::Bottom,
        Key::Text("i") => Command::Inputs,
        Key::Text("f") => Command::Findings,
        Key::Text("/") => Command::Filter,
        Key::Text("q") => Command::Quit,
        Key::Text(text) if text.len() == 1 => {
            TOKENS.find(text).map_or(Command::None, Command::Select)
        }
        _ => Command::None,
    }
}
impl Model {
    pub fn with_inputs(mut self, inputs: Vec<InputDocument>) -> Self {
        self.inputs = inputs;
        self
    }
    pub fn inputs(&self) -> &[InputDocument] {
        &self.inputs
    }
    pub fn pane(&self) -> Pane {
        if self.view.overlay {
            self.view.input.pane
        } else {
            self.view.review.pane
        }
    }
    pub fn width(&self) -> usize {
        self.view.cols
    }
    pub fn pane_height(&self) -> usize {
        self.view
            .rows
            .saturating_sub(self.agents.len().saturating_add(6))
            .max(1)
    }
    pub fn viewport(&mut self, mut lines: impl FnMut(&Self, Pane) -> usize) -> Range<usize> {
        let total = lines(self, self.pane());
        let height = self.pane_height();
        let distance = total.saturating_sub(height);
        let cursor = self.view.cursor();
        cursor.distance = if cursor.top {
            distance
        } else {
            distance.min(cursor.distance)
        };
        cursor.top = false;
        let end = total - cursor.distance;
        end.saturating_sub(height)..end
    }
    pub fn resize(&mut self, cols: usize, rows: usize, lines: impl FnMut(&Self, Pane) -> usize) {
        self.view.cols = if cols > 0 { cols } else { 100 };
        self.view.rows = if rows > 0 { rows } else { 30 };
        self.viewport(lines);
    }
    fn choices(&self) -> Vec<Pane> {
        if self.view.overlay {
            return (0..self.inputs.len()).map(Pane::Input).collect();
        }
        let mut panes = vec![Pane::Combined];
        panes.extend((0..self.agents.len()).map(Pane::Agent));
        if self.findings.is_some() {
            panes.push(Pane::Findings);
        }
        panes
    }
    fn select(&mut self, index: usize) {
        if let Some(pane) = self.choices().get(index) {
            *self.view.cursor() = Cursor::new(*pane);
        }
    }
    pub fn key(&mut self, key: Key<'_>, mut lines: impl FnMut(&Self, Pane) -> usize) -> Action {
        self.exit_in = std::time::Duration::ZERO;
        if matches!(key, Key::CtrlC) {
            return Action::Interrupt;
        }
        self.viewport(&mut lines);
        if matches!(key, Key::Escape) && self.view.overlay {
            self.view.close_overlay();
        } else if self.pane() == Pane::Findings
            && self.findings.as_ref().is_some_and(|f| f.typing())
        {
            let edit = match key {
                Key::Text(s) => FilterKey::Text(s),
                Key::Backspace => FilterKey::Backspace,
                Key::Enter => FilterKey::Enter,
                Key::Escape => FilterKey::Escape,
                _ => FilterKey::Other,
            };
            if let Some(findings) = &mut self.findings {
                findings.edit(edit);
            }
            self.view.cursor().top = true;
        } else {
            match command(key) {
                Command::Quit if self.report.is_some() => return Action::Quit,
                Command::Inputs if !self.inputs.is_empty() => {
                    if self.view.overlay {
                        self.view.close_overlay();
                    } else {
                        self.view.overlay = true;
                        self.view.show_completed = self.view.review.pane == Pane::Findings;
                    }
                }
                Command::Findings if self.findings.is_some() => {
                    self.view.close_overlay();
                    self.view.review = Cursor::new(Pane::Findings);
                }
                Command::Filter if self.pane() == Pane::Findings => {
                    if let Some(f) = &mut self.findings {
                        f.start_filter();
                    }
                }
                Command::Select(index) => self.select(index),
                Command::Move(forward) => {
                    let choices = self.choices();
                    if let Some(index) = choices.iter().position(|p| *p == self.pane()) {
                        let next = if forward {
                            index.checked_add(1)
                        } else {
                            index.checked_sub(1)
                        };
                        if let Some(next) = next {
                            self.select(next);
                        }
                    }
                }
                Command::Scroll(up, page) => {
                    let amount = if page { self.pane_height() } else { 1 };
                    let cursor = self.view.cursor();
                    cursor.distance = if up {
                        cursor.distance.saturating_add(amount)
                    } else {
                        cursor.distance.saturating_sub(amount)
                    };
                }
                Command::Top => self.view.cursor().top = true,
                Command::Bottom => self.view.cursor().distance = 0,
                _ => (),
            }
        }
        self.viewport(lines);
        Action::None
    }
}
