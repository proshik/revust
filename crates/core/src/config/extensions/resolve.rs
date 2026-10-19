use super::Document;
use crate::config::runtime::{Origin, RuntimeConfig};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    User,
    Project,
}
pub struct Layer {
    pub directory: PathBuf,
    pub kind: LayerKind,
    pub document: Document,
}
pub struct EffectivePolicy {
    values: Value,
    origins: BTreeMap<String, Value>,
}
impl EffectivePolicy {
    pub fn resolve(
        runtime: &RuntimeConfig,
        layers: &[Layer],
        profile: &str,
        cli: &Value,
    ) -> Result<Self, String> {
        super::schema::policy().validate(cli, "$cli")?;
        let mut result = Self {
            values: json!({}),
            origins: BTreeMap::new(),
        };
        let mut retries = json!({});
        for (stage, count, delay) in [
            ("find", 1, "5s"),
            ("synthesis", 1, "0s"),
            ("verify", 0, "0s"),
        ] {
            retries[stage] = json!({"count":count,"conditions":"legacy","delay":{
                "base":delay,"factor":1.0,"max":null,"jitter":delay}});
        }
        result.apply(&json!({
            "runtime":{},"stages":{"find":{},"synthesis":{"enabled":true,"parallelism":1},"verify":{"enabled":true}},
            "retries":retries,"verification":{"min_directory_group_size":2,"min_confidence":0},
            "budget":{"max_calls":null,"elapsed":null},
            "completeness":{"required_reviewers":[],"min_successful_reviewers":0,
                "require_synthesis_success":false,"require_all_verified":false,"synthesis_failure":"legacy"},
            "executors":{"claude":{"executable":"claude","env":{}},"codex":{"executable":"codex","env":{}},
                "opencode":{"executable":"opencode","env":{},"options":{"session_mode":"isolated","providers_file":""}}},
            "presentation":{"host":"auto","mode":"auto","geometry":{},"unavailable":"error"},
            "output":{"tui":true,"format":"json"}
        }), &json!({"layer":"default","kind":"legacy"}));
        for (origin, kind, name) in [
            (Origin::Default, None, "default"),
            (Origin::User, Some(LayerKind::User), "user"),
            (Origin::Project, Some(LayerKind::Project), "project"),
            (Origin::Flag, None, "flag"),
        ] {
            let mut source = json!({"layer":name,"kind":"legacy"});
            if origin == Origin::User {
                source["source"] = json!(runtime.paths.config_dir.join("config"));
            }
            if origin == Origin::Project {
                source["source"] =
                    json!(runtime.paths.project_dir.as_ref().map(|p| p.join("config")));
            }
            for knob in runtime.knobs.iter().filter(|k| k.value_source == origin) {
                let value = serde_json::to_value(&knob.value).map_err(|e| e.to_string())?;
                match knob.name {
                    "profile" | "tasks-dir" => (),
                    "auto-exit" => result.apply(&json!({"output":{"auto_exit":value}}), &source),
                    key => result.apply(&json!({"runtime":{key.replace('-',"_"):value}}), &source),
                }
            }
            let Some(kind) = kind else {
                continue;
            };
            let matching: Vec<_> = layers.iter().filter(|layer| layer.kind == kind).collect();
            if matching.len() > 1 {
                return Err(format!("duplicate {name} extension layer"));
            }
            for layer in matching {
                let mut source = json!({"layer":name,"kind":"extension","source":layer.directory.join("extensions.json")});
                if let Some(policy) = layer.document.value.get("policy") {
                    result.apply(policy, &source);
                }
                if let Some(policy) = layer
                    .document
                    .value
                    .get("profiles")
                    .and_then(|p| p.get(profile))
                    .and_then(|p| p.get("policy"))
                {
                    source["profile"] = json!(profile);
                    result.apply(policy, &source);
                }
            }
        }
        result.apply(cli, &json!({"layer":"flag","kind":"extension"}));
        Ok(result)
    }
    /// The legacy CLI accepts any signed integer; extension documents retain
    /// their version-1 0..100 validation. Apply this explicit flag last.
    pub(crate) fn set_legacy_min_confidence(&mut self, value: i64) {
        self.apply(
            &json!({"verification":{"min_confidence":value}}),
            &json!({"layer":"flag","kind":"legacy"}),
        );
    }
    pub fn inspection(&self) -> Value {
        let mut values = self.values.clone();
        if let Some(executors) = values.get_mut("executors").and_then(Value::as_object_mut) {
            for executor in executors.values_mut() {
                if let Some(env) = executor.get_mut("env").and_then(Value::as_object_mut) {
                    for value in env.values_mut() {
                        *value = json!("<redacted>");
                    }
                }
            }
        }
        json!({"values":values,"origins":self.origins})
    }

    /// Raw values are for child execution only. Use inspection() for user output.
    pub fn execution_values(&self) -> &Value {
        &self.values
    }

    fn apply(&mut self, patch: &Value, source: &Value) {
        // Expand runtime-wide values first so a stage override in the same
        // document refines it, while a later layer can replace both.
        if let Some(runtime) = patch.get("runtime").and_then(Value::as_object) {
            for (key, value) in runtime {
                if key == "codex_sandbox" {
                    self.apply_leaves(
                        &json!({"executors":{"codex":{"options":{"sandbox":value}}}}),
                        source,
                    );
                }
                let field = match key.as_str() {
                    "idle_timeout" | "hard_timeout" => Some(key.as_str()),
                    "max_parallel" => Some("parallelism"),
                    _ => None,
                };
                if let Some(field) = field {
                    for stage in ["find", "synthesis", "verify"] {
                        if field == "parallelism" && stage == "synthesis" {
                            continue;
                        }
                        self.apply_leaves(&json!({"stages":{stage:{field:value}}}), source);
                    }
                }
            }
        }
        self.apply_leaves(patch, source);
    }

    fn apply_leaves(&mut self, patch: &Value, source: &Value) {
        fn visit(
            base: &mut Value,
            patch: &Value,
            path: &str,
            source: &Value,
            origins: &mut BTreeMap<String, Value>,
        ) {
            if let Value::Object(entries) = patch {
                if !base.is_object() {
                    *base = json!({});
                }
                for (key, value) in entries {
                    let path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                    visit(
                        base.as_object_mut()
                            .unwrap()
                            .entry(key)
                            .or_insert(Value::Null),
                        value,
                        &path,
                        source,
                        origins,
                    );
                }
            } else {
                *base = patch.clone();
                origins.insert(path.to_owned(), source.clone());
            }
        }
        visit(&mut self.values, patch, "", source, &mut self.origins);
    }
}
