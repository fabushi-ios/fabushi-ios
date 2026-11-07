use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_capability::{
    CsnapsCapability, CsnapsUnavailableReason, resolve_csnaps_bin_path_in,
    resolve_csnaps_capability_at,
};
use uuid::Uuid;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "fabushi-csnaps-{name}-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).expect("create scratch dir");
    root
}

#[test]
fn override_and_default_path_match_frozen_contract() {
    let root = scratch("path");
    let mut env = BTreeMap::new();

    assert_eq!(
        resolve_csnaps_bin_path_in(&env, &root),
        root.join("extensions").join("codebase-telemetry").join("csnaps")
    );

    env.insert("SAND_CSNAPS_BIN".into(), "  /tmp/custom-csnaps  ".into());
    assert_eq!(
        resolve_csnaps_bin_path_in(&env, &root),
        PathBuf::from("/tmp/custom-csnaps")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_and_non_file_paths_preserve_frozen_reasons() {
    let root = scratch("missing");
    let missing = root.join("missing");
    assert_eq!(
        resolve_csnaps_capability_at(missing.clone()),
        CsnapsCapability::Unavailable {
            executable_path: missing,
            reason: CsnapsUnavailableReason::Missing,
        }
    );

    let directory = root.join("directory");
    fs::create_dir_all(&directory).expect("create directory");
    assert_eq!(
        resolve_csnaps_capability_at(directory.clone()),
        CsnapsCapability::Unavailable {
            executable_path: directory,
            reason: CsnapsUnavailableReason::NotFile,
        }
    );

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn executable_bit_controls_availability_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("mode");
    let binary = root.join("csnaps");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("write binary");

    let mut permissions = fs::metadata(&binary).expect("stat binary").permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&binary, permissions).expect("clear executable bit");
    assert_eq!(
        resolve_csnaps_capability_at(binary.clone()),
        CsnapsCapability::Unavailable {
            executable_path: binary.clone(),
            reason: CsnapsUnavailableReason::NotExecutable,
        }
    );

    let mut permissions = fs::metadata(&binary).expect("stat binary").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).expect("set executable bit");
    assert_eq!(
        resolve_csnaps_capability_at(binary.clone()),
        CsnapsCapability::Available {
            executable_path: binary,
        }
    );

    let _ = fs::remove_dir_all(root);
}
