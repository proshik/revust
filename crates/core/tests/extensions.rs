use revust_core::config::extensions::{Document, merge};
use serde_json::json;

#[test]
fn profile_policy_preserves_false_zero_null_and_replaces_arrays() {
    let user = Document::parse(
        r#"{
      "version":1,
      "policy":{"budget":{"max_calls":12,"elapsed":"5m"},
        "completeness":{"required_reviewers":["first","second"]},
        "stages":{"verify":{"enabled":true}},"retries":{"find":{"count":1}}},
      "profiles":{"focused":{"policy":{"budget":{"max_calls":null},
        "completeness":{"required_reviewers":[]},"stages":{"verify":{"enabled":false}},
        "retries":{"find":{"count":0,"conditions":[]}}}}}
    }"#,
    )
    .unwrap();
    let selected = user.policy(Some("focused"));
    assert_eq!(selected["budget"], json!({"max_calls":null,"elapsed":"5m"}));
    assert_eq!(selected["stages"]["verify"]["enabled"], false);
    assert_eq!(
        selected["retries"]["find"],
        json!({"count":0,"conditions":[]})
    );
    assert_eq!(selected["completeness"]["required_reviewers"], json!([]));
    assert_eq!(user.policy(Some("another"))["budget"]["max_calls"], 12);
    let mut inherited = selected;
    merge(
        &mut inherited,
        &json!({"budget":{"elapsed":null},"retries":{"verify":{"count":2}}}),
    );
    assert_eq!(
        inherited["budget"],
        json!({"max_calls":null,"elapsed":null})
    );
    assert_eq!(inherited["retries"]["find"]["count"], 0);
    assert_eq!(inherited["retries"]["verify"]["count"], 2);
}

#[test]
fn invalid_extension_values_fail_without_echoing_secret_values() {
    for document in [
        json!({}),
        json!({"version":2}),
        json!({"version":1,"typo":true}),
        json!({"version":1,"policy":{"budget":{"max_calls":0}}}),
        json!({"version":1,"policy":{"budget":{"elapsed":"0s"}}}),
        json!({"version":1,"policy":{"runtime":{"max_parallel":0}}}),
        json!({"version":1,"policy":{"stages":{"find":{"enabled":false}}}}),
        json!({"version":1,"policy":{"stages":{"verify":{"enabled":null}}}}),
        json!({"version":1,"policy":{"stages":{"verify":{"enabled":"false"}}}}),
        json!({"version":1,"policy":{"stages":{"verify":{"idle_timeout":"-1s"}}}}),
        json!({"version":1,"policy":{"retries":{"verify":{"count":-1}}}}),
        json!({"version":1,"policy":{"retries":{"verify":{"conditions":["typo"]}}}}),
        json!({"version":1,"policy":{"retries":{"verify":{"delay":{"factor":0.5}}}}}),
        json!({"version":1,"policy":{"verification":{"min_confidence":101}}}),
        json!({"version":1,"policy":{"executors":{"opencode":{"options":{"session_mode":"attach"}}}}}),
        json!({"version":1,"policy":{"executors":{"codex":{"env":{"TOKEN":{"secret":"do-not-echo"}}}}}}),
    ] {
        let error = Document::parse(&document.to_string())
            .err()
            .expect("must reject");
        assert!(!error.contains("do-not-echo"));
    }
}

#[test]
fn explicit_empty_child_environment_is_valid_and_new_executor_is_isolated() {
    let document = Document::parse(
        r#"{"version":1,"policy":{"executors":{"opencode":{
        "executable":"./bin/opencode","env":{"OPTIONAL_SETTING":""},
        "options":{"agent":"review","session_mode":"isolated"}}}}}"#,
    )
    .unwrap();
    assert_eq!(
        document.policy(None)["executors"]["opencode"]["env"]["OPTIONAL_SETTING"],
        ""
    );
}
