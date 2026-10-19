//! Provider definitions for isolated sessions, loaded from a user file.

use serde_json::{Value, json};
use std::{fs, io, path::Path, path::PathBuf};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Native package substitutions for opencode.jsonc provider entries.
const PACKAGES: &[(&str, &str)] = &[(
    "@ai-sdk/openai-compatible",
    "@opencode/ai/providers/openai-compatible",
)];

/// Shared qualification rules for provider maps from env content or a file.
pub fn validate(providers: &Value) -> io::Result<()> {
    let map = providers
        .as_object()
        .ok_or_else(|| invalid("OpenCode providers must be an object"))?;
    for provider in map.values() {
        if !provider.is_object() {
            return Err(invalid("OpenCode provider must be an object"));
        }
        if let Some(package) = provider.get("package") {
            // Qualified native package: never install an npm or file provider.
            if package != "@opencode/ai/providers/openai-compatible" {
                return Err(invalid("unqualified OpenCode provider package"));
            }
        }
        if provider
            .get("models")
            .and_then(Value::as_object)
            .is_some_and(|models| models.values().any(|model| model.get("package").is_some()))
        {
            return Err(invalid(
                "OpenCode model package overrides are not qualified",
            ));
        }
    }
    Ok(())
}

/// Load provider definitions from a configured file. The format is detected
/// after parsing: a {"providers":{...}} wrapper is unwrapped, an
/// opencode.jsonc document ({"provider":{...}} plus optional
/// "enabled_providers") is converted to native entries, and any other object
/// is a bare provider-ID map used as is.
pub fn load(path: &Path) -> io::Result<Value> {
    let text = fs::read_to_string(path)
        .map_err(|error| invalid(&format!("providers file {}: {error}", path.display())))?;
    let value: Value = serde_json::from_str(&strip_jsonc(&text)).map_err(|error| {
        invalid(&format!(
            "providers file {} is not valid JSON(C): {error}",
            path.display()
        ))
    })?;
    let document = value.as_object().ok_or_else(|| {
        invalid(&format!(
            "providers file {} must contain an object",
            path.display()
        ))
    })?;
    let mut providers = if let Some(providers) = document.get("providers") {
        providers.clone()
    } else if let Some(provider) = document.get("provider") {
        convert(provider, document.get("enabled_providers"))?
    } else {
        value.clone()
    };
    expand_home_references(&mut providers);
    Ok(providers)
}

/// Rewrite {file:~/...} secret references to absolute paths. The isolated
/// child receives a fresh HOME, so tilde expansion must happen against the
/// revust process home while the value stays a file reference that OpenCode
/// itself reads.
fn expand_home_references(providers: &mut Value) {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .and_then(|home| home.into_string().ok());
    if let Some(home) = home {
        expand_references(providers, &home);
    }
}

fn expand_references(value: &mut Value, home: &str) {
    if home.is_empty() {
        return;
    }
    match value {
        Value::String(text) => {
            if let Some(rest) = text.strip_prefix("{file:~/") {
                *text = format!("{{file:{home}/{rest}");
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| expand_references(item, home)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| expand_references(item, home)),
        _ => (),
    }
}

/// Resolve a configured path: a leading ~ expands to HOME and relative paths
/// resolve from the revust process working directory.
pub fn resolve_path(text: &str) -> io::Result<PathBuf> {
    let home = || {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| invalid("HOME is not set to expand a providers_file path"))
    };
    let path = if text == "~" {
        home()?
    } else if let Some(rest) = text.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        PathBuf::from(text)
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| invalid(&format!("providers file working directory: {error}")))
    }
}

/// Convert opencode.jsonc provider entries: npm maps to a native package,
/// options become settings at provider and model level, and
/// enabled_providers filters entries when present.
fn convert(provider: &Value, enabled: Option<&Value>) -> io::Result<Value> {
    let entries = provider
        .as_object()
        .ok_or_else(|| invalid("providers file \"provider\" must be an object"))?;
    let enabled: Option<Vec<&str>> = match enabled {
        None | Some(Value::Null) => None,
        Some(list) => Some(
            list.as_array()
                .ok_or_else(|| invalid("providers file \"enabled_providers\" must be an array"))?
                .iter()
                .map(|id| {
                    id.as_str().ok_or_else(|| {
                        invalid("providers file \"enabled_providers\" must contain strings")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let mut providers = json!({});
    for (id, entry) in entries {
        if enabled
            .as_ref()
            .is_some_and(|list| !list.contains(&id.as_str()))
        {
            continue;
        }
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid(&format!("providers file provider {id:?} must be an object")))?;
        let mut converted = Value::Object(entry.clone());
        if let Some(npm) = entry.get("npm") {
            let npm = npm.as_str().ok_or_else(|| {
                invalid(&format!(
                    "providers file provider {id:?} npm must be a string"
                ))
            })?;
            let supported = PACKAGES
                .iter()
                .map(|(source, _)| *source)
                .collect::<Vec<_>>()
                .join(", ");
            let package = PACKAGES
                .iter()
                .find(|(source, _)| *source == npm)
                .map(|(_, target)| *target)
                .ok_or_else(|| {
                    invalid(&format!(
                        "providers file provider {id:?} uses unsupported npm package {npm:?}; supported: {supported}"
                    ))
                })?;
            converted["package"] = json!(package);
        }
        if let Some(options) = entry.get("options") {
            converted["settings"] = options.clone();
        }
        if let Some(models) = entry.get("models").and_then(Value::as_object) {
            let mut converted_models = json!({});
            for (model, definition) in models {
                let mut converted_model = definition.clone();
                if let Some(options) = definition.get("options") {
                    converted_model["settings"] = options.clone();
                }
                if let Some(model) = converted_model.as_object_mut() {
                    model.remove("options");
                }
                converted_models[model] = converted_model;
            }
            converted["models"] = converted_models;
        }
        if let Some(provider) = converted.as_object_mut() {
            provider.remove("npm");
            provider.remove("options");
        }
        providers[id] = converted;
    }
    Ok(providers)
}

/// Strip // and /* */ comments and trailing commas outside string literals,
/// keeping string contents byte-identical.
fn strip_jsonc(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(char) = chars.next() {
        if in_string {
            output.push(char);
            match char {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        output.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => (),
            }
            continue;
        }
        match char {
            '"' => {
                in_string = true;
                output.push(char);
            }
            '/' if chars.peek() == Some(&'/') => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        output.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                output.push(' ');
                while let Some(skipped) = chars.next() {
                    if skipped == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            }
            ',' if chars
                .clone()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| next == '}' || next == ']') => {}
            _ => output.push(char),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_jsonc_keeps_comment_markers_inside_string_literals() {
        let text = r#"{
            // line comment with "quotes" and /* block marker
            "url": "https://example.invalid/v1//double", /* trailing block
                spanning lines */
            "key": "a//b/*c*/d\"e",
            "list": [1, 2, 3,],
            "nested": {"deep": {"value": "comma,in,string"},},
        }"#;
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value["url"], "https://example.invalid/v1//double");
        assert_eq!(value["key"], "a//b/*c*/d\"e");
        assert_eq!(value["list"], json!([1, 2, 3]));
        assert_eq!(value["nested"]["deep"]["value"], "comma,in,string");
    }

    #[test]
    fn strip_jsonc_handles_escapes_and_unterminated_comments() {
        let value: Value =
            serde_json::from_str(&strip_jsonc(r#"{"a": "back\\\"slash", "b": 1,}"#)).unwrap();
        assert_eq!(value["a"], "back\\\"slash");
        assert_eq!(value["b"], 1);
        assert!(serde_json::from_str::<Value>(&strip_jsonc("{\"a\": 1 /* unterminated")).is_err());
    }

    #[test]
    fn home_references_expand_only_for_file_prefixes() {
        let mut value: Value = serde_json::from_str(
            r#"{"p":{"settings":{"apiKey":"{file:~/.secrets/key}","other":"{env:~x}",
            "nested":[{"deep":"{file:~/s}"}]},"models":{"m":{"settings":{"limit":{"output":8}}}}}}"#,
        )
        .unwrap();
        expand_references(&mut value, "/users/u");
        assert_eq!(
            value["p"]["settings"]["apiKey"],
            "{file:/users/u/.secrets/key}"
        );
        assert_eq!(value["p"]["settings"]["other"], "{env:~x}");
        assert_eq!(
            value["p"]["settings"]["nested"][0]["deep"],
            "{file:/users/u/s}"
        );
        assert_eq!(value["p"]["models"]["m"]["settings"]["limit"]["output"], 8);
        // Without a home the references stay untouched.
        let mut value = json!({"p":{"settings":{"apiKey":"{file:~/.secrets/key}"}}});
        expand_references(&mut value, "");
        assert_eq!(value["p"]["settings"]["apiKey"], "{file:~/.secrets/key}");
    }
}
