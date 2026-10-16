use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::box_store_manifest::BoxStoreManifestEntry;
use crate::box_store_pack::{
    BOX_STORE_PACKS_PREFIX, PACK_INDEX_VERSION, PACK_MAX_MEMBER_SIZE_SUM, PACK_MEMBER_MAX_BYTES,
    PackEntry, PackExtractionSink, PackIndex, PackMember, PackSource, PlannedPackMember,
    build_pack_file, extract_pack_members, pack_member_clen_bound, plan_pack_maintenance,
    serialize_pack_index, sha256_hex,
};
use crate::box_store_pack_pipeline::{
    PACK_BUILD_MIN_BYTES, PACK_BUILD_MIN_MEMBERS, PACK_EXTRACT_CONCURRENCY, PACK_TMP_MAX_AGE_MS,
    should_restore_from_pack, temp_dir,
};

/// The current iOS Agent Store HTTP contract is stricter than Desktop's pack
/// object format: the service accepts at most 8 MiB per object. The pack format
/// still validates Desktop's 320 MiB logical cap, while the shipping iOS
/// transport splits new packs so their worst-case gzip expansion also fits the
/// service limit. Loose blobs remain canonical fallback objects.
pub const IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PreparedPackObject {
    pub entry: PackEntry,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PreparedPackSnapshot {
    pub index: PackIndex,
    pub retired_pack_ids: Vec<String>,
    pub objects: Vec<PreparedPackObject>,
    pub deferred_members: usize,
}

fn cleanup_stale_pack_temp_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age.as_millis() as u64 > PACK_TMP_MAX_AGE_MS)
        {
            if metadata.is_dir() {
                let _ = fs::remove_dir_all(path);
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn transport_chunks(planned: &[PlannedPackMember]) -> (Vec<Vec<PlannedPackMember>>, usize) {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut worst_bytes = 0_u64;
    let mut deferred = 0_usize;

    for member in planned {
        let bound = pack_member_clen_bound(member.size);
        if bound > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES {
            deferred = deferred.saturating_add(1);
            continue;
        }
        if !current.is_empty()
            && worst_bytes.saturating_add(bound) > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES
        {
            chunks.push(std::mem::take(&mut current));
            worst_bytes = 0;
        }
        worst_bytes = worst_bytes.saturating_add(bound);
        current.push(member.clone());
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    (chunks, deferred)
}

pub fn prepare_pack_snapshot(
    temp_base: &Path,
    target_root: &Path,
    entries: &BTreeMap<String, BoxStoreManifestEntry>,
    existing_index: Option<&PackIndex>,
) -> Result<PreparedPackSnapshot, String> {
    let mut live = HashMap::<String, u64>::new();
    let mut eligible = HashMap::<String, u64>::new();
    let mut local_by_sha = HashMap::<String, PathBuf>::new();

    for (rel_path, entry) in entries {
        let (sha, size) = match entry {
            BoxStoreManifestEntry::LegacyFile { sha, size }
            | BoxStoreManifestEntry::File { sha, size, .. } => (sha, *size),
            BoxStoreManifestEntry::Symlink { .. } => continue,
        };
        live.insert(sha.clone(), size);
        if size >= PACK_MEMBER_MAX_BYTES
            || pack_member_clen_bound(size) > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES
            || eligible.contains_key(sha)
        {
            continue;
        }
        let Some(abs_path) = crate::box_store_hydration::resolve_restore_destination(
            target_root,
            rel_path,
        ) else {
            continue;
        };
        let Ok(metadata) = fs::symlink_metadata(&abs_path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != size {
            continue;
        }
        eligible.insert(sha.clone(), size);
        local_by_sha.insert(sha.clone(), abs_path);
    }

    let mut transport_retired = existing_index
        .map(|index| {
            index
                .packs
                .iter()
                .filter(|pack| pack.bytes > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES)
                .map(|pack| pack.id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let transport_index = existing_index.map(|index| PackIndex {
        version: index.version,
        max_vmtime: index.max_vmtime,
        packs: index
            .packs
            .iter()
            .filter(|pack| pack.bytes <= IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES)
            .cloned()
            .collect(),
    });

    let plan = plan_pack_maintenance(
        transport_index.as_ref(),
        &live,
        &eligible,
        PACK_MAX_MEMBER_SIZE_SUM,
        PACK_BUILD_MIN_MEMBERS,
        PACK_BUILD_MIN_BYTES,
    );
    transport_retired.extend(plan.retired_pack_ids.iter().cloned());

    let temp_root = temp_dir(temp_base);
    fs::create_dir_all(&temp_root).map_err(|error| error.to_string())?;
    cleanup_stale_pack_temp_dir(&temp_root);

    let mut objects = Vec::<PreparedPackObject>::new();
    let mut deferred_members = plan.deferred_members;

    for planned in &plan.new_packs {
        let (chunks, transport_deferred) = transport_chunks(planned);
        deferred_members = deferred_members.saturating_add(transport_deferred);
        for chunk in chunks {
            let sources = chunk
                .iter()
                .filter_map(|member| {
                    local_by_sha.get(&member.sha).map(|abs_path| PackSource {
                        abs_path: abs_path.clone(),
                        sha: member.sha.clone(),
                        size: member.size,
                        vmtime: member.vmtime,
                    })
                })
                .collect::<Vec<_>>();
            if sources.is_empty() {
                continue;
            }
            let temp_path = temp_root.join(format!(
                "ios-build-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let built = match build_pack_file(&temp_path, &sources, || false) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    let _ = fs::remove_file(&temp_path);
                    continue;
                }
                Err(error) => {
                    let _ = fs::remove_file(&temp_path);
                    return Err(error);
                }
            };
            deferred_members = deferred_members.saturating_add(built.skipped);
            let built_size_sum = built.members.iter().map(|member| member.size).sum::<u64>();
            if built.members.is_empty()
                || (built.members.len() < PACK_BUILD_MIN_MEMBERS
                    && built_size_sum < PACK_BUILD_MIN_BYTES)
                || built.file_bytes > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES
            {
                deferred_members = deferred_members.saturating_add(built.members.len());
                let _ = fs::remove_file(&temp_path);
                continue;
            }
            let bytes = fs::read(&temp_path).map_err(|error| error.to_string())?;
            let _ = fs::remove_file(&temp_path);
            if bytes.len() as u64 != built.file_bytes {
                return Err("iOS Box Store pack build size changed after flush".into());
            }
            let pack_id = sha256_hex(&bytes);
            objects.push(PreparedPackObject {
                entry: PackEntry {
                    id: pack_id,
                    bytes: built.file_bytes,
                    members: built.members,
                },
                bytes,
            });
        }
    }

    let mut packs = plan.kept_packs.clone();
    packs.extend(objects.iter().map(|object| object.entry.clone()));
    let referenced = packs
        .iter()
        .map(|pack| pack.id.as_str())
        .collect::<HashSet<_>>();
    transport_retired.retain(|id| !referenced.contains(id.as_str()));
    transport_retired.sort();
    transport_retired.dedup();

    let max_vmtime = packs
        .iter()
        .flat_map(|pack| pack.members.iter().map(|member| member.vmtime))
        .max()
        .unwrap_or(0)
        .max(plan.new_max_vmtime)
        .max(existing_index.map(|index| index.max_vmtime).unwrap_or(0));

    Ok(PreparedPackSnapshot {
        index: PackIndex {
            version: PACK_INDEX_VERSION,
            max_vmtime,
            packs,
        },
        retired_pack_ids: transport_retired,
        objects,
        deferred_members,
    })
}

pub fn serialize_transport_pack_index(index: &PackIndex) -> Result<Vec<u8>, String> {
    let bytes = serialize_pack_index(index)
        .map_err(|error| error.to_string())?
        .into_bytes();
    if bytes.len() as u64 > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES {
        return Err("iOS Box Store pack index exceeds the Agent Store 8 MiB transport cap".into());
    }
    Ok(bytes)
}

pub fn pack_is_worth_downloading(
    pack: &PackEntry,
    wanted: &BTreeMap<String, u64>,
) -> bool {
    if pack.bytes > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES {
        return false;
    }
    let selected = pack
        .members
        .iter()
        .filter(|member| wanted.get(&member.sha) == Some(&member.size))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return false;
    }
    let compressed = selected
        .iter()
        .map(|member| member.clen)
        .fold(0_u64, u64::saturating_add);
    let uncompressed = selected
        .iter()
        .map(|member| member.size)
        .fold(0_u64, u64::saturating_add);
    should_restore_from_pack(selected.len(), compressed, uncompressed, pack.bytes)
}

struct CollectSink {
    wanted: BTreeMap<String, u64>,
    blobs: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl PackExtractionSink for CollectSink {
    fn wants(&self, member: &PackMember) -> bool {
        self.wanted.get(&member.sha) == Some(&member.size)
    }

    fn on_blob(&self, member: &PackMember, bytes: Vec<u8>) -> Result<(), String> {
        self.blobs
            .lock()
            .map_err(|_| "iOS Box Store pack collector lock poisoned".to_string())?
            .insert(member.sha.clone(), bytes);
        Ok(())
    }
}

pub fn extract_transport_pack(
    temp_base: &Path,
    pack: &PackEntry,
    bytes: &[u8],
    wanted: &BTreeMap<String, u64>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    if !pack_is_worth_downloading(pack, wanted) {
        return Ok(BTreeMap::new());
    }
    if bytes.len() as u64 != pack.bytes
        || bytes.len() as u64 > IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES
    {
        return Err("iOS Box Store pack download size does not match bounded index metadata".into());
    }
    if pack.id.len() != 64
        || !pack.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || sha256_hex(bytes) != pack.id.to_ascii_lowercase()
    {
        return Err("iOS Box Store pack content address does not match downloaded bytes".into());
    }

    let temp_root = temp_dir(temp_base);
    fs::create_dir_all(&temp_root).map_err(|error| error.to_string())?;
    cleanup_stale_pack_temp_dir(&temp_root);
    let temp_path = temp_root.join(format!(
        "ios-read-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::write(&temp_path, bytes).map_err(|error| error.to_string())?;
    let sink = Arc::new(CollectSink {
        wanted: wanted.clone(),
        blobs: Mutex::new(BTreeMap::new()),
    });
    let result = extract_pack_members(
        &temp_path,
        &pack.members,
        PACK_EXTRACT_CONCURRENCY,
        sink.clone(),
    );
    let _ = fs::remove_file(&temp_path);
    let result = result?;
    if result.mismatched > 0 {
        return Err(format!(
            "iOS Box Store pack extraction rejected {} corrupt member(s)",
            result.mismatched
        ));
    }
    sink.blobs
        .lock()
        .map_err(|_| "iOS Box Store pack collector lock poisoned".to_string())
        .map(|blobs| blobs.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::box_store_hydration::snapshot_local_agent_root;

    fn root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-ios-pack-runtime-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn transport_chunking_respects_worst_case_gzip_expansion_cap() {
        let planned = (0..16)
            .map(|index| PlannedPackMember {
                sha: format!("{index:064x}"),
                size: 1024 * 1024,
                vmtime: index + 1,
            })
            .collect::<Vec<_>>();
        let (chunks, deferred) = transport_chunks(&planned);
        assert_eq!(deferred, 0);
        assert!(chunks.len() >= 2);
        for chunk in chunks {
            let worst = chunk
                .iter()
                .map(|member| pack_member_clen_bound(member.size))
                .sum::<u64>();
            assert!(worst <= IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES);
        }
    }

    #[test]
    fn prepared_pack_round_trip_restores_sixteen_small_members() {
        let root = root("round-trip");
        let target = root.join("agent");
        fs::create_dir_all(&target).unwrap();
        for index in 0..16_u64 {
            fs::write(
                target.join(format!("member-{index}.txt")),
                format!("member-{index}-{}", "x".repeat(index as usize + 1)),
            )
            .unwrap();
        }
        let entries = snapshot_local_agent_root(&target).expect("snapshot");
        let prepared =
            prepare_pack_snapshot(&root, &target, &entries, None).expect("prepare pack");
        assert!(!prepared.objects.is_empty());
        assert!(prepared
            .objects
            .iter()
            .all(|object| object.bytes.len() as u64 <= IOS_AGENT_STORE_PACK_TRANSPORT_MAX_BYTES));

        let wanted = entries
            .values()
            .filter_map(|entry| match entry {
                BoxStoreManifestEntry::LegacyFile { sha, size }
                | BoxStoreManifestEntry::File { sha, size, .. } => Some((sha.clone(), *size)),
                BoxStoreManifestEntry::Symlink { .. } => None,
            })
            .collect::<BTreeMap<_, _>>();
        let first = &prepared.objects[0];
        assert!(pack_is_worth_downloading(&first.entry, &wanted));
        let extracted =
            extract_transport_pack(&root, &first.entry, &first.bytes, &wanted).expect("extract");
        assert_eq!(extracted.len(), first.entry.members.len());
        for member in &first.entry.members {
            let bytes = extracted.get(&member.sha).expect("member bytes");
            assert_eq!(bytes.len() as u64, member.size);
            assert_eq!(sha256_hex(bytes), member.sha);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_old_pack_is_retired_without_removing_loose_fallback() {
        let root = root("retire");
        let target = root.join("agent");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("live.txt"), b"live").unwrap();
        let entries = snapshot_local_agent_root(&target).expect("snapshot");
        let old = PackIndex {
            version: PACK_INDEX_VERSION,
            max_vmtime: 2,
            packs: vec![PackEntry {
                id: "1".repeat(64),
                bytes: 2,
                members: vec![
                    PackMember {
                        sha: "2".repeat(64),
                        size: 4,
                        offset: 0,
                        clen: 1,
                        vmtime: 1,
                    },
                    PackMember {
                        sha: "3".repeat(64),
                        size: 4,
                        offset: 1,
                        clen: 1,
                        vmtime: 2,
                    },
                ],
            }],
        };
        let prepared =
            prepare_pack_snapshot(&root, &target, &entries, Some(&old)).expect("prepare");
        assert_eq!(prepared.retired_pack_ids, vec!["1".repeat(64)]);
        assert!(prepared.index.packs.iter().all(|pack| pack.id != "1".repeat(64)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pack_download_heuristic_keeps_loose_blob_fallback_for_low_value_pack() {
        let wanted = BTreeMap::from([("a".to_string(), 1_u64)]);
        let pack = PackEntry {
            id: "0".repeat(64),
            bytes: 80,
            members: vec![PackMember {
                sha: "a".into(),
                size: 1,
                offset: 0,
                clen: 9,
                vmtime: 1,
            }],
        };
        assert!(!pack_is_worth_downloading(&pack, &wanted));
    }
}
