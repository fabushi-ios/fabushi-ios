use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::box_store_sync::chrome_session_stage::{
    ChromeStageFailure, ChromeStageReport, ChromeStageRetryPolicy,
    chrome_session_stage_telemetry, stage_box_chrome_session_with,
};
use mahayana_host_runtime::extensions::box_store_sync::sqlite_snapshot::{
    SqliteSnapshotFailure, SqliteSnapshotOperation, SqliteSnapshotPathStage,
};

fn scratch(label: &str) -> std::path::PathBuf {
    let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path=std::env::temp_dir().join(format!("fabushi-chrome-stage-{label}-{}-{n}",std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn failure(cause:&str,error_class:&str)->SqliteSnapshotFailure{
    SqliteSnapshotFailure{
        operation:SqliteSnapshotOperation::VacuumInto,
        path_stage:SqliteSnapshotPathStage::SourceOrStagedMain,
        cause:cause.into(),
        error_class:error_class.into(),
        errno:None,
        sqlite_code:None,
    }
}

#[test]
fn stages_existing_db_with_frozen_relative_path_and_mode_then_cleans_up() {
    let root=scratch("success");
    let source=root.join("profile");
    fs::create_dir_all(&source).unwrap();
    let cookies=source.join("Cookies");
    fs::write(&cookies,b"db").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cookies,fs::Permissions::from_mode(0o640)).unwrap();
    }
    let mut reports=Vec::new();
    let staged=stage_box_chrome_session_with(
        &source,
        "home/box/chrome-profile/Default",
        &["Cookies","Web Data"],
        ChromeStageRetryPolicy{max_attempts:1,delay_ms:0},
        |src,dest|{fs::copy(src,dest).map(|_|()).map_err(|e|failure("error",&e.to_string()))},
        |_src,_dest|panic!("raw copy must not run"),
        |_|{},
        |report|reports.push(report),
    ).unwrap();
    assert_eq!(staged.files.len(),1);
    assert_eq!(staged.skipped,0);
    assert_eq!(staged.files[0].rel_path,"home/box/chrome-profile/Default/Cookies");
    assert_eq!(fs::read(&staged.files[0].abs_path).unwrap(),b"db");
    #[cfg(unix)]
    assert_eq!(staged.files[0].mode,0o640);
    assert_eq!(reports.len(),1);
    assert_eq!(reports[0].staged,1);
    let staged_path=staged.files[0].abs_path.clone();
    staged.cleanup().unwrap();
    assert!(!staged_path.exists());
    let _=fs::remove_dir_all(root);
}

#[test]
fn retry_removes_partial_destination_before_next_vacuum_attempt() {
    let root=scratch("retry");
    let source=root.join("profile");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("Cookies"),b"source").unwrap();
    let mut attempts=0usize;
    let staged=stage_box_chrome_session_with(
        &source,
        "rel",
        &["Cookies"],
        ChromeStageRetryPolicy{max_attempts:2,delay_ms:0},
        |_src,dest|{
            attempts+=1;
            if attempts==1{
                fs::write(dest,b"partial").unwrap();
                Err(failure("busy","database is busy"))
            }else{
                assert!(!dest.exists(),"retry must pre-clean partial destination");
                fs::write(dest,b"clean").unwrap();
                Ok(())
            }
        },
        |_src,_dest|panic!("raw copy not reached after retry success"),
        |_|{},
        |_|{},
    ).unwrap();
    assert_eq!(attempts,2);
    assert_eq!(fs::read(&staged.files[0].abs_path).unwrap(),b"clean");
    staged.cleanup().unwrap();
    let _=fs::remove_dir_all(root);
}

#[test]
fn busy_failure_falls_back_to_locked_copy_but_nonbusy_failure_is_skipped() {
    let root=scratch("fallback");
    let source=root.join("profile");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("Cookies"),b"cookie").unwrap();
    fs::write(source.join("Web Data"),b"web").unwrap();
    let mut reports=Vec::new();
    let staged=stage_box_chrome_session_with(
        &source,
        "rel",
        &["Cookies","Web Data"],
        ChromeStageRetryPolicy{max_attempts:1,delay_ms:0},
        |src,_dest|{
            if src.file_name().unwrap()=="Cookies"{Err(failure("busy","database is locked"))}
            else{Err(failure("io","disk error"))}
        },
        |src,dest|{
            fs::copy(src,dest).map(|_|()).map_err(|e|failure("error",&e.to_string()))
        },
        |_|{},
        |report|reports.push(report),
    ).unwrap();
    assert_eq!(staged.files.len(),1);
    assert_eq!(staged.files[0].rel_path,"rel/Cookies");
    assert_eq!(staged.skipped,1);
    assert_eq!(reports.len(),1);
    assert_eq!(reports[0].staged,1);
    assert_eq!(reports[0].skipped,1);
    assert_eq!(reports[0].skipped_db_names,vec!["Web Data"]);
    assert_eq!(reports[0].error_class.as_deref(),Some("disk error"));
    assert_eq!(reports[0].failure.as_ref().map(|f|f.phase.as_str()),Some("vacuum"));
    staged.cleanup().unwrap();
    let _=fs::remove_dir_all(root);
}

#[test]
fn raw_copy_failure_reports_raw_copy_phase() {
    let root=scratch("raw-fail");
    let source=root.join("profile");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("Cookies"),b"cookie").unwrap();
    let mut reports=Vec::new();
    let staged=stage_box_chrome_session_with(
        &source,
        "rel",
        &["Cookies"],
        ChromeStageRetryPolicy{max_attempts:1,delay_ms:0},
        |_src,_dest|Err(failure("busy","database is busy")),
        |_src,_dest|Err(failure("corrupt","raw copy failed")),
        |_|{},
        |report|reports.push(report),
    ).unwrap();
    assert!(staged.files.is_empty());
    assert_eq!(staged.skipped,1);
    assert_eq!(reports[0].failure.as_ref().map(|f|f.phase.as_str()),Some("raw_copy"));
    assert_eq!(reports[0].error_class.as_deref(),Some("raw copy failed"));
    staged.cleanup().unwrap();
    let _=fs::remove_dir_all(root);
}

#[test]
fn canonical_stage_telemetry_matches_frozen_level_and_metadata() {
    let ok = chrome_session_stage_telemetry(&ChromeStageReport {
        staged: 2,
        skipped: 0,
        skipped_db_names: vec![],
        error_class: None,
        failure: None,
    });
    assert_eq!(ok.level, "info");
    assert_eq!(ok.metadata["staged"], "2");
    assert_eq!(ok.metadata["skipped"], "0");
    assert!(!ok.metadata.contains_key("skipped_dbs"));

    let warned = chrome_session_stage_telemetry(&ChromeStageReport {
        staged: 1,
        skipped: 2,
        skipped_db_names: vec!["Cookies".into(), "Web Data".into()],
        error_class: Some("Error".into()),
        failure: Some(ChromeStageFailure {
            db: "Web Data".into(),
            phase: "raw_copy".into(),
            snapshot: SqliteSnapshotFailure {
                operation: SqliteSnapshotOperation::CopySourceSidecars,
                path_stage: SqliteSnapshotPathStage::SourceAndStagedSidecars,
                cause: "io".into(),
                error_class: "Error".into(),
                errno: Some("ENOSPC".into()),
                sqlite_code: Some(13),
            },
        }),
    });
    assert_eq!(warned.level, "warn");
    assert_eq!(warned.metadata["staged"], "1");
    assert_eq!(warned.metadata["skipped"], "2");
    assert_eq!(warned.metadata["skipped_dbs"], "Cookies,Web Data");
    assert_eq!(warned.metadata["error_class"], "Error");
    assert_eq!(warned.metadata["failure_db"], "Web Data");
    assert_eq!(warned.metadata["failure_phase"], "raw_copy");
    assert_eq!(warned.metadata["failure_operation"], "copy_source_sidecars");
    assert_eq!(warned.metadata["failure_path_stage"], "source_and_staged_sidecars");
    assert_eq!(warned.metadata["failure_cause"], "io");
    assert_eq!(warned.metadata["errno"], "ENOSPC");
    assert_eq!(warned.metadata["sqlite_code"], "13");
}
