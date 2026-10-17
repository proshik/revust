use crate::config::duration::LegacyDuration;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) enum Rule {
    Bool,
    Text,
    EnvValue,
    Uint(u64, u64),
    Number(f64),
    Choice(Vec<&'static str>),
    Duration(bool),
    Nullable(Box<Rule>),
    List(Box<Rule>),
    Object(BTreeMap<&'static str, Rule>, Vec<&'static str>),
    Map(Box<Rule>),
    Conditions,
    Version,
}
fn object(fields: impl IntoIterator<Item = (&'static str, Rule)>) -> Rule {
    Rule::Object(fields.into_iter().collect(), vec![])
}
fn choice(values: &[&'static str]) -> Rule {
    Rule::Choice(values.to_vec())
}
fn positive() -> Rule {
    Rule::Uint(1, u64::MAX)
}
fn list(rule: Rule) -> Rule {
    Rule::List(Box::new(rule))
}
fn nullable(rule: Rule) -> Rule {
    Rule::Nullable(Box::new(rule))
}
fn stage(enabled: bool) -> Rule {
    let mut fields = vec![
        ("idle_timeout", Rule::Duration(true)),
        ("hard_timeout", Rule::Duration(true)),
        ("parallelism", positive()),
    ];
    if enabled {
        fields.push(("enabled", Rule::Bool));
    }
    object(fields)
}
fn retry() -> Rule {
    object([
        ("count", Rule::Uint(0, u32::MAX.into())),
        ("conditions", Rule::Conditions),
        (
            "delay",
            object([
                ("base", Rule::Duration(true)),
                ("factor", Rule::Number(1.0)),
                ("max", nullable(Rule::Duration(true))),
                ("jitter", Rule::Duration(true)),
            ]),
        ),
    ])
}
fn executor(options: Rule) -> Rule {
    object([
        ("executable", Rule::Text),
        ("env", Rule::Map(Box::new(Rule::EnvValue))),
        ("options", options),
    ])
}
fn sandbox() -> Rule {
    choice(&["read-only", "workspace-write", "danger-full-access"])
}
pub(super) fn policy() -> Rule {
    object([
        (
            "runtime",
            object([
                ("idle_timeout", Rule::Duration(true)),
                ("hard_timeout", Rule::Duration(true)),
                ("stagger_delay", Rule::Duration(true)),
                ("max_parallel", positive()),
                ("verify_groups", positive()),
                ("verify_group_by", choice(&["dir", "source"])),
                ("codex_sandbox", sandbox()),
            ]),
        ),
        (
            "stages",
            object([
                ("find", stage(false)),
                ("synthesis", stage(true)),
                ("verify", stage(true)),
            ]),
        ),
        (
            "retries",
            object([
                ("find", retry()),
                ("synthesis", retry()),
                ("verify", retry()),
            ]),
        ),
        (
            "verification",
            object([
                ("min_directory_group_size", positive()),
                ("min_confidence", Rule::Uint(0, 100)),
            ]),
        ),
        (
            "budget",
            object([
                ("max_calls", nullable(positive())),
                ("elapsed", nullable(Rule::Duration(false))),
            ]),
        ),
        (
            "completeness",
            object([
                ("required_reviewers", list(Rule::Text)),
                ("min_successful_reviewers", Rule::Uint(0, u64::MAX)),
                ("require_synthesis_success", Rule::Bool),
                ("require_all_verified", Rule::Bool),
                (
                    "synthesis_failure",
                    choice(&["legacy", "fail", "passthrough"]),
                ),
            ]),
        ),
        (
            "executors",
            object([
                (
                    "claude",
                    executor(object([("setting_sources", list(choice(&["project"])))])),
                ),
                ("codex", executor(object([("sandbox", sandbox())]))),
                (
                    "opencode",
                    executor(object([
                        ("agent", Rule::Text),
                        ("session_mode", choice(&["isolated"])),
                    ])),
                ),
            ]),
        ),
        (
            "presentation",
            object([
                ("host", Rule::Text),
                ("mode", choice(&["auto", "tui", "headless"])),
                (
                    "geometry",
                    object([("columns", positive()), ("rows", positive())]),
                ),
                ("unavailable", choice(&["error", "terminal", "headless"])),
            ]),
        ),
        (
            "output",
            object([
                ("tui", Rule::Bool),
                ("format", choice(&["json", "markdown"])),
                ("auto_exit", Rule::Duration(true)),
            ]),
        ),
    ])
}
pub(super) fn document() -> Rule {
    Rule::Object(
        [
            ("version", Rule::Version),
            ("policy", policy()),
            (
                "profiles",
                Rule::Map(Box::new(object([
                    ("file", Rule::Text),
                    ("policy", policy()),
                ]))),
            ),
        ]
        .into_iter()
        .collect(),
        vec!["version"],
    )
}
const CONDITIONS: &[&str] = &[
    "process_error",
    "idle_timeout",
    "hard_timeout",
    "rate_limit",
    "nonzero_exit",
    "missing_output",
    "invalid_output",
];

impl Rule {
    pub(super) fn validate(&self, value: &Value, path: &str) -> Result<(), String> {
        let invalid = || format!("{path}: invalid value or type");
        match self {
            Self::EnvValue if value.as_str().is_some_and(|s| !s.contains('\0')) => Ok(()),
            Self::Bool if value.is_boolean() => Ok(()),
            Self::Text
                if value
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && !s.contains('\0')) =>
            {
                Ok(())
            }
            Self::Uint(min, max) if value.as_u64().is_some_and(|v| v >= *min && v <= *max) => {
                Ok(())
            }
            Self::Number(min) if value.as_f64().is_some_and(|v| v.is_finite() && v >= *min) => {
                Ok(())
            }
            Self::Choice(choices) if value.as_str().is_some_and(|s| choices.contains(&s)) => Ok(()),
            Self::Duration(zero)
                if value
                    .as_str()
                    .and_then(|s| s.parse::<LegacyDuration>().ok())
                    .is_some_and(|d| d.0 > 0 || (*zero && d.0 == 0)) =>
            {
                Ok(())
            }
            Self::Nullable(_) if value.is_null() => Ok(()),
            Self::Nullable(rule) => rule.validate(value, path),
            Self::List(rule) => {
                for (i, item) in value.as_array().ok_or_else(invalid)?.iter().enumerate() {
                    rule.validate(item, &format!("{path}[{i}]"))?;
                }
                Ok(())
            }
            Self::Map(rule) => {
                for (key, item) in value.as_object().ok_or_else(invalid)? {
                    if key.is_empty() || key.contains(['\0', '=']) {
                        return Err(format!("{path}: invalid map key"));
                    }
                    rule.validate(item, &format!("{path}.{key}"))?;
                }
                Ok(())
            }
            Self::Object(fields, required) => {
                let values = value.as_object().ok_or_else(invalid)?;
                for key in required {
                    if !values.contains_key(*key) {
                        return Err(format!("{path}.{key}: required field"));
                    }
                }
                for (key, item) in values {
                    fields
                        .get(key.as_str())
                        .ok_or_else(|| format!("{path}.{key}: unknown field"))?
                        .validate(item, &format!("{path}.{key}"))?;
                }
                Ok(())
            }
            Self::Conditions if value == "legacy" => Ok(()),
            Self::Conditions => list(choice(CONDITIONS)).validate(value, path),
            Self::Version if value.as_u64() == Some(1) => Ok(()),
            _ => Err(invalid()),
        }
    }
    pub(super) fn json(&self) -> Value {
        match self {
            Self::EnvValue => json!({"type":"string","pattern":"^[^\\u0000]*$"}),
            Self::Bool => json!({"type":"boolean"}),
            Self::Text => json!({"type":"string","minLength":1,"pattern":"^[^\\u0000]*$"}),
            Self::Uint(min, max) => json!({"type":"integer","minimum":min,"maximum":max}),
            Self::Number(min) => json!({"type":"number","minimum":min}),
            Self::Choice(choices) => json!({"type":"string","enum":choices}),
            Self::Duration(zero) => {
                json!({"type":"string","format":"go-duration","x-allow-zero":zero,"x-minimum-nanoseconds":if *zero {0} else {1}})
            }
            Self::Nullable(rule) => json!({"anyOf":[rule.json(), {"type":"null"}]}),
            Self::List(rule) => json!({"type":"array","items":rule.json()}),
            Self::Map(rule) => {
                json!({"type":"object","additionalProperties":rule.json(),"propertyNames":{"minLength":1,"pattern":"^[^=\\u0000]*$"}})
            }
            Self::Object(fields, required) => {
                json!({"type":"object","additionalProperties":false,"required":required,"properties":fields.iter().map(|(name,rule)| (*name,rule.json())).collect::<BTreeMap<_,_>>()})
            }
            Self::Conditions => {
                json!({"anyOf":[{"const":"legacy"},list(choice(CONDITIONS)).json()]})
            }
            Self::Version => json!({"const":1,"type":"integer"}),
        }
    }
}
