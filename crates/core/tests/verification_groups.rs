use revust_core::runtime::{data::Finding, groups::group};
fn finding(file: &str, source: &str) -> Finding {
    Finding {
        file: file.into(),
        sources: vec![source.into()],
        ..Finding::default()
    }
}
#[test]
fn directory_thin_merge_and_limit_preserve_whole_buckets_and_source_mode_keeps_singletons() {
    let inputs = vec![
        finding("a/one", "first"),
        finding("a/two", "second"),
        finding("b/one", "first"),
        finding("c/one", "second"),
        finding("d/one", "third"),
        finding("d/two", "third"),
    ];
    let groups = group(&inputs, false, 2, None);
    assert_eq!(
        groups.iter().map(|g| g.keys.clone()).collect::<Vec<_>>(),
        vec![vec!["a"], vec!["d"], vec!["b", "c"]]
    );
    let capped = group(&inputs, false, 2, Some(2));
    assert_eq!(
        capped.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        vec!["a+d", "b+c"]
    );
    assert_eq!(capped[0].findings.len(), 4);
    assert_eq!(group(&inputs, true, 100, None).len(), 3);
    assert_eq!(group(&inputs, false, 1, None).len(), 4);
}
#[test]
fn labels_cannot_collide_or_create_paths_and_directory_keys_follow_posix_cleaning() {
    let groups = group(
        &[
            finding("foo/bar/a", "a"),
            finding("foo-bar/a", "a"),
            finding("foo-bar-2/a", "a"),
            finding("one/../x/a", "a"),
            finding("./root", "a"),
        ],
        false,
        1,
        None,
    );
    let names: std::collections::BTreeSet<_> = groups.iter().map(|g| &g.name).collect();
    assert_eq!(names.len(), groups.len());
    assert!(names.iter().all(|name| !name.contains('/')));
    assert!(groups.iter().any(|g| g.keys == vec!["x"]));
    assert!(groups.iter().any(|g| g.name == "root"));
}

#[test]
fn displayed_verifier_names_are_short_and_have_their_own_collision_space() {
    let groups = group(
        &[
            finding("a/pkg/one", "a"),
            finding("b/pkg/one", "b"),
            finding("c/pkg-2/one", "c"),
            finding("root", "d"),
        ],
        false,
        1,
        None,
    );
    assert_eq!(
        groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        ["root", "a-pkg", "b-pkg", "c-pkg-2"]
    );
    assert_eq!(
        groups.iter().map(|g| g.shown.as_str()).collect::<Vec<_>>(),
        ["root", "pkg", "pkg-2", "pkg-2-2"]
    );
}
