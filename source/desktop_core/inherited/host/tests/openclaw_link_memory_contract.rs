use std::path::Path;
use mahayana_host_runtime::extensions::attachments::link_understanding_service::*;
use mahayana_host_runtime::extensions::memory::root_memory_provenance::*;

#[test]
fn test_ocrm_link_und_o01_markdown_span_exclusion_and_bare_link_extraction() {
    let msg = "Check out [Our Docs](https://docs.example.com/guide) and then visit https://example.com/welcome and https://example.com/welcome again.";
    
    // Markdown link spans should capture the link
    let spans = find_markdown_link_spans(msg);
    assert_eq!(spans.len(), 1);

    // Bare link extraction should exclude markdown links and deduplicate
    let links = extract_bare_links_from_message(msg, 3);
    assert_eq!(links, vec!["https://example.com/welcome"]);

    // Exclude blocked hosts
    let blocked_msg = "Links: https://localhost/api http://127.0.0.1:8080/test https://valid.example.com/ok";
    let extracted = extract_bare_links_from_message(blocked_msg, 5);
    assert_eq!(extracted, vec!["https://valid.example.com/ok"]);

    // Max links clamping
    let multi = "https://one.example.com https://two.example.com https://three.example.com https://four.example.com";
    let capped = extract_bare_links_from_message(multi, 2);
    assert_eq!(capped.len(), 2);
    assert_eq!(capped, vec!["https://one.example.com/", "https://two.example.com/"]);
}

#[test]
fn test_ocrm_link_und_o02_template_detection_and_body_enrichment() {
    assert!(is_link_url_template("curl -s {{LinkUrl}}"));
    assert!(is_link_url_template("wget {{LinkFinalUrl}}"));
    assert!(!is_link_url_template("python process.py --stdin"));

    let body = "User prompt text.";
    let outputs = vec!["Link 1 content".to_string(), "Link 2 content".to_string()];
    let enriched = enrich_message_body_with_links(Some(body), &outputs);
    assert_eq!(
        enriched,
        "User prompt text.\n\nLink 1 content\nLink 2 content"
    );

    assert_eq!(
        enrich_message_body_with_links(None, &outputs),
        "Link 1 content\nLink 2 content"
    );
    assert_eq!(
        enrich_message_body_with_links(Some(body), &[]),
        "User prompt text."
    );
}

#[test]
fn test_ocrm_memory_root_o01_paths_and_auxiliary_filtering() {
    let ws = Path::new("/workspace");
    assert_eq!(
        resolve_canonical_root_memory_path(ws),
        ws.join("MEMORY.md")
    );
    assert_eq!(
        resolve_legacy_root_memory_path(ws),
        ws.join("memory.md")
    );
    assert_eq!(
        resolve_root_memory_repair_dir(ws),
        ws.join(".openclaw-repair/root-memory")
    );

    // Skipping auxiliary paths
    assert!(should_skip_root_memory_auxiliary_path(ws, &ws.join("memory.md")));
    assert!(should_skip_root_memory_auxiliary_path(ws, &ws.join(".openclaw-repair/root-memory")));
    assert!(should_skip_root_memory_auxiliary_path(ws, &ws.join(".openclaw-repair/root-memory/backup.md")));

    // Non-auxiliary paths must NOT be skipped
    assert!(!should_skip_root_memory_auxiliary_path(ws, &ws.join("MEMORY.md")));
    assert!(!should_skip_root_memory_auxiliary_path(ws, &ws.join("memory/notes.md")));
    assert!(!should_skip_root_memory_auxiliary_path(ws, Path::new("/outside/workspace/memory.md")));
}

#[test]
fn test_ocrm_memory_provenance_o02_relative_paths_and_origin_resolution() {
    // 1. Path normalization
    assert_eq!(
        normalize_memory_artifact_relative_path("MEMORY.md"),
        Some("MEMORY.md".to_string())
    );
    assert_eq!(
        normalize_memory_artifact_relative_path("memory.md"),
        Some("memory.md".to_string())
    );
    assert_eq!(
        normalize_memory_artifact_relative_path("USER.md"),
        Some("USER.md".to_string())
    );
    assert_eq!(
        normalize_memory_artifact_relative_path("users/bob-123/USER.md"),
        Some("users/bob-123/USER.md".to_string())
    );
    assert_eq!(
        normalize_memory_artifact_relative_path(r"memory\sub\topic.md"),
        Some("memory/sub/topic.md".to_string())
    );

    // Rejections
    assert_eq!(normalize_memory_artifact_relative_path("memory/dreaming/d1.md"), None);
    assert_eq!(normalize_memory_artifact_relative_path("memory/.dreams/d1.md"), None);
    assert_eq!(normalize_memory_artifact_relative_path("memory/../outside.md"), None);
    assert_eq!(normalize_memory_artifact_relative_path("/absolute/MEMORY.md"), None);
    assert_eq!(normalize_memory_artifact_relative_path("other/file.md"), None);

    // 2. Hash calculation
    let hash1 = compute_file_content_hash("hello world");
    let hash2 = compute_file_content_hash("hello world");
    let hash3 = compute_file_content_hash("hello world!");
    assert_eq!(hash1, hash2);
    assert_ne!(hash1, hash3);
    assert_eq!(hash1.len(), 64);

    // 3. Write origin resolution
    let initial_origin = resolve_memory_artifact_write_origin(
        None,
        MemoryArtifactOriginClass::Agent,
        &hash1,
    );
    assert_eq!(initial_origin, MemoryArtifactOriginClass::Agent);

    let prev = MemoryArtifactProvenance {
        file_hash: hash1.clone(),
        origin_class: MemoryArtifactOriginClass::Agent,
        observed_at: 1000,
        session_id: None,
        session_key: None,
    };

    // Agent writes matching before hash preserves Agent origin
    let matching_origin = resolve_memory_artifact_write_origin(
        Some(&prev),
        MemoryArtifactOriginClass::Agent,
        &hash1,
    );
    assert_eq!(matching_origin, MemoryArtifactOriginClass::Agent);

    // Agent writes mismatching before hash degrades to Untrusted
    let tampered_origin = resolve_memory_artifact_write_origin(
        Some(&prev),
        MemoryArtifactOriginClass::Agent,
        &hash3,
    );
    assert_eq!(tampered_origin, MemoryArtifactOriginClass::Untrusted);

    // Untrusted requested origin always yields Untrusted
    let explicit_untrusted = resolve_memory_artifact_write_origin(
        Some(&prev),
        MemoryArtifactOriginClass::Untrusted,
        &hash1,
    );
    assert_eq!(explicit_untrusted, MemoryArtifactOriginClass::Untrusted);
}
