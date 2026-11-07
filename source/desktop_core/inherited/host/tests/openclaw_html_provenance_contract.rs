use std::fs;
use mahayana_host_runtime::extensions::attachments::host_read_media_policy::{
    filter_stale_provenance_rows, is_allowed_host_read_html, validate_provenance_row,
    GeneratedHtmlProvenanceRow, HostReadHtmlTrust,
};
use sha2::{Digest, Sha256};

#[test]
fn test_html_provenance_marker_validation() {
    let valid_row = GeneratedHtmlProvenanceRow {
        realpath: "/path/to/report.html".to_string(),
        kind: "trusted-generated-html".to_string(),
        version: 1,
        sha256: "abc123def456".to_string(),
        size_bytes: 4096,
        created_at_ms: 1700000000000,
    };
    let marker = validate_provenance_row(&valid_row);
    assert!(marker.is_some());
    let marker = marker.unwrap();
    assert_eq!(marker.sha256, "abc123def456");
    assert_eq!(marker.size, 4096);

    let wrong_version = GeneratedHtmlProvenanceRow {
        version: 2,
        ..valid_row.clone()
    };
    assert!(validate_provenance_row(&wrong_version).is_none());

    let wrong_kind = GeneratedHtmlProvenanceRow {
        kind: "untrusted-html".to_string(),
        ..valid_row
    };
    assert!(validate_provenance_row(&wrong_kind).is_none());
}

#[test]
fn test_html_provenance_filter_stale_rows() {
    let temp_dir = std::env::temp_dir().join(format!("ocrm-html-provenance-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).expect("create_dir_all");
    let regular_file = temp_dir.join("regular.html");
    fs::write(&regular_file, b"<!doctype html><html><body>Report</body></html>").unwrap();

    let missing_file = temp_dir.join("missing.html");

    #[cfg(unix)]
    let symlink_file = temp_dir.join("symlink.html");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&regular_file, &symlink_file).unwrap();

    let rows = vec![
        GeneratedHtmlProvenanceRow {
            realpath: regular_file.to_string_lossy().to_string(),
            kind: "trusted-generated-html".to_string(),
            version: 1,
            sha256: "dummy".to_string(),
            size_bytes: 40,
            created_at_ms: 1000,
        },
        GeneratedHtmlProvenanceRow {
            realpath: missing_file.to_string_lossy().to_string(),
            kind: "trusted-generated-html".to_string(),
            version: 1,
            sha256: "dummy".to_string(),
            size_bytes: 40,
            created_at_ms: 1000,
        },
        #[cfg(unix)]
        GeneratedHtmlProvenanceRow {
            realpath: symlink_file.to_string_lossy().to_string(),
            kind: "trusted-generated-html".to_string(),
            version: 1,
            sha256: "dummy".to_string(),
            size_bytes: 40,
            created_at_ms: 1000,
        },
    ];

    let stale = filter_stale_provenance_rows(&rows);
    #[cfg(unix)]
    {
        assert_eq!(stale.len(), 2);
        let stale_paths: Vec<_> = stale.iter().map(|r| r.realpath.as_str()).collect();
        assert!(stale_paths.contains(&missing_file.to_string_lossy().as_ref()));
        assert!(stale_paths.contains(&symlink_file.to_string_lossy().as_ref()));
    }
    #[cfg(not(unix))]
    {
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].realpath, missing_file.to_string_lossy().as_ref());
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_html_provenance_host_read_admission() {
    let payload = b"<!doctype html><html><head><title>Test</title></head><body>Valid</body></html>";
    let sha256 = format!("{:x}", Sha256::digest(payload));
    let size = payload.len();

    let trust = HostReadHtmlTrust::Outbound {
        expected_sha256: sha256.clone(),
        expected_size: size,
    };

    // Valid HTML with matching outbound provenance trust
    assert!(is_allowed_host_read_html(
        Some("text/html"),
        Some(payload),
        Some(&trust),
        false
    ));

    // Mismatched size rejected
    let bad_size_trust = HostReadHtmlTrust::Outbound {
        expected_sha256: sha256.clone(),
        expected_size: size + 1,
    };
    assert!(!is_allowed_host_read_html(
        Some("text/html"),
        Some(payload),
        Some(&bad_size_trust),
        false
    ));

    // Mismatched SHA256 rejected
    let bad_sha_trust = HostReadHtmlTrust::Outbound {
        expected_sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        expected_size: size,
    };
    assert!(!is_allowed_host_read_html(
        Some("text/html"),
        Some(payload),
        Some(&bad_sha_trust),
        false
    ));

    // No provenance trust rejected unless explicit publication
    assert!(!is_allowed_host_read_html(
        Some("text/html"),
        Some(payload),
        None,
        false
    ));
    assert!(is_allowed_host_read_html(
        Some("text/html"),
        Some(payload),
        None,
        true
    ));
}
