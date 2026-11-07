use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::box_store_sync::box_store_pack_pipeline::should_restore_from_pack;
use mahayana_host_runtime::extensions::box_store_sync::box_store_pack::{
    PACK_INDEX_VERSION, PACK_MEMBER_MAX_BYTES, PackEntry, PackExtractionSink, PackIndex,
    PackMember, PackSource, build_pack_file, extract_pack_members, is_box_store_pack_build_enabled,
    pack_member_clen_bound, parse_pack_index, parse_pack_retired, plan_pack_maintenance,
    serialize_pack_index, serialize_pack_retired, sha256_hex,
};

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("fabushi-pack-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn schema_parser_enforces_version_ranges_and_gzip_expansion_bounds() {
    let valid = PackIndex {
        version: PACK_INDEX_VERSION,
        max_vmtime: 7,
        packs: vec![PackEntry {
            id: "p1".into(),
            bytes: 20,
            members: vec![PackMember {
                sha: "abc".into(),
                size: 3,
                offset: 0,
                clen: 20,
                vmtime: 7,
            }],
        }],
    };
    let raw = serialize_pack_index(&valid).unwrap();
    assert!(raw.contains("\"maxVmtime\":7"));
    assert!(!raw.contains("max_vmtime"));
    assert_eq!(parse_pack_index(&raw), Some(valid.clone()));

    let mut invalid = valid;
    invalid.packs[0].members[0].clen =
        pack_member_clen_bound(invalid.packs[0].members[0].size) + 1;
    assert!(parse_pack_index(&serialize_pack_index(&invalid).unwrap()).is_none());

    assert!(parse_pack_index(r#"{"version":2,"maxVmtime":0,"packs":[]}"#).is_none());
    assert!(pack_member_clen_bound(PACK_MEMBER_MAX_BYTES) > PACK_MEMBER_MAX_BYTES);
}

#[test]
fn retired_round_trip_and_pack_feature_flag_match_frozen_contract() {
    let retired = vec!["a".to_string(), "b".to_string()];
    assert_eq!(
        parse_pack_retired(&serialize_pack_retired(&retired).unwrap()),
        Some(retired)
    );
    assert!(!is_box_store_pack_build_enabled(Some("0")));
    assert!(!is_box_store_pack_build_enabled(Some(" FALSE ")));
    assert!(!is_box_store_pack_build_enabled(Some("no")));
    assert!(is_box_store_pack_build_enabled(None));
    assert!(is_box_store_pack_build_enabled(Some("1")));
}

struct Sink {
    wanted: String,
    blobs: Mutex<Vec<(String, Vec<u8>)>>,
}

impl PackExtractionSink for Sink {
    fn wants(&self, member: &PackMember) -> bool {
        member.sha == self.wanted
    }

    fn on_blob(&self, member: &PackMember, bytes: Vec<u8>) -> Result<(), String> {
        self.blobs.lock().unwrap().push((member.sha.clone(), bytes));
        Ok(())
    }
}

#[test]
fn build_skips_changed_sources_and_extract_validates_size_and_sha() {
    let root = temp_root();
    let a = root.join("a");
    let b = root.join("b");
    fs::write(&a, b"alpha").unwrap();
    fs::write(&b, b"bravo").unwrap();
    let pack = root.join("pack.gz");

    let a_sha = sha256_hex(b"alpha");
    let sources = vec![
        PackSource {
            abs_path: a.clone(),
            sha: a_sha.clone(),
            size: 5,
            vmtime: 1,
        },
        PackSource {
            abs_path: b,
            sha: "wrong".into(),
            size: 5,
            vmtime: 2,
        },
    ];
    let built = build_pack_file(&pack, &sources, || false)
        .unwrap()
        .unwrap();
    assert_eq!(built.members.len(), 1);
    assert_eq!(built.skipped, 1);
    assert_eq!(built.members[0].sha, a_sha);

    let sink = Arc::new(Sink {
        wanted: built.members[0].sha.clone(),
        blobs: Mutex::new(Vec::new()),
    });
    let result = extract_pack_members(&pack, &built.members, 4, sink.clone()).unwrap();
    assert_eq!(result.extracted, 1);
    assert_eq!(result.mismatched, 0);
    assert_eq!(sink.blobs.lock().unwrap()[0].1, b"alpha");

    let mut corrupt = built.members.clone();
    corrupt[0].sha = sha256_hex(b"different");
    let corrupt_sink = Arc::new(Sink {
        wanted: corrupt[0].sha.clone(),
        blobs: Mutex::new(Vec::new()),
    });
    let result = extract_pack_members(&pack, &corrupt, 2, corrupt_sink.clone()).unwrap();
    assert_eq!(result.extracted, 0);
    assert_eq!(result.mismatched, 1);
    assert!(corrupt_sink.blobs.lock().unwrap().is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn aborted_build_removes_partial_pack() {
    let root = temp_root();
    let src = root.join("a");
    let dest = root.join("pack");
    fs::write(&src, b"alpha").unwrap();
    let source = PackSource {
        abs_path: src,
        sha: sha256_hex(b"alpha"),
        size: 5,
        vmtime: 1,
    };
    assert!(build_pack_file(&dest, &[source], || true).unwrap().is_none());
    assert!(!dest.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn maintenance_keeps_live_packs_retires_partial_packs_and_defers_small_tail() {
    let index = PackIndex {
        version: PACK_INDEX_VERSION,
        max_vmtime: 10,
        packs: vec![
            PackEntry {
                id: "keep".into(),
                bytes: 20,
                members: vec![PackMember {
                    sha: "a".into(),
                    size: 5,
                    offset: 0,
                    clen: 10,
                    vmtime: 8,
                }],
            },
            PackEntry {
                id: "retire".into(),
                bytes: 20,
                members: vec![
                    PackMember {
                        sha: "b".into(),
                        size: 5,
                        offset: 0,
                        clen: 10,
                        vmtime: 9,
                    },
                    PackMember {
                        sha: "gone".into(),
                        size: 5,
                        offset: 10,
                        clen: 10,
                        vmtime: 10,
                    },
                ],
            },
        ],
    };
    let live = HashMap::from([
        ("a".to_string(), 5),
        ("b".to_string(), 5),
        ("c".to_string(), 5),
    ]);
    let eligible = HashMap::from([("c".to_string(), 5)]);
    let plan = plan_pack_maintenance(Some(&index), &live, &eligible, 20, 2, 8);

    assert_eq!(plan.kept_packs.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["keep"]);
    assert_eq!(plan.retired_pack_ids, ["retire"]);
    assert_eq!(plan.new_packs.len(), 1);
    assert_eq!(plan.new_packs[0].iter().map(|m| m.sha.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    assert_eq!(plan.new_packs[0][0].vmtime, 8);
    assert_eq!(plan.new_packs[0][1].vmtime, 11);
    assert_eq!(plan.new_max_vmtime, 11);

    let small = plan_pack_maintenance(
        None,
        &HashMap::new(),
        &HashMap::from([("z".to_string(), 1)]),
        20,
        2,
        8,
    );
    assert!(small.new_packs.is_empty());
    assert_eq!(small.deferred_members, 1);
}


#[test]
fn pack_restore_heuristic_matches_frozen_bulk_small_contract() {
    assert!(!should_restore_from_pack(15, 10, 1024, 80));
    assert!(should_restore_from_pack(16, 10, 1024, 80));
    assert!(should_restore_from_pack(1, 10, 4 * 1024 * 1024, 80));
    assert!(!should_restore_from_pack(16, 9, 1024, 80));
}
