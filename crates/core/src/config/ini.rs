//! A small assignment grammar with explicit source locations and no writes.
use std::{fs, io, path::Path};
pub struct Entry {
    pub key: String,
    pub value: String,
    pub line: usize,
}
fn content(line: &str) -> Option<&str> {
    let text = line.trim();
    (!text.is_empty() && !text.starts_with(['#', ';'])).then_some(text)
}
pub fn read(path: &Path) -> Result<Vec<Entry>, String> {
    let source = match fs::read_to_string(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
        Ok(source) => source,
    };
    // Comment/header-only files are refreshable templates, not active settings.
    if !source
        .lines()
        .filter_map(content)
        .any(|line| !line.starts_with('[') && line.contains('='))
    {
        return Ok(Vec::new());
    }
    source
        .lines()
        .enumerate()
        .filter_map(|(line, text)| content(text).map(|text| (line + 1, text)))
        .map(|(line, text)| {
            let failure = |detail| format!("parse {}:{line}: {detail}", path.display());
            if let Some(section) = text.strip_prefix('[') {
                let section = section
                    .strip_suffix(']')
                    .ok_or_else(|| failure("malformed section header".to_owned()))?
                    .trim();
                match section {
                    "" => Err(failure("empty section name".to_owned())),
                    name if name.eq_ignore_ascii_case("Application Options") => Ok(None),
                    name => Err(failure(format!("could not find option group `{name}'"))),
                }
            } else {
                let (key, value) = text
                    .split_once('=')
                    .ok_or_else(|| failure("malformed key=value".to_owned()))?;
                let value = value.trim();
                let decoded = if value.starts_with('"') {
                    quoted(value).map_err(failure)?
                } else {
                    value.to_owned()
                };
                Ok(Some(Entry {
                    key: key.trim().to_owned(),
                    value: decoded,
                    line,
                }))
            }
        })
        .filter_map(Result::transpose)
        .collect()
}

/// Decode escape units into bytes, then apply the public lossy UTF-8 boundary.
fn quoted(text: &str) -> Result<String, String> {
    let mut cursor = text.chars();
    if cursor.next() != Some('"') {
        return Err("invalid quoted string".into());
    }
    let mut bytes = Vec::new();
    loop {
        let character = cursor.next().ok_or("invalid quoted string")?;
        match character {
            '"' => {
                return if cursor.next().is_none() {
                    Ok(String::from_utf8_lossy(&bytes).into_owned())
                } else {
                    Err("invalid quoted string".into())
                };
            }
            '\n' | '\r' => return Err("invalid quoted string".into()),
            '\\' => {
                let code = cursor.next().ok_or("invalid quoted string")?;
                match code {
                    'x' | 'u' | 'U' | '0'..='7' => {
                        let octal = code.is_ascii_digit();
                        let count = match code {
                            'u' => 4,
                            'U' => 8,
                            _ => 2,
                        };
                        let radix = if octal { 8 } else { 16 };
                        let mut number = if octal { code.to_digit(8).unwrap() } else { 0 };
                        for _ in 0..count {
                            let digit = cursor
                                .next()
                                .filter(char::is_ascii)
                                .and_then(|c| c.to_digit(radix))
                                .ok_or("invalid quoted escape")?;
                            number = number
                                .checked_mul(radix)
                                .and_then(|n| n.checked_add(digit))
                                .ok_or("invalid quoted escape")?;
                        }
                        if matches!(code, 'u' | 'U') {
                            let scalar = char::from_u32(number).ok_or("invalid Unicode escape")?;
                            bytes.extend_from_slice(scalar.encode_utf8(&mut [0; 4]).as_bytes());
                        } else {
                            bytes.push(u8::try_from(number).map_err(|_| "invalid byte escape")?);
                        }
                    }
                    _ => {
                        let escaped = match code {
                            'a' => 7,
                            'b' => 8,
                            'f' => 12,
                            'n' => 10,
                            'r' => 13,
                            't' => 9,
                            'v' => 11,
                            '\\' => b'\\',
                            '"' => b'"',
                            _ => return Err("invalid quoted escape".into()),
                        };
                        bytes.push(escaped);
                    }
                }
            }
            _ => bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
}
