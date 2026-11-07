use std::collections::BTreeMap;

use mahayana_host_runtime::extensions::cross_user_sharing::xuser_sharing_environment::{
    SAND_DEV_XUSER_SHARING_ENV, SAND_XUSER_SHARING_ALLOW_PROD_ENV,
    is_production_backend_url, resolve_xuser_sharing_environment,
};

fn env(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn packaged_host_allows_cross_user_sharing_without_dev_overrides() {
    let resolved = resolve_xuser_sharing_environment(
        "https://api.ombhrum.com",
        &env(&[("SAND_PACKAGED", "1")]),
    );
    assert!(resolved.is_allowed);
    assert!(resolved.reason.is_none());
}

#[test]
fn dev_host_requires_explicit_xuser_opt_in() {
    let resolved = resolve_xuser_sharing_environment("https://fabushi-ci-isolated-auth.bhrumom.workers.dev", &env(&[]));
    assert!(!resolved.is_allowed);
    assert!(resolved
        .reason
        .as_deref()
        .is_some_and(|value| value.contains(SAND_DEV_XUSER_SHARING_ENV)));
}

#[test]
fn opted_in_dev_host_allows_non_production_backend() {
    let resolved = resolve_xuser_sharing_environment(
        "https://fabushi-ci-isolated-auth.bhrumom.workers.dev",
        &env(&[(SAND_DEV_XUSER_SHARING_ENV, "1")]),
    );
    assert!(resolved.is_allowed);
}

#[test]
fn opted_in_dev_host_still_requires_explicit_production_backend_opt_in() {
    let resolved = resolve_xuser_sharing_environment(
        "https://api.ombhrum.com",
        &env(&[(SAND_DEV_XUSER_SHARING_ENV, "1")]),
    );
    assert!(!resolved.is_allowed);
    assert!(resolved
        .reason
        .as_deref()
        .is_some_and(|value| value.contains(SAND_XUSER_SHARING_ALLOW_PROD_ENV)));

    let allowed = resolve_xuser_sharing_environment(
        "https://api.ombhrum.com",
        &env(&[
            (SAND_DEV_XUSER_SHARING_ENV, "1"),
            (SAND_XUSER_SHARING_ALLOW_PROD_ENV, "1"),
        ]),
    );
    assert!(allowed.is_allowed);
}

#[test]
fn production_origin_comparison_is_fail_closed() {
    assert!(is_production_backend_url("https://api.ombhrum.com/foo"));
    assert!(is_production_backend_url("https://api.ombhrum.com:443"));
    assert!(is_production_backend_url("not a url"));
    assert!(!is_production_backend_url("https://api.ombhrum.com.evil.example"));
    assert!(!is_production_backend_url("https://api.ombhrum.com:444"));
}

#[test]
fn foreign_or_malformed_service_is_denied_even_when_packaged_or_deliberately_opted_in() {
    for backend in ["https://api2.cursor.sh", "https://api3.cursor.sh", "https://cursor.com",
        "https://api.ombhrum.com.evil.example", "http://api.ombhrum.com", "https://api.ombhrum.com:444",
        "https://user:password@api.ombhrum.com", "https://api.ombhrum.com/?token=x", "not a url"] {
        for environment in [env(&[("SAND_PACKAGED", "1")]), env(&[(SAND_DEV_XUSER_SHARING_ENV,"1"),(SAND_XUSER_SHARING_ALLOW_PROD_ENV,"1")])] {
            assert!(!resolve_xuser_sharing_environment(backend,&environment).is_allowed, "{backend}");
        }
    }
}

#[test]
fn explicit_loopback_and_isolated_ci_follow_dev_gate_without_production_override() {
    for backend in ["http://127.0.0.1:4200", "http://localhost:4200", "http://[::1]:4200", "https://fabushi-ci-isolated-auth.bhrumom.workers.dev"] {
        assert!(!resolve_xuser_sharing_environment(backend,&env(&[])).is_allowed);
        assert!(resolve_xuser_sharing_environment(backend,&env(&[(SAND_DEV_XUSER_SHARING_ENV,"1")])).is_allowed);
        assert!(resolve_xuser_sharing_environment(backend,&env(&[("SAND_PACKAGED","1")])).is_allowed);
    }
}
