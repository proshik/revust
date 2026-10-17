//! Translate public Crossterm input into observational model actions.
use crate::{
    Model,
    navigation::{Action, Key, Pane},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
fn editing_key(event: &KeyEvent, editing: bool) -> Option<Key<'static>> {
    let modifiers = event.modifiers;
    if modifiers.contains(KeyModifiers::CONTROL) {
        return Some(match event.code {
            KeyCode::Char('c') => Key::CtrlC,
            KeyCode::Char('b') if modifiers == KeyModifiers::CONTROL => Key::CtrlB,
            KeyCode::Char('f') if modifiers == KeyModifiers::CONTROL => Key::CtrlF,
            _ => Key::Other,
        });
    }
    let allowed = KeyModifiers::SHIFT
        | if editing {
            KeyModifiers::ALT
        } else {
            KeyModifiers::NONE
        };
    if matches!(event.code, KeyCode::Char(_)) && modifiers.difference(allowed).is_empty() {
        return None;
    }
    if event.code == KeyCode::BackTab && modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return Some(Key::BackTab);
    }
    if !modifiers.is_empty() {
        return Some(Key::Other);
    }
    Some(match event.code {
        KeyCode::Tab => Key::Tab,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        _ => Key::Other,
    })
}
impl Model {
    pub fn terminal_event(
        &mut self,
        event: Event,
        lines: impl FnMut(&Self, Pane) -> usize,
    ) -> Action {
        let editing =
            self.pane() == Pane::Findings && self.findings.as_ref().is_some_and(|f| f.typing());
        match event {
            Event::Resize(width, height) => {
                self.resize(width as usize, height as usize, lines);
                Action::None
            }
            Event::Key(event) if event.kind != KeyEventKind::Release => {
                match editing_key(&event, editing) {
                    Some(key) => self.key(key, lines),
                    None => {
                        let KeyCode::Char(ch) = event.code else {
                            unreachable!()
                        };
                        self.key(Key::Text(&ch.to_string()), lines)
                    }
                }
            }
            Event::Paste(text) => self.key(
                if editing {
                    Key::Text(&text)
                } else {
                    Key::Other
                },
                lines,
            ),
            _ => Action::None,
        }
    }
}
