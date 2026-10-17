//! Validated foreground commands using the terminal library's public formatter.
use crossterm::style::{Color, SetForegroundColor};
pub use revust_core::prompts::colors::derived;
pub fn sequence(value: &str) -> Option<String> {
    let color = if let Some(rgb) = value
        .strip_prefix('#')
        .filter(|rgb| rgb.len() == 6 && rgb.is_ascii())
    {
        Color::Rgb {
            r: u8::from_str_radix(&rgb[..2], 16).ok()?,
            g: u8::from_str_radix(&rgb[2..4], 16).ok()?,
            b: u8::from_str_radix(&rgb[4..], 16).ok()?,
        }
    } else {
        Color::AnsiValue(value.parse::<u8>().ok().filter(|index| *index < 16)?)
    };
    Some(if crossterm::style::Colored::ansi_color_disabled() {
        String::new()
    } else {
        SetForegroundColor(color).to_string()
    })
}
pub fn paint(color: &str, name: &str) -> String {
    let text = crate::status::metadata(name);
    if text.is_empty() {
        return text;
    }
    match sequence(color) {
        Some(command) if !command.is_empty() => {
            format!("{command}{text}{}", SetForegroundColor(Color::Reset))
        }
        _ => text,
    }
}
