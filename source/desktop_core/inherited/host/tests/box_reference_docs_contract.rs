use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::runner::box_reference_docs::{
    DEBUGGING_THE_BOX_FILE, LEGACY_SAND_BOX_REFERENCE_DIR, SAND_APP_UI_FILE,
    SAND_APP_UI_REFERENCE_DOC, SAND_BOX_DEBUGGING_REFERENCE_DOC,
    SAND_BOX_REFERENCE_DOCS, provision_sand_box_prompt_artifacts_with,
    write_sand_box_reference_docs,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-box-reference-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn frozen_reference_documents_keep_the_expected_prompt_contract() {
    assert_eq!(SAND_BOX_REFERENCE_DOCS.len(), 2);
    assert!(SAND_BOX_DEBUGGING_REFERENCE_DOC.starts_with("# Debugging the box\n"));
    assert!(SAND_BOX_DEBUGGING_REFERENCE_DOC.contains("box-doctor"));
    assert!(SAND_BOX_DEBUGGING_REFERENCE_DOC.contains("request_box_help"));
    assert!(SAND_APP_UI_REFERENCE_DOC.starts_with("# The Fabushi app UI"));
    assert!(SAND_APP_UI_REFERENCE_DOC.contains("Update Fabushi's Computer"));
    assert!(SAND_APP_UI_REFERENCE_DOC.ends_with('\n'));
}

#[test]
fn reference_docs_are_atomically_replaced_and_returned_in_frozen_order() {
    let root = temp_root("write");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join(DEBUGGING_THE_BOX_FILE), "stale").expect("stale doc");

    let written = write_sand_box_reference_docs(&root).expect("write docs");
    assert_eq!(
        written,
        vec![root.join(DEBUGGING_THE_BOX_FILE), root.join(SAND_APP_UI_FILE)]
    );
    assert_eq!(
        fs::read_to_string(root.join(DEBUGGING_THE_BOX_FILE)).expect("debugging doc"),
        SAND_BOX_DEBUGGING_REFERENCE_DOC
    );
    assert_eq!(
        fs::read_to_string(root.join(SAND_APP_UI_FILE)).expect("ui doc"),
        SAND_APP_UI_REFERENCE_DOC
    );
    assert!(
        fs::read_dir(&root)
            .expect("read root")
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn provision_removes_legacy_reference_directory_without_touching_unrelated_data_root() {
    let root = temp_root("provision");
    let sand_root = root.join("sand-root");
    let reference_dir = root.join("reference");
    let legacy_dir = root.join("legacy-reference");
    let data_root = root.join("different-data-root");
    let model_alias = root.join("model-alias");
    fs::create_dir_all(&legacy_dir).expect("legacy dir");
    fs::write(legacy_dir.join("old.md"), "old").expect("legacy doc");

    let written = provision_sand_box_prompt_artifacts_with(
        &sand_root,
        &reference_dir,
        &legacy_dir,
        &data_root,
        &model_alias,
    )
    .expect("provision");
    assert_eq!(written.len(), 2);
    assert!(!legacy_dir.exists());
    assert!(!model_alias.exists());
    assert_eq!(
        fs::read_to_string(reference_dir.join(DEBUGGING_THE_BOX_FILE))
            .expect("debugging doc"),
        SAND_BOX_DEBUGGING_REFERENCE_DOC
    );

    let _ = fs::remove_dir_all(root);
    let _ = LEGACY_SAND_BOX_REFERENCE_DIR;
}
