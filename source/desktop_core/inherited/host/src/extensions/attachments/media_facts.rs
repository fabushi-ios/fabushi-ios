use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

pub const PERSISTED_LEGACY_MEDIA_KEYS: &[&str] = &[
    "MediaPath",
    "MediaPaths",
    "MediaUrl",
    "MediaUrls",
    "MediaType",
    "MediaTypes",
    "MediaTranscribedIndexes",
    "MediaStaged",
    "MediaWorkspaceDir",
];

pub const ALL_LEGACY_MEDIA_CONTEXT_KEYS: &[&str] = &[
    "MediaPath",
    "MediaPaths",
    "MediaUrl",
    "MediaUrls",
    "MediaType",
    "MediaTypes",
    "MediaDir",
    "MediaTranscribedIndexes",
    "MediaStaged",
    "MediaWorkspaceDir",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaFact {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(rename = "fileName", skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(rename = "sizeBytes", skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcribed: Option<bool>,
    #[serde(rename = "messageId", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(rename = "workspaceDir", skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged: Option<bool>,
    #[serde(rename = "hydrationSuppressed", skip_serializing_if = "Option::is_none")]
    pub hydration_suppressed: Option<bool>,
}

pub fn normalize_string(opt: Option<&Value>) -> Option<String> {
    match opt {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }
        _ => None,
    }
}

pub fn normalize_mime(mime: &str) -> String {
    mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

pub fn is_generic_binary_content_type(ct: Option<&str>) -> bool {
    match ct {
        Some(s) => {
            let n = normalize_mime(s);
            n == "application/octet-stream" || n == "binary/octet-stream"
        }
        None => false,
    }
}

pub fn kind_from_mime(mime: &str) -> Option<&'static str> {
    let n = normalize_mime(mime);
    if n.starts_with("image/") {
        Some("image")
    } else if n.starts_with("audio/") {
        Some("audio")
    } else if n.starts_with("video/") {
        Some("video")
    } else if n == "application/pdf"
        || n == "text/plain"
        || n == "text/markdown"
        || n == "text/csv"
        || n == "application/msword"
        || n.contains("document")
        || n.contains("sheet")
        || n.contains("presentation")
    {
        Some("document")
    } else {
        None
    }
}

pub fn kind_from_file_path(path_str: &str) -> Option<&'static str> {
    let lower = path_str.to_ascii_lowercase();
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
        || lower.ends_with(".ico")
        || lower.ends_with(".tif")
        || lower.ends_with(".tiff")
    {
        Some("image")
    } else if lower.ends_with(".mp3")
        || lower.ends_with(".wav")
        || lower.ends_with(".m4a")
        || lower.ends_with(".aac")
        || lower.ends_with(".ogg")
        || lower.ends_with(".flac")
    {
        Some("audio")
    } else if lower.ends_with(".mp4")
        || lower.ends_with(".mov")
        || lower.ends_with(".mkv")
        || lower.ends_with(".webm")
        || lower.ends_with(".avi")
    {
        Some("video")
    } else if lower.ends_with(".pdf")
        || lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".csv")
        || lower.ends_with(".doc")
        || lower.ends_with(".docx")
    {
        Some("document")
    } else {
        None
    }
}

pub fn resolve_media_fact_kind(fact: &MediaFact) -> Option<String> {
    if let Some(ref k) = fact.kind {
        if k != "unknown" && !k.is_empty() {
            return Some(k.clone());
        }
    }

    if let Some(ref ct) = fact.content_type {
        if !is_generic_binary_content_type(Some(ct)) {
            if let Some(k) = kind_from_mime(ct) {
                return Some(k.to_string());
            }
            let norm = normalize_mime(ct);
            if matches!(
                norm.as_str(),
                "image" | "audio" | "video" | "document" | "sticker"
            ) {
                return Some(norm);
            }
        }
    }

    let candidates = [
        fact.path.as_deref(),
        fact.url.as_deref(),
        fact.file_name.as_deref(),
    ];
    for cand in candidates.into_iter().flatten() {
        let lower = cand.to_ascii_lowercase();
        if lower.ends_with(".svg") {
            return None;
        }
        if let Some(k) = kind_from_file_path(cand) {
            return Some(k.to_string());
        }
    }

    None
}

pub fn is_meaningful_media_fact(fact: &MediaFact) -> bool {
    fact.path.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || fact.url.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || fact.content_type.is_some()
        || fact.kind.as_ref().map(|k| k != "unknown" && !k.is_empty()).unwrap_or(false)
}

pub fn has_staged_media_facts(facts: &[MediaFact]) -> bool {
    let stageable: Vec<&MediaFact> = facts.iter().filter(|f| f.path.is_some()).collect();
    !stageable.is_empty()
        && stageable.iter().all(|f| f.workspace_dir.is_some() || f.staged == Some(true))
}

pub fn is_image_media_fact(fact: &MediaFact) -> bool {
    let k = resolve_media_fact_kind(fact);
    matches!(k.as_deref(), Some("image") | Some("sticker"))
}

pub fn is_video_media_fact(fact: &MediaFact) -> bool {
    let k = resolve_media_fact_kind(fact);
    matches!(k.as_deref(), Some("video"))
}

pub fn strip_legacy_media_context_fields(map: &mut Map<String, Value>) {
    for key in ALL_LEGACY_MEDIA_CONTEXT_KEYS {
        map.remove(*key);
    }
}

pub fn has_meaningful_retired_media_carrier(message: &Value) -> bool {
    let Some(obj) = message.as_object() else {
        return false;
    };

    let mut legacy_obj = Map::new();
    if let Some(media) = obj.get("media") {
        legacy_obj.insert("media".to_string(), media.clone());
    }
    for key in PERSISTED_LEGACY_MEDIA_KEYS {
        if let Some(v) = obj.get(*key) {
            legacy_obj.insert(key.to_string(), v.clone());
        }
    }

    if let Ok(facts) = resolve_media_facts(&legacy_obj) {
        if facts.iter().any(is_meaningful_media_fact) {
            return true;
        }
    }

    let canonical = read_persisted_media_facts(message).unwrap_or_default();
    if canonical.is_empty() {
        return false;
    }

    let transcribed_indexes = obj
        .get("MediaTranscribedIndexes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64().map(|n| n as usize))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    transcribed_indexes.iter().any(|&idx| idx < canonical.len())
        || normalize_string(obj.get("MediaWorkspaceDir")).is_some()
        || obj.get("MediaStaged").and_then(|v| v.as_bool()) == Some(true)
}

pub fn read_persisted_media_facts(message: &Value) -> Option<Vec<MediaFact>> {
    let openclaw = message.as_object()?.get("__openclaw")?.as_object()?;
    let media_arr = openclaw.get("media")?.as_array()?;
    let facts: Vec<MediaFact> = media_arr
        .iter()
        .filter_map(|item| serde_json::from_value(item.clone()).ok())
        .collect();
    Some(facts)
}

pub fn resolve_media_facts(source: &Map<String, Value>) -> Result<Vec<MediaFact>, String> {
    let canonical: Vec<MediaFact> = match source.get("media").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .map(|item| serde_json::from_value(item.clone()).unwrap_or_default())
            .collect(),
        None => Vec::new(),
    };

    let paths: Vec<Option<String>> = source
        .get("MediaPaths")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(|item| normalize_string(Some(item))).collect())
        .unwrap_or_default();

    let urls: Vec<Option<String>> = source
        .get("MediaUrls")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(|item| normalize_string(Some(item))).collect())
        .unwrap_or_default();

    let types: Vec<Option<String>> = source
        .get("MediaTypes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(|item| normalize_string(Some(item))).collect())
        .unwrap_or_default();

    let single_path = normalize_string(source.get("MediaPath"));
    let single_url = normalize_string(source.get("MediaUrl"));
    let single_type = normalize_string(source.get("MediaType"));

    let slot_count = paths.len().max(urls.len());
    if !types.is_empty() && types.len() < slot_count {
        let mut ambiguous = false;
        for i in 0..slot_count {
            let meaningful = paths.get(i).cloned().flatten().is_some()
                || urls.get(i).cloned().flatten().is_some();
            let fact = canonical.get(i);
            if meaningful {
                let has_target = fact.map(|f| f.path.is_some() || f.url.is_some()).unwrap_or(false);
                let has_type = fact.map(|f| f.content_type.is_some() || f.kind.is_some()).unwrap_or(false);
                if !has_target || !has_type {
                    ambiguous = true;
                    break;
                }
            }
        }
        if ambiguous {
            return Err("legacy media arrays have ambiguous sparse positional alignment".to_string());
        }
    }

    let count = canonical
        .len()
        .max(paths.len())
        .max(urls.len())
        .max(types.len())
        .max(if single_path.is_some() || single_url.is_some() || single_type.is_some() { 1 } else { 0 });

    let transcribed_set: HashSet<usize> = source
        .get("MediaTranscribedIndexes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64().map(|n| n as usize))
                .collect()
        })
        .unwrap_or_default();

    let workspace_dir = normalize_string(source.get("MediaWorkspaceDir"));
    let staged = source.get("MediaStaged").and_then(|v| v.as_bool()).unwrap_or(false);

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let canon = canonical.get(i);
        let p = canon.and_then(|f| f.path.clone())
            .or_else(|| paths.get(i).cloned().flatten())
            .or_else(|| if i == 0 { single_path.clone() } else { None });
        let u = canon.and_then(|f| f.url.clone())
            .or_else(|| urls.get(i).cloned().flatten())
            .or_else(|| if i == 0 { single_url.clone() } else { None });
        let ct = canon.and_then(|f| f.content_type.clone())
            .or_else(|| types.get(i).cloned().flatten())
            .or_else(|| if i == 0 { single_type.clone() } else { None });
        let is_transcribed = canon.and_then(|f| f.transcribed).unwrap_or(false) || transcribed_set.contains(&i);

        let fact = MediaFact {
            path: p,
            url: u,
            content_type: ct,
            kind: canon.and_then(|f| f.kind.clone()),
            file_name: canon.and_then(|f| f.file_name.clone()),
            origin: canon.and_then(|f| f.origin.clone()),
            size_bytes: canon.and_then(|f| f.size_bytes),
            duration_ms: canon.and_then(|f| f.duration_ms),
            width: canon.and_then(|f| f.width),
            height: canon.and_then(|f| f.height),
            transcribed: if is_transcribed { Some(true) } else { None },
            message_id: canon.and_then(|f| f.message_id.clone()),
            workspace_dir: canon.and_then(|f| f.workspace_dir.clone()).or_else(|| workspace_dir.clone()),
            staged: if canon.and_then(|f| f.staged) == Some(true) || staged { Some(true) } else { None },
            hydration_suppressed: canon.and_then(|f| f.hydration_suppressed),
        };
        result.push(fact);
    }

    Ok(result)
}

pub fn canonicalize_persisted_user_message_media(message: &mut Value) -> Result<bool, String> {
    let Some(obj) = message.as_object_mut() else {
        return Ok(false);
    };

    let had_legacy = PERSISTED_LEGACY_MEDIA_KEYS.iter().any(|k| obj.contains_key(*k));
    let had_top_level_media = obj.contains_key("media");
    let had_canonical = obj
        .get("__openclaw")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("media"))
        .is_some();

    if !had_legacy && !had_top_level_media && !had_canonical {
        return Ok(false);
    }

    let facts = resolve_media_facts(obj)?;

    // Clean legacy keys
    obj.remove("media");
    for k in PERSISTED_LEGACY_MEDIA_KEYS {
        obj.remove(*k);
    }

    // Update __openclaw.media
    let mut openclaw = obj
        .get("__openclaw")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    if !facts.is_empty() || had_canonical || had_top_level_media {
        let serialized_facts: Vec<Value> = facts
            .into_iter()
            .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
            .collect();
        openclaw.insert("media".to_string(), Value::Array(serialized_facts));
    }

    if !openclaw.is_empty() {
        obj.insert("__openclaw".to_string(), Value::Object(openclaw));
    } else {
        obj.remove("__openclaw");
    }

    Ok(true)
}

pub fn project_media_facts(facts: &[MediaFact], mode: &str) -> Map<String, Value> {
    let mut out = Map::new();
    let is_channel = mode == "channel";

    let first = facts.first();
    if let Some(f) = first {
        if let Some(ref p) = f.path {
            out.insert("MediaPath".to_string(), Value::String(p.clone()));
        }
        let url_val = if is_channel {
            f.url.as_ref().or(f.path.as_ref())
        } else {
            f.path.as_ref()
        };
        if let Some(u) = url_val {
            out.insert("MediaUrl".to_string(), Value::String(u.clone()));
        }
        let type_val = f.content_type.as_ref().or_else(|| if is_channel { f.kind.as_ref() } else { None });
        if let Some(t) = type_val {
            out.insert("MediaType".to_string(), Value::String(t.clone()));
        }
    }

    let paths: Vec<Value> = facts.iter().map(|f| {
        f.path.as_ref().map(|s| Value::String(s.clone())).unwrap_or_else(|| Value::String("".to_string()))
    }).collect();
    if !paths.is_empty() && (paths.iter().any(|v| v != "") || !is_channel) {
        out.insert("MediaPaths".to_string(), Value::Array(paths));
    }

    let urls: Vec<Value> = facts.iter().map(|f| {
        let u = if is_channel { f.url.as_ref().or(f.path.as_ref()) } else { f.path.as_ref() };
        u.map(|s| Value::String(s.clone())).unwrap_or_else(|| Value::String("".to_string()))
    }).collect();
    if !urls.is_empty() && (urls.iter().any(|v| v != "") || !is_channel) {
        out.insert("MediaUrls".to_string(), Value::Array(urls));
    }

    let types: Vec<Value> = facts.iter().filter_map(|f| {
        let t = f.content_type.as_ref().or_else(|| if is_channel { f.kind.as_ref() } else { None });
        if is_channel && mode == "compact" {
            t.map(|s| Value::String(s.clone()))
        } else {
            Some(t.map(|s| Value::String(s.clone())).unwrap_or_else(|| Value::String("".to_string())))
        }
    }).collect();
    if !types.is_empty() && (types.iter().any(|v| v != "") || !is_channel) {
        out.insert("MediaTypes".to_string(), Value::Array(types));
    }

    if is_channel {
        let transcribed_indexes: Vec<Value> = facts
            .iter()
            .enumerate()
            .filter_map(|(idx, f)| if f.transcribed == Some(true) { Some(json!(idx)) } else { None })
            .collect();
        if !transcribed_indexes.is_empty() {
            out.insert("MediaTranscribedIndexes".to_string(), Value::Array(transcribed_indexes));
        }
    }

    out
}
