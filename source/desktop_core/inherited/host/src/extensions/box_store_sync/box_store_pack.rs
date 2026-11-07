use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BOX_STORE_PACKS_PREFIX: &str = "packs";
pub const BOX_STORE_PACK_INDEX_KEY: &str = "packs/index.json";
pub const BOX_STORE_PACK_RETIRED_KEY: &str = "packs/retired.json";
pub const PACK_INDEX_VERSION: u64 = 1;
pub const PACK_RETIRED_VERSION: u64 = 1;
pub const PACK_MEMBER_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const PACK_MAX_MEMBER_SIZE_SUM: u64 = 256 * 1024 * 1024;
pub const PACK_OBJECT_MAX_BYTES: u64 = 320 * 1024 * 1024;
pub const PACK_INDEX_MAX_BYTES: u64 = 64 * 1024 * 1024;

pub const fn pack_member_clen_bound(size: u64) -> u64 {
    size + 4_096 + size.div_ceil(512)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMember {
    pub sha: String,
    pub size: u64,
    pub offset: u64,
    pub clen: u64,
    pub vmtime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackEntry {
    pub id: String,
    pub bytes: u64,
    pub members: Vec<PackMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackIndex {
    pub version: u64,
    pub max_vmtime: u64,
    pub packs: Vec<PackEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PackRetired {
    version: u64,
    retired: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSource {
    pub abs_path: PathBuf,
    pub sha: String,
    pub size: u64,
    pub vmtime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackBuildResult {
    pub members: Vec<PackMember>,
    pub file_bytes: u64,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackExtractResult {
    pub extracted: usize,
    pub mismatched: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPackMember {
    pub sha: String,
    pub size: u64,
    pub vmtime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMaintenancePlan {
    pub kept_packs: Vec<PackEntry>,
    pub retired_pack_ids: Vec<String>,
    pub new_packs: Vec<Vec<PlannedPackMember>>,
    pub deferred_members: usize,
    pub new_max_vmtime: u64,
}

pub trait PackAdmissionPermit: Send {}
impl<T: Send> PackAdmissionPermit for T {}

pub trait PackExtractionSink: Send + Sync {
    fn wants(&self, member: &PackMember) -> bool;
    fn admit_bytes(
        &self,
        _bytes: u64,
    ) -> Result<Option<Box<dyn PackAdmissionPermit>>, String> {
        Ok(None)
    }
    fn on_blob(&self, member: &PackMember, bytes: Vec<u8>) -> Result<(), String>;
}

pub fn validate_pack_member(member: &PackMember) -> bool {
    !member.sha.is_empty()
        && member.size <= PACK_MEMBER_MAX_BYTES
        && member.clen <= pack_member_clen_bound(member.size)
}

pub fn validate_pack_entry(pack: &PackEntry) -> bool {
    if pack.id.is_empty() || pack.bytes > PACK_OBJECT_MAX_BYTES {
        return false;
    }
    let mut size_sum = 0_u64;
    for member in &pack.members {
        if !validate_pack_member(member) {
            return false;
        }
        let Some(end) = member.offset.checked_add(member.clen) else {
            return false;
        };
        if end > pack.bytes {
            return false;
        }
        let Some(next) = size_sum.checked_add(member.size) else {
            return false;
        };
        size_sum = next;
        if size_sum > PACK_MAX_MEMBER_SIZE_SUM {
            return false;
        }
    }
    true
}

pub fn parse_pack_index(raw: &str) -> Option<PackIndex> {
    if raw.len() as u64 > PACK_INDEX_MAX_BYTES {
        return None;
    }
    let index: PackIndex = serde_json::from_str(raw).ok()?;
    if index.version != PACK_INDEX_VERSION
        || index.packs.iter().any(|pack| !validate_pack_entry(pack))
    {
        return None;
    }
    Some(index)
}

pub fn serialize_pack_index(index: &PackIndex) -> Result<String, serde_json::Error> {
    serde_json::to_string(index)
}

pub fn parse_pack_retired(raw: &str) -> Option<Vec<String>> {
    let value: PackRetired = serde_json::from_str(raw).ok()?;
    if value.version != PACK_RETIRED_VERSION
        || value.retired.iter().any(|id| id.is_empty())
    {
        return None;
    }
    Some(value.retired)
}

pub fn serialize_pack_retired(retired: &[String]) -> Result<String, serde_json::Error> {
    serde_json::to_string(&PackRetired {
        version: PACK_RETIRED_VERSION,
        retired: retired.to_vec(),
    })
}

pub fn build_pack_file<F>(
    dest_path: &Path,
    sources: &[PackSource],
    should_abort: F,
) -> Result<Option<PackBuildResult>, String>
where
    F: Fn() -> bool,
{
    let mut output = File::create(dest_path).map_err(|error| error.to_string())?;
    let mut members = Vec::new();
    let mut offset = 0_u64;
    let mut skipped = 0_usize;

    for source in sources {
        if should_abort() {
            drop(output);
            let _ = fs::remove_file(dest_path);
            return Ok(None);
        }
        let bytes = match fs::read(&source.abs_path) {
            Ok(bytes) => bytes,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if bytes.len() as u64 != source.size || sha256_hex(&bytes) != source.sha {
            skipped += 1;
            continue;
        }
        if source.size > PACK_MEMBER_MAX_BYTES {
            skipped += 1;
            continue;
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        let compressed = encoder.finish().map_err(|error| error.to_string())?;
        let clen = compressed.len() as u64;
        if clen > pack_member_clen_bound(source.size) {
            return Err("compressed member exceeds gzip expansion bound".into());
        }
        output
            .write_all(&compressed)
            .map_err(|error| error.to_string())?;
        members.push(PackMember {
            sha: source.sha.clone(),
            size: source.size,
            offset,
            clen,
            vmtime: source.vmtime,
        });
        offset = offset
            .checked_add(clen)
            .ok_or_else(|| "pack offset overflow".to_string())?;
        if offset > PACK_OBJECT_MAX_BYTES {
            return Err("pack object exceeds the maximum byte cap".into());
        }
    }
    output.flush().map_err(|error| error.to_string())?;
    drop(output);
    let file_bytes = fs::metadata(dest_path)
        .map_err(|error| error.to_string())?
        .len();
    Ok(Some(PackBuildResult {
        members,
        file_bytes,
        skipped,
    }))
}

pub fn extract_pack_members(
    pack_path: &Path,
    members: &[PackMember],
    concurrency: usize,
    sink: Arc<dyn PackExtractionSink>,
) -> Result<PackExtractResult, String> {
    let selected = members
        .iter()
        .filter(|member| sink.wants(member))
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(PackExtractResult {
            extracted: 0,
            mismatched: 0,
        });
    }
    if selected.iter().any(|member| !validate_pack_member(member)) {
        return Err("invalid pack member".into());
    }

    let workers = concurrency.max(1).min(selected.len());
    let selected = Arc::new(selected);
    let next = Arc::new(AtomicUsize::new(0));
    let extracted = Arc::new(AtomicUsize::new(0));
    let mismatched = Arc::new(AtomicUsize::new(0));
    let failure = Arc::new(Mutex::new(None::<String>));
    let path = pack_path.to_path_buf();
    let mut handles = Vec::with_capacity(workers);

    for _ in 0..workers {
        let selected = Arc::clone(&selected);
        let next = Arc::clone(&next);
        let extracted = Arc::clone(&extracted);
        let mismatched = Arc::clone(&mismatched);
        let failure = Arc::clone(&failure);
        let sink = Arc::clone(&sink);
        let path = path.clone();
        handles.push(thread::spawn(move || {
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) => {
                    *failure.lock().unwrap() = Some(error.to_string());
                    return;
                }
            };
            loop {
                if failure.lock().unwrap().is_some() {
                    return;
                }
                let index = next.fetch_add(1, Ordering::AcqRel);
                let Some(member) = selected.get(index) else {
                    return;
                };
                let _permit = match sink.admit_bytes(member.clen.saturating_add(member.size)) {
                    Ok(permit) => permit,
                    Err(error) => {
                        *failure.lock().unwrap() = Some(error);
                        return;
                    }
                };
                if let Err(error) = file.seek(SeekFrom::Start(member.offset)) {
                    *failure.lock().unwrap() = Some(error.to_string());
                    return;
                }
                let Ok(clen) = usize::try_from(member.clen) else {
                    mismatched.fetch_add(1, Ordering::AcqRel);
                    continue;
                };
                let mut compressed = vec![0_u8; clen];
                if file.read_exact(&mut compressed).is_err() {
                    mismatched.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                let mut decoder = GzDecoder::new(compressed.as_slice());
                let mut limited = decoder.by_ref().take(member.size.saturating_add(1));
                let mut bytes = Vec::with_capacity(member.size.min(usize::MAX as u64) as usize);
                if limited.read_to_end(&mut bytes).is_err()
                    || bytes.len() as u64 != member.size
                    || sha256_hex(&bytes) != member.sha
                {
                    mismatched.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                if let Err(error) = sink.on_blob(member, bytes) {
                    *failure.lock().unwrap() = Some(error);
                    return;
                }
                extracted.fetch_add(1, Ordering::AcqRel);
            }
        }));
    }

    for handle in handles {
        if handle.join().is_err() {
            return Err("pack extraction worker panicked".into());
        }
    }
    if let Some(error) = failure.lock().unwrap().take() {
        return Err(error);
    }
    Ok(PackExtractResult {
        extracted: extracted.load(Ordering::Acquire),
        mismatched: mismatched.load(Ordering::Acquire),
    })
}

pub fn plan_pack_maintenance(
    index: Option<&PackIndex>,
    live: &HashMap<String, u64>,
    eligible: &HashMap<String, u64>,
    max_pack_member_size_sum: u64,
    min_pack_members: usize,
    min_pack_bytes: u64,
) -> PackMaintenancePlan {
    #[derive(Clone)]
    struct Candidate {
        sha: String,
        size: u64,
        survivor: bool,
    }

    let mut kept_packs = Vec::new();
    let mut retired_pack_ids = Vec::new();
    let mut survivors = Vec::<(String, u64)>::new();
    let mut packed_shas = HashSet::<String>::new();

    for pack in index.map(|index| index.packs.as_slice()).unwrap_or(&[]) {
        let all_live = !pack.members.is_empty()
            && pack
                .members
                .iter()
                .all(|member| live.get(&member.sha) == Some(&member.size));
        if all_live {
            kept_packs.push(pack.clone());
            packed_shas.extend(pack.members.iter().map(|member| member.sha.clone()));
        } else {
            retired_pack_ids.push(pack.id.clone());
            survivors.extend(
                pack.members
                    .iter()
                    .filter(|member| live.get(&member.sha) == Some(&member.size))
                    .map(|member| (member.sha.clone(), member.size)),
            );
        }
    }

    let untouched_max = kept_packs
        .iter()
        .flat_map(|pack| pack.members.iter().map(|member| member.vmtime))
        .max()
        .unwrap_or(0);
    let survivor_shas = survivors
        .iter()
        .map(|(sha, _)| sha.clone())
        .collect::<HashSet<_>>();
    let mut newbies = eligible
        .iter()
        .filter(|(sha, _)| !packed_shas.contains(*sha) && !survivor_shas.contains(*sha))
        .map(|(sha, size)| (sha.clone(), *size))
        .collect::<Vec<_>>();
    survivors.sort_by(|left, right| left.0.cmp(&right.0));
    newbies.sort_by(|left, right| left.0.cmp(&right.0));

    let candidates = survivors
        .into_iter()
        .map(|(sha, size)| Candidate {
            sha,
            size,
            survivor: true,
        })
        .chain(newbies.into_iter().map(|(sha, size)| Candidate {
            sha,
            size,
            survivor: false,
        }))
        .collect::<Vec<_>>();

    let mut groups = Vec::<Vec<Candidate>>::new();
    let mut current = Vec::<Candidate>::new();
    let mut current_bytes = 0_u64;
    for member in candidates {
        if !current.is_empty()
            && current_bytes.saturating_add(member.size) > max_pack_member_size_sum
        {
            groups.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(member.size);
        current.push(member);
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let mut deferred_members = 0;
    if let Some(last) = groups.last() {
        let bytes = last.iter().map(|member| member.size).sum::<u64>();
        if last.len() < min_pack_members && bytes < min_pack_bytes {
            deferred_members = last.len();
            groups.pop();
        }
    }

    let base_max = index.map(|index| index.max_vmtime).unwrap_or(0);
    let mut next_vmtime = base_max.max(untouched_max);
    let new_packs = groups
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .map(|member| {
                    let vmtime = if member.survivor {
                        untouched_max
                    } else {
                        next_vmtime = next_vmtime.saturating_add(1);
                        next_vmtime
                    };
                    PlannedPackMember {
                        sha: member.sha,
                        size: member.size,
                        vmtime,
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    PackMaintenancePlan {
        kept_packs,
        retired_pack_ids,
        new_packs,
        deferred_members,
        new_max_vmtime: next_vmtime.max(untouched_max).max(base_max),
    }
}

pub fn is_box_store_pack_build_enabled(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "false" | "no")
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}
