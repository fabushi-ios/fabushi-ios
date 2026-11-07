use std::fs;

use mahayana_host_runtime::extensions::box_store_sync::workspace_ignore::{
    SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS, SAND_WORKSPACE_IGNORE_FILE_NAME,
    compile_workspace_ignore, load_workspace_ignore, parse_ignore_patterns,
};
use uuid::Uuid;

#[test]
fn parses_ignore_file_lines_like_the_frozen_host() {
    assert_eq!(
        parse_ignore_patterns("node_modules/  \n# comment\n\n!node_modules/keep/\r\n"),
        vec!["node_modules/", "!node_modules/keep/"]
    );
}

#[test]
fn default_workspace_patterns_ignore_nested_build_artifacts() {
    let ignore = compile_workspace_ignore(SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS);
    assert!(ignore.ignores("project/node_modules/pkg/index.js"));
    assert!(ignore.ignores("nested/app/target/debug/app"));
    assert!(ignore.ignores("src/cache.pyc"));
    assert!(ignore.ignores("core.123"));
    assert!(!ignore.ignores("src/main.rs"));
}

#[test]
fn negation_unignores_target_and_prevents_unsafe_directory_pruning() {
    let ignore = compile_workspace_ignore([
        "dist/",
        "!dist/keep.txt",
        "build/",
    ]);
    assert!(ignore.ignores("dist/drop.txt"));
    assert!(!ignore.ignores("dist/keep.txt"));
    assert!(!ignore.can_prune_dir("dist"));
    assert!(ignore.can_prune_dir("build"));

    let everywhere = compile_workspace_ignore(["target/", "!keep.txt"]);
    assert!(!everywhere.can_prune_dir("target"));
}

#[test]
fn globstar_question_and_character_classes_match_frozen_rules() {
    let ignore = compile_workspace_ignore([
        "**/cache/*.tmp",
        "logs/file?.txt",
        "core.[0-9]*",
    ]);
    assert!(ignore.ignores("cache/a.tmp"));
    assert!(ignore.ignores("a/b/cache/c.tmp"));
    assert!(ignore.ignores("logs/file1.txt"));
    assert!(!ignore.ignores("logs/file12.txt"));
    assert!(ignore.ignores("core.7dump"));
    assert!(!ignore.ignores("core.xdump"));
}

#[test]
fn malformed_negated_regex_class_fails_open_to_ignore_nothing() {
    let ignore = compile_workspace_ignore(["dist/", "![z-a]"]);
    assert!(ignore.is_ignore_nothing());
    assert!(!ignore.ignores("dist/file"));
    assert!(!ignore.can_prune_dir("dist"));
}

#[test]
fn load_workspace_ignore_merges_defaults_and_sandignore() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-workspace-ignore-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).expect("create workspace");
    fs::write(
        root.join(SAND_WORKSPACE_IGNORE_FILE_NAME),
        "generated/\n!generated/keep.txt\n",
    )
    .expect("write ignore file");

    let ignore = load_workspace_ignore(&root, &["node_modules/"]);
    assert!(ignore.ignores("node_modules/a.js"));
    assert!(ignore.ignores("generated/drop.txt"));
    assert!(!ignore.ignores("generated/keep.txt"));
    assert!(!ignore.can_prune_dir("generated"));

    let _ = fs::remove_dir_all(root);
}
