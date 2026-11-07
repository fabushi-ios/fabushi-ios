//! OpenClaw Memory Host SDK Contract Test Suite
//!
//! Verifies OCRM-RESP-MEMORY-DREAMING-001 & OCRM-RESP-MEMORY-EVENT-STORE-001:
//! - Memory dreaming promotion calibration, score thresholds & recency decay
//! - Event store normalization, item bound capping (10 items), path truncation (256 bytes)
//! - Retention of exact aggregate result counts under diagnostic truncation
//! - Retired JSONL discovery log path resolution

use mahayana_host_runtime::extensions::memory::dreaming_service::*;
use mahayana_host_runtime::extensions::memory::event_store_service::*;

#[test]
fn test_ocrm_memory_dreaming_promotion_and_decay() {
    let config = MemoryDreamingConfig::default();
    assert_eq!(config.min_score, 0.75);
    assert_eq!(config.min_recall_count, 3);
    assert_eq!(config.min_unique_queries, 3);
    assert_eq!(config.recency_half_life_days, 14.0);

    // Eligible candidate: fresh (1 day old), high score 0.90, 4 recalls, 3 queries
    let eligible = MemoryPromotionCandidate {
        key: "cand-fresh".to_string(),
        path: "memory/2026-10-25.md".to_string(),
        start_line: 10,
        end_line: 25,
        score: 0.90,
        recall_count: 4,
        unique_queries: 3,
        age_days: 1.0,
    };
    assert!(is_candidate_eligible_for_promotion(&eligible, &config));

    // Decayed candidate: initial score 0.85, but 42 days old (3 half-lives -> factor 0.125 -> score ~0.106)
    let decayed = MemoryPromotionCandidate {
        key: "cand-old".to_string(),
        path: "memory/2026-09-10.md".to_string(),
        start_line: 10,
        end_line: 25,
        score: 0.85,
        recall_count: 4,
        unique_queries: 3,
        age_days: 42.0,
    };
    assert!(!is_candidate_eligible_for_promotion(&decayed, &config));

    // Insufficient query diversity (only 1 unique query)
    let low_queries = MemoryPromotionCandidate {
        key: "cand-spam".to_string(),
        path: "memory/2026-10-25.md".to_string(),
        start_line: 1,
        end_line: 5,
        score: 0.90,
        recall_count: 10,
        unique_queries: 1, // < 3 required
        age_days: 1.0,
    };
    assert!(!is_candidate_eligible_for_promotion(&low_queries, &config));
}

#[test]
fn test_ocrm_memory_event_store_normalization_and_truncation() {
    // 15 results (exceeds MAX_MEMORY_HOST_EVENT_ITEMS = 10)
    let results: Vec<MemoryRecallResult> = (0..15)
        .map(|i| MemoryRecallResult {
            path: format!("memory/doc_{i}.md"),
            start_line: 1,
            end_line: 10,
            score: 0.8,
        })
        .collect();

    let raw_event = MemoryHostEventRecord::RecallRecorded {
        timestamp: "2026-10-27T08:00:00Z".to_string(),
        query: "rust native migration".to_string(),
        result_count: 15,
        results,
        storage_truncated: false,
    };

    let normalized = normalize_memory_host_event_for_storage(raw_event);

    match normalized {
        MemoryHostEventRecord::RecallRecorded {
            result_count,
            results,
            storage_truncated,
            ..
        } => {
            assert_eq!(result_count, 15); // Aggregate count preserved!
            assert_eq!(results.len(), 10); // Truncated to 10 items
            assert!(storage_truncated); // Marked truncated
        }
        _ => panic!("Expected RecallRecorded"),
    }
}

#[test]
fn test_ocrm_memory_event_store_path_truncation_and_slice() {
    let long_path = "a/".repeat(200); // 400 chars, exceeds 256 bytes

    let raw_event = MemoryHostEventRecord::RecallRecorded {
        timestamp: "2026-10-27T08:00:00Z".to_string(),
        query: "test".to_string(),
        result_count: 1,
        results: vec![MemoryRecallResult {
            path: long_path,
            start_line: 1,
            end_line: 5,
            score: 0.5,
        }],
        storage_truncated: false,
    };

    let normalized = normalize_memory_host_event_for_storage(raw_event);
    match normalized {
        MemoryHostEventRecord::RecallRecorded {
            results,
            storage_truncated,
            ..
        } => {
            assert!(results[0].path.len() <= 256);
            assert!(storage_truncated);
        }
        _ => panic!("Expected RecallRecorded"),
    }

    // Event limit slicing
    let items = vec![1, 2, 3, 4, 5];
    assert_eq!(apply_memory_host_event_limit(&items, Some(2)), vec![4, 5]);
    assert_eq!(apply_memory_host_event_limit(&items, Some(10)), vec![1, 2, 3, 4, 5]);
    assert_eq!(apply_memory_host_event_limit(&items, Some(0)), Vec::<i32>::new());
    assert_eq!(apply_memory_host_event_limit(&items, None), vec![1, 2, 3, 4, 5]);

    // Retired log path
    let log_path = resolve_memory_host_event_log_path("/test/workspace");
    assert_eq!(
        log_path.to_str().unwrap(),
        "/test/workspace/memory/.dreams/events.jsonl"
    );
}
