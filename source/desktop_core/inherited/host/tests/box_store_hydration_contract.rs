use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::box_store_sync::box_store_hydration::{
    BOX_STORE_HYDRATION_HANDOFF_MANIFEST_PATH, HydrationEvidence,
    is_box_store_fully_hydrated, is_hydration_handoff_manifest_path,
    remove_hydration_handoff_marker, write_hydration_handoff_marker,
};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("fabushi-hydration-{label}-{}-{n}",std::process::id()))
}

#[test]
fn hydration_evidence_matches_frozen_legacy_and_manifest_rules() {
    assert!(!is_box_store_fully_hydrated(None));
    assert!(!is_box_store_fully_hydrated(Some(&HydrationEvidence::default())));

    let complete=HydrationEvidence{
        failures:Some(vec![]),manifest_entries:Some(2),files:Some(2),verified:Some(2),..Default::default()
    };
    assert!(is_box_store_fully_hydrated(Some(&complete)));

    let partial=HydrationEvidence{verified:Some(1),..complete.clone()};
    assert!(!is_box_store_fully_hydrated(Some(&partial)));

    let legacy=HydrationEvidence{
        failures:Some(vec![]),manifest_entries:Some(9),files:Some(1),verified:Some(0),
        hydrate_source:Some("legacy".into()),authoritative_store_db_entries:Some(3),
        restored_store_db_entries:Some(3),
    };
    assert!(is_box_store_fully_hydrated(Some(&legacy)));
    assert!(!is_box_store_fully_hydrated(Some(&HydrationEvidence{
        restored_store_db_entries:Some(2),..legacy
    })));
}

#[test]
fn handoff_path_matching_and_marker_io_are_durable() {
    assert!(is_hydration_handoff_manifest_path(BOX_STORE_HYDRATION_HANDOFF_MANIFEST_PATH));
    assert!(is_hydration_handoff_manifest_path(&format!(
        "{BOX_STORE_HYDRATION_HANDOFF_MANIFEST_PATH}.123.tmp"
    )));
    assert!(!is_hydration_handoff_manifest_path(&format!(
        "{BOX_STORE_HYDRATION_HANDOFF_MANIFEST_PATH}.tmp.extra"
    )));

    let dir=temp_dir("marker");
    let marker=dir.join("nested/marker");
    write_hydration_handoff_marker(&marker).expect("write marker");
    assert_eq!(fs::read_to_string(&marker).unwrap(),"complete\n");
    let names=fs::read_dir(marker.parent().unwrap()).unwrap()
        .map(|entry|entry.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
    assert_eq!(names,vec!["marker"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&marker).unwrap().permissions().mode() & 0o777,0o600);
    }

    remove_hydration_handoff_marker(&marker).expect("remove marker");
    assert!(!marker.exists());
    assert!(marker.parent().unwrap().exists());
    let _=fs::remove_dir_all(dir);
}
