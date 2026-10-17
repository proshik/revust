use revust_core::{
    config::{
        paths::Layers,
        runtime::{Invocation, RuntimeConfig},
    },
    prompts::{PromptSet, template},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "revust-core-contract-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn put(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn winning_runtime_values_are_typed_after_precedence_but_all_keys_are_checked() {
    let t = Tree::new();
    t.put(
        "user/config",
        "max-parallel = not-an-integer\nprofile = \"caller\\x2dprofile\"\n",
    );
    t.put(".revust/config", "MaxParallel = 0\nmax-parallel = 8\n");
    let inv = Invocation {
        config_dir: Some(t.0.join("user")),
        ..Default::default()
    };
    let resolved = serde_json::to_value(RuntimeConfig::resolve(&t.0, None, &inv).unwrap()).unwrap();
    let knob = |name| {
        resolved["knobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == name)
            .unwrap()
    };
    assert_eq!(knob("max-parallel")["value"], 0);
    assert_eq!(knob("max-parallel")["source"], "project");
    assert_eq!(knob("profile")["value"], "caller-profile");
    t.put("user/config", "unknown-runtime-key = 1\n");
    assert!(
        RuntimeConfig::resolve(&t.0, None, &inv)
            .unwrap_err()
            .contains("unknown-runtime-key")
    );
}
#[test]
fn winning_document_is_parsed_once_and_raw_content_is_not_normalized() {
    let t = Tree::new();
    t.put(
        "user/prompts/profiles/custom.md.md",
        "---\ninvalid: [\n---\n",
    );
    let raw = "---\r\nmodel: codex/fixture:high\r\nagents:\r\n  - name: observer\r\n    lenses: [probe]\r\n---\r\n  Input {{SCOPE}}\r\n";
    t.put("project/prompts/profiles/custom.md.md", raw);
    t.put("project/lenses/probe.md", "Probe {{WORKDIR}}\n");
    let set = PromptSet::load(&Layers {
        user: t.0.join("user"),
        project: Some(t.0.join("project")),
    })
    .unwrap();
    assert_eq!(set.content("prompts/profiles/custom.md.md"), Some(raw));
    let agents = set.roster("custom.md", &[]).unwrap();
    assert_eq!(agents[0].runner.model, "fixture");
    let vars = BTreeMap::from([
        ("SCOPE".into(), "literal {{WORKDIR}}".into()),
        ("WORKDIR".into(), "/project".into()),
    ]);
    assert_eq!(
        set.compose_agent("custom.md", &agents[0], &vars, "")
            .unwrap(),
        "Input literal {{WORKDIR}}\n\nProbe /project"
    );
}
#[test]
fn placeholder_grammar_keeps_nonidentifiers_and_inserted_text_literal() {
    let values = BTreeMap::from([("SCOPE".into(), "{{GOAL}} λ".into())]);
    let body = "λ {{{SCOPE}}} {{ spaced }} {{SCOPE}} {{é}}";
    assert_eq!(
        template::render(body, &values, "").unwrap(),
        "λ {{{GOAL}} λ} {{ spaced }} {{GOAL}} λ {{é}}"
    );
    assert!(template::validate("{{UNKNOWN}}").is_err());
    assert!(template::render("{{GOAL}}", &values, "").is_err());
    let rendered = template::render("{{SCOPE}}", &values, "history {{SCOPE}}").unwrap();
    assert_eq!(rendered.matches("history {{SCOPE}}").count(), 1);
    let _: Value = json!({"result":rendered});
}
