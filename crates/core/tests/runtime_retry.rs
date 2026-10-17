use revust_core::{
    config::{
        extensions::EffectivePolicy,
        runtime::{Invocation, RuntimeConfig},
    },
    runtime::{
        Stage,
        retry::{Failure, RetryPolicy},
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
fn policy(patch: Value) -> EffectivePolicy {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "revust-retry-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let runtime = RuntimeConfig::resolve(
        &root,
        None,
        &Invocation {
            config_dir: Some(root.join("user")),
            ..Invocation::default()
        },
    )
    .unwrap();
    let policy = EffectivePolicy::resolve(&runtime, &[], "comprehensive", &patch).unwrap();
    fs::remove_dir_all(root).unwrap();
    policy
}

#[test]
fn default_retry_policy_bounds_eligible_attempts_by_stage() {
    let policy = policy(json!({}));
    for (stage, failure, allowed) in [
        (Stage::Find, Some(Failure::NonzeroExit), true),
        (Stage::Find, Some(Failure::InvalidOutput), false),
        (Stage::Find, None, false),
        (Stage::Synthesis, Some(Failure::MissingOutput), true),
        (Stage::Synthesis, Some(Failure::InvalidOutput), false),
        (Stage::Verify, Some(Failure::NonzeroExit), false),
    ] {
        let retry = RetryPolicy::from_policy(&policy, stage).unwrap();
        let delay = failure.and_then(|f| retry.delay(1, f, Duration::ZERO).unwrap());
        assert_eq!(delay.is_some(), allowed);
        if let Some(failure) = failure {
            assert_eq!(retry.delay(2, failure, Duration::ZERO).unwrap(), None);
        }
    }
    let find = RetryPolicy::from_policy(&policy, Stage::Find).unwrap();
    assert_eq!(
        find.delay(1, Failure::IdleTimeout, Duration::from_secs(4))
            .unwrap(),
        Some(Duration::from_secs(9))
    );
    assert!(
        find.delay(1, Failure::IdleTimeout, Duration::from_secs(5))
            .is_err()
    );
}

#[test]
fn explicit_conditions_growth_cap_and_zero_keep_their_meaning() {
    let policy = policy(
        json!({"retries":{"verify":{"count":3,"conditions":["invalid_output"],
        "delay":{"base":"2s","factor":3.0,"max":"7s","jitter":"1s"}},
        "find":{"count":0},"synthesis":{"conditions":[]}}}),
    );
    let verify = RetryPolicy::from_policy(&policy, Stage::Verify).unwrap();
    for (number, seconds) in [(1, 2.5), (2, 6.5), (3, 7.0)] {
        assert_eq!(
            verify
                .delay(number, Failure::InvalidOutput, Duration::from_millis(500))
                .unwrap(),
            Some(Duration::from_secs_f64(seconds))
        );
    }
    assert_eq!(
        verify
            .delay(4, Failure::InvalidOutput, Duration::ZERO)
            .unwrap(),
        None
    );
    assert_eq!(
        verify
            .delay(1, Failure::NonzeroExit, Duration::ZERO)
            .unwrap(),
        None
    );
    assert_eq!(
        RetryPolicy::from_policy(&policy, Stage::Find)
            .unwrap()
            .delay(1, Failure::IdleTimeout, Duration::ZERO)
            .unwrap(),
        None
    );
    assert_eq!(
        RetryPolicy::from_policy(&policy, Stage::Synthesis)
            .unwrap()
            .delay(1, Failure::MissingOutput, Duration::ZERO)
            .unwrap(),
        None
    );
}

#[test]
fn unchanged_delay_preserves_nanoseconds_and_growth_saturates_under_the_cap() {
    let configured = policy(json!({"retries":{"find":{"count":3,"delay":{
        "base":"9223372036854775807ns","factor":1.0,"jitter":"0s"}},
        "verify":{"count":3,"delay":{"base":"2s","factor":1e308,"max":"7s"}}}}));
    let find = RetryPolicy::from_policy(&configured, Stage::Find).unwrap();
    for number in [1, 3] {
        assert_eq!(
            find.delay(number, Failure::MissingOutput, Duration::ZERO)
                .unwrap(),
            Some(Duration::from_nanos(i64::MAX as u64))
        );
    }
    assert_eq!(
        RetryPolicy::from_policy(&configured, Stage::Verify)
            .unwrap()
            .delay(3, Failure::MissingOutput, Duration::ZERO)
            .unwrap(),
        Some(Duration::from_secs(7))
    );
}
