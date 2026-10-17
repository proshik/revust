//! Strict adjacent configuration; parsing never launches a provider or writes files.
mod profiles;
mod write;
pub use write::Edit;
mod resolve;
pub use profiles::{ExtendedProfile, ExtensionSet};
mod schema;
use crate::{
    config::{
        paths::Layers,
        runtime::{Invocation, RuntimeConfig},
    },
    prompts::PromptSet,
};
pub use resolve::{EffectivePolicy, Layer, LayerKind};
use serde_json::Value;
use std::path::Path;

pub struct Document {
    value: Value,
}
impl Document {
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("extensions JSON: {e}"))?;
        schema::document().validate(&value, "$")?;
        Ok(Self { value })
    }
    pub fn policy(&self, profile: Option<&str>) -> Value {
        let mut policy = self
            .value
            .get("policy")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(overlay) =
            profile.and_then(|name| self.value.get("profiles")?.get(name)?.get("policy"))
        {
            merge(&mut policy, overlay);
        }
        policy
    }
}

/// Arrays and scalars replace; null remains an explicit removal marker.
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

pub fn json_schema() -> Value {
    let mut schema = schema::document().json();
    schema["$schema"] = Value::String("https://json-schema.org/draft/2020-12/schema".into());
    schema["title"] = Value::String("revust extensions version 1".into());
    schema
}

pub fn inspect(cwd: &Path, home: Option<&Path>, invocation: &Invocation) -> Result<Value, String> {
    inspect_with_policy(cwd, home, invocation, &serde_json::json!({}), None)
}

pub fn inspect_with_policy(
    cwd: &Path,
    home: Option<&Path>,
    invocation: &Invocation,
    cli: &Value,
    min_confidence: Option<i64>,
) -> Result<Value, String> {
    let runtime = RuntimeConfig::resolve(cwd, home, invocation)?;
    let layers = Layers {
        user: runtime.paths.config_dir.clone(),
        project: runtime.paths.project_dir.clone(),
    };
    let prompts = PromptSet::load(&layers)?;
    let extensions = ExtensionSet::load(&layers, &prompts)?;
    let profile = runtime
        .knobs
        .iter()
        .find_map(|k| match (k.name, &k.value) {
            ("profile", crate::config::runtime::Value::Text(value)) => Some(value.as_str()),
            _ => None,
        })
        .expect("profile knob");
    let selected = if let Some(profile) = extensions.profiles.get(profile) {
        let stages=crate::prompts::runner::STAGES.iter().map(|name| {
            Ok(serde_json::json!({"name":name,"runner":prompts.stage_runner(&profile.runner,name,profile.stages.get(*name))?}))
        }).collect::<Result<Vec<_>,String>>()?;
        serde_json::json!({"name":profile.name,"description":profile.description,"runner":profile.runner,"roster":profile.agents,"stages":stages,"source":profile.source})
    } else {
        let catalogue = serde_json::to_value(prompts.catalog()).map_err(|e| e.to_string())?;
        catalogue["profiles"]
            .as_array()
            .expect("profile catalogue")
            .iter()
            .find(|p| p["name"] == profile)
            .cloned()
            .ok_or_else(|| format!("unknown profile {profile:?}"))?
    };
    let mut policy = EffectivePolicy::resolve(&runtime, &extensions.layers, profile, cli)?;
    if let Some(value) = min_confidence {
        policy.set_legacy_min_confidence(value);
    }
    Ok(
        serde_json::json!({"reference_revision":"revust-contract-v1",
        "legacy":runtime,"selected_profile":selected,"policy":policy.inspection(),
        "extension_profiles":extensions.profiles.keys().collect::<Vec<_>>(),"prompt_files":prompts.provenance()}),
    )
}
