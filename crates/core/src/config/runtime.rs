use super::{duration::LegacyDuration, ini, paths};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Invocation {
    pub config_dir: Option<PathBuf>,
    pub workdir: Option<PathBuf>,
    pub overrides: Vec<(String, String)>,
    pub task: String,
    pub run: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Default,
    User,
    Project,
    Flag,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Value {
    Integer(i64),
    Text(String),
    Duration(LegacyDuration),
}

#[derive(Debug, Serialize)]
pub struct Knob {
    pub name: &'static str,
    pub value: Value,
    pub source: Origin,
    #[serde(skip)]
    pub(crate) value_source: Origin,
}

#[derive(Debug, Serialize)]
pub struct RuntimePaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<PathBuf>,
    pub tasks_dir: PathBuf,
    pub config_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<PathBuf>,
    pub workdir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_fallback: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_fallback_error: Option<String>,
}

/// The runtime subset of inspection, not the profile/task catalogue.
#[derive(Debug, Serialize)]
pub struct RuntimeConfig {
    pub knobs: Vec<Knob>,
    pub paths: RuntimePaths,
}

enum Kind {
    Duration,
    Integer,
    Text,
    Choice(&'static [&'static str]),
}
struct Definition {
    name: &'static str,
    alias: &'static str,
    default: &'static str,
    kind: Kind,
}

const DEFINITIONS: &[Definition] = &[
    Definition {
        name: "idle-timeout",
        alias: "IdleTimeout",
        default: "2m",
        kind: Kind::Duration,
    },
    Definition {
        name: "hard-timeout",
        alias: "HardTimeout",
        default: "20m",
        kind: Kind::Duration,
    },
    Definition {
        name: "stagger-delay",
        alias: "StaggerDelay",
        default: "30s",
        kind: Kind::Duration,
    },
    Definition {
        name: "max-parallel",
        alias: "MaxParallel",
        default: "4",
        kind: Kind::Integer,
    },
    Definition {
        name: "verify-groups",
        alias: "VerifyGroups",
        default: "6",
        kind: Kind::Integer,
    },
    Definition {
        name: "verify-group-by",
        alias: "VerifyGroupBy",
        default: "dir",
        kind: Kind::Choice(&["dir", "source"]),
    },
    Definition {
        name: "tasks-dir",
        alias: "TasksDir",
        default: "./.revust/tasks",
        kind: Kind::Text,
    },
    Definition {
        name: "auto-exit",
        alias: "AutoExit",
        default: "0s",
        kind: Kind::Duration,
    },
    Definition {
        name: "profile",
        alias: "Profile",
        default: "comprehensive",
        kind: Kind::Text,
    },
    Definition {
        name: "codex-sandbox",
        alias: "CodexSandbox",
        default: "read-only",
        kind: Kind::Choice(&["read-only", "workspace-write", "danger-full-access"]),
    },
];

/// A parsed assignment keeps location and layer until precedence is decided.
struct Assignment {
    field: usize,
    raw_key: String,
    value: String,
    origin: Origin,
    location: Option<(PathBuf, usize)>,
}
impl Assignment {
    fn error(&self, message: String) -> String {
        match &self.location {
            Some((path, line)) => format!("parse {}:{line}: {message}", path.display()),
            None => message,
        }
    }
}

impl RuntimeConfig {
    pub fn resolve(
        cwd: &Path,
        home: Option<&Path>,
        invocation: &Invocation,
    ) -> Result<Self, String> {
        let roots = paths::Layers::resolve(cwd, home, invocation.config_dir.as_deref())
            .map_err(|error| error.to_string())?;
        let mut assignments = Vec::new();
        // Reversing flags gives last occurrence precedence. Every flag is validated,
        // even if another occurrence will win; this also serves inspection commands.
        for (key, value) in invocation.overrides.iter().rev() {
            validate_option(key, value)?;
            assignments.push(Assignment {
                field: definition_index(key, false).expect("validated option"),
                raw_key: key.clone(),
                value: value.clone(),
                origin: Origin::Flag,
                location: None,
            });
        }
        let directories = roots
            .project
            .iter()
            .map(|p| (p, Origin::Project))
            .chain(std::iter::once((&roots.user, Origin::User)));
        for (directory, origin) in directories {
            let path = directory.join("config");
            for entry in ini::read(&path)? {
                let field = definition_index(&entry.key, true).ok_or_else(|| {
                    format!(
                        "parse {}:{}: unknown option: {}",
                        path.display(),
                        entry.line,
                        entry.key
                    )
                })?;
                assignments.push(Assignment {
                    field,
                    raw_key: entry.key,
                    value: entry.value,
                    origin,
                    location: Some((path.clone(), entry.line)),
                });
            }
        }
        // Public diagnostics historically identify a raw canonical key separately
        // from field-name aliases. Policy resolution uses the actual winning layer.
        let mut reported = BTreeMap::new();
        for assignment in &assignments {
            reported
                .entry(assignment.raw_key.to_ascii_lowercase())
                .or_insert(assignment.origin);
        }
        let knobs = DEFINITIONS
            .iter()
            .enumerate()
            .map(|(field, definition)| {
                let winner = assignments
                    .iter()
                    .find(|assignment| assignment.field == field);
                let value = definition
                    .parse_value(winner.map_or(definition.default, |a| a.value.as_str()))
                    .map_err(|error| winner.map_or(error.clone(), |a| a.error(error)))?;
                Ok(Knob {
                    name: definition.name,
                    value,
                    source: reported
                        .get(definition.name)
                        .copied()
                        .unwrap_or(Origin::Default),
                    value_source: winner.map_or(Origin::Default, |a| a.origin),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let tasks_text = knobs
            .iter()
            .find_map(|knob| match &knob.value {
                Value::Text(text) if knob.name == "tasks-dir" => Some(text),
                _ => None,
            })
            .expect("tasks-dir catalog entry");
        let tasks_dir =
            paths::absolute(cwd, Path::new(tasks_text)).map_err(|error| error.to_string())?;
        let archive = match (
            safe_identifier(&invocation.task),
            safe_identifier(&invocation.run),
        ) {
            (true, true) if invocation.run != "task.md" => {
                Some(tasks_dir.join(&invocation.task).join(&invocation.run))
            }
            _ => None,
        };
        let (workdir, workdir_error) = match paths::work_dir(cwd, invocation.workdir.as_deref()) {
            Ok(directory) => (directory, None),
            Err(error) => {
                let input = invocation.workdir.clone().unwrap_or_default();
                let diagnostic = format!("workdir {}: {error}", input.display());
                (input, Some(diagnostic))
            }
        };
        let (profile_fallback, profile_fallback_error) = match paths::profile_fallback(cwd) {
            Ok(profile) => (profile, None),
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(Self {
            knobs,
            paths: RuntimePaths {
                archive,
                tasks_dir,
                config_dir: roots.user,
                project_dir: roots.project,
                workdir,
                workdir_error,
                profile_fallback,
                profile_fallback_error,
            },
        })
    }
}
fn safe_identifier(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('.')
        && !text.contains(['/', '\\'])
        && !text.contains("..")
}
fn definition_index(key: &str, ini: bool) -> Option<usize> {
    DEFINITIONS.iter().position(|d| {
        d.name == key || (ini && (d.alias == key || d.name.eq_ignore_ascii_case(key)))
    })
}
impl Definition {
    fn parse_value(&self, text: &str) -> Result<Value, String> {
        match self.kind {
            Kind::Integer => parse_integer(text).map(Value::Integer),
            Kind::Duration => text.parse().map(Value::Duration),
            Kind::Text => Ok(Value::Text(text.to_owned())),
            Kind::Choice(allowed) => {
                if allowed.contains(&text) {
                    Ok(Value::Text(text.to_owned()))
                } else {
                    Err(format!(
                        "Invalid value {text:?} for option --{}. Allowed values: {}",
                        self.name,
                        allowed.join(", ")
                    ))
                }
            }
        }
    }
}
pub fn validate_option(name: &str, value: &str) -> Result<(), String> {
    let index = definition_index(name, false).ok_or_else(|| format!("unknown option: {name}"))?;
    DEFINITIONS[index].parse_value(value).map(|_| ())
}

/// Checked accumulation supports signed decimal, octal and explicit base prefixes.
pub fn parse_integer(input: &str) -> Result<i64, String> {
    let syntax = || format!("parsing {input:?}: invalid syntax");
    let overflow = || format!("parsing {input:?}: value out of range");
    let negative = input.starts_with('-');
    let unsigned = input.strip_prefix(['-', '+']).unwrap_or(input);
    let (radix, offset) = match unsigned.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, 2),
        [b'0', b'o' | b'O', ..] => (8, 2),
        [b'0', b'b' | b'B', ..] => (2, 2),
        [b'0', _, ..] => (8, 1),
        _ => (10, 0),
    };
    let digits = &unsigned[offset..];
    let digits = if offset > 0 {
        digits.strip_prefix('_').unwrap_or(digits)
    } else {
        digits
    };
    if digits.is_empty() {
        return Err(syntax());
    }
    let mut previous_digit = false;
    let mut magnitude = Some(0_u64);
    for character in digits.chars() {
        if character == '_' {
            if !previous_digit {
                return Err(syntax());
            }
            previous_digit = false;
            continue;
        }
        let digit = character
            .is_ascii()
            .then(|| character.to_digit(radix))
            .flatten()
            .ok_or_else(syntax)?;
        previous_digit = true;
        magnitude = magnitude
            .and_then(|number| number.checked_mul(u64::from(radix)))
            .and_then(|number| number.checked_add(u64::from(digit)));
    }
    if !previous_digit {
        return Err(syntax());
    }
    let magnitude = magnitude.ok_or_else(overflow)?;
    let limit = i64::MAX as u64 + u64::from(negative);
    if magnitude > limit {
        return Err(overflow());
    }
    if negative {
        Ok((-(i128::from(magnitude))) as i64)
    } else {
        Ok(magnitude as i64)
    }
}
