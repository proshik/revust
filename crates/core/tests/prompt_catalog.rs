use revust_core::{config::paths::Layers, prompts::PromptSet};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "revust-prompts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn put(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

#[test]
fn composed_bodies_use_winning_files_lens_order_and_literal_insertions() {
    use std::collections::BTreeMap;
    let tree = Tree::new();
    tree.put(
        "user/prompts/profiles/synthetic.md",
        "---\nmalformed: [\n---\n",
    );
    tree.put("project/prompts/profiles/synthetic.md", "---\nmodel: codex/synthetic:high\nagents:\n  - name: observer\n    lenses: [first, second]\n---\nFIND {{SCOPE}}\n");
    tree.put("user/lenses/first.md", "USER FIRST\n");
    tree.put("project/lenses/first.md", "FIRST {{SCOPE}}\n");
    tree.put("user/lenses/second.md", "SECOND {{WORKDIR}}\n");
    tree.put(
        "project/prompts/synthesis.md",
        "---\nmodel: claude/synthesis:low\n---\nSYNTHESIS {{FINDINGS}}\n",
    );
    let set = PromptSet::load(&Layers {
        user: tree.0.join("user"),
        project: Some(tree.0.join("project")),
    })
    .unwrap();
    let vars = BTreeMap::from([
        ("SCOPE".into(), "/synthetic/scope.md".into()),
        ("WORKDIR".into(), "/synthetic/work".into()),
        ("FINDINGS".into(), "literal {{message}}".into()),
    ]);
    let roster = set.roster("synthetic", &[]).unwrap();
    assert_eq!(
        set.compose_agent("synthetic", &roster[0], &vars, "")
            .unwrap(),
        "FIND /synthetic/scope.md\n\nFIRST /synthetic/scope.md\n\nSECOND /synthetic/work"
    );
    let override_roster = set
        .roster("synthetic", &["second".into(), "first".into()])
        .unwrap();
    assert_eq!(override_roster.len(), 1);
    assert_eq!(override_roster[0].name, "lenses");
    assert_eq!(override_roster[0].runner.executor, "codex");
    assert_eq!(override_roster[0].runner.model, "synthetic");
    assert_eq!(
        set.compose_agent("synthetic", &override_roster[0], &vars, "")
            .unwrap(),
        "FIND /synthetic/scope.md\n\nSECOND /synthetic/work\n\nFIRST /synthetic/scope.md"
    );
    let (runner, text) = set
        .compose_stage("synthetic", "synthesis", &vars, "")
        .unwrap();
    assert_eq!(runner.executor, "claude");
    assert_eq!(runner.model, "synthesis");
    assert_eq!(runner.effort, "low");
    assert_eq!(text, "SYNTHESIS literal {{message}}");
    assert!(
        set.roster("synthetic", &["missing".into()])
            .unwrap_err()
            .contains("unknown lens")
    );
    assert!(
        set.roster("missing", &[])
            .unwrap_err()
            .contains("unknown profile")
    );
    assert_eq!(set.content("lenses/first.md"), Some("FIRST {{SCOPE}}\n"));
    let origins = set.provenance();
    let first = origins
        .iter()
        .find(|f| f.path == "lenses/first.md")
        .unwrap();
    assert_eq!(first.layer, "project");
    assert_eq!(
        first.source.as_ref(),
        Some(&tree.0.join("project/lenses/first.md"))
    );
    assert_eq!(
        origins
            .iter()
            .find(|f| f.path == "lenses/second.md")
            .unwrap()
            .layer,
        "user"
    );
    assert_eq!(
        origins
            .iter()
            .find(|f| f.path == "lenses/bugs.md")
            .unwrap()
            .layer,
        "embedded"
    );
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn profile_metadata_is_strict_but_shadowed_documents_do_not_fail_loading() {
    let tree = Tree::new();
    for metadata in [
        "unknown: field",
        "agents: false",
        "model: unknown/model",
        "agents: [{name: a, lenses: [missing]}]",
    ] {
        tree.put(
            "user/prompts/profiles/check.md",
            &format!("---\n{metadata}\n---\nInspect {{SCOPE}}"),
        );
        let layers = Layers {
            user: tree.0.join("user"),
            project: None,
        };
        assert!(PromptSet::load(&layers).is_err(), "{metadata}");
    }
    tree.put("project/prompts/profiles/check.md", "---\nmodel: codex/authored:high\nagents: [{name: one, lenses: [bugs]}]\n---\nInspect {{SCOPE}}\n");
    let set = PromptSet::load(&Layers {
        user: tree.0.join("user"),
        project: Some(tree.0.join("project")),
    })
    .unwrap();
    let roster = set.roster("check", &[]).unwrap();
    assert_eq!(roster[0].name, "one");
    assert_eq!(roster[0].runner.model, "authored");
    assert_eq!(roster[0].lenses, ["bugs"]);
    assert!(
        set.content("prompts/profiles/check.md")
            .unwrap()
            .contains("authored:high")
    );
}
