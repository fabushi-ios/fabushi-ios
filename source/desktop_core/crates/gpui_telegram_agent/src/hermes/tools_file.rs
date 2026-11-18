use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::security::scan_content_for_injection;
use super::toolsets::{HermesToolContext, HermesToolResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshotEntry {
    pub path: PathBuf,
    pub existed_before: bool,
    pub original_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCheckpoint {
    pub session_id: String,
    pub turn_index: u64,
    pub entries: Vec<FileSnapshotEntry>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCheckpointSummary {
    pub session_id: String,
    pub turn_index: u64,
    pub file_count: usize,
    pub files: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackReport {
    pub session_id: String,
    pub turn_index: u64,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
}

/// Automatically snapshots file contents (or marks non-existent newly created files)
/// before `write_file` / `patch` edits so `/rollback` / `rollback_turn` can atomically revert.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FileCheckpointManager {
    checkpoints: HashMap<String, Vec<TurnCheckpoint>>,
}

impl FileCheckpointManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Captures the pre-edit snapshot for `path` on `(session_id, turn_index)`.
    /// Subsequent edits to the same file within the same turn keep the original pre-turn state.
    pub fn snapshot_before_edit(
        &mut self,
        session_id: &str,
        turn_index: u64,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, String> {
        let turns = self.checkpoints.entry(session_id.to_string()).or_default();
        let turn_cp = match turns.iter_mut().find(|t| t.turn_index == turn_index) {
            Some(cp) => cp,
            None => {
                turns.push(TurnCheckpoint {
                    session_id: session_id.to_string(),
                    turn_index,
                    entries: Vec::new(),
                    created_at: Utc::now(),
                });
                turns.last_mut().expect("just pushed")
            }
        };

        if let Some(existing) = turn_cp.entries.iter().find(|e| e.path == path) {
            return Ok(existing.original_bytes.clone());
        }

        let (existed_before, original_bytes) = if path.exists() {
            let bytes = fs::read(path)
                .map_err(|e| format!("Failed to snapshot '{}': {e}", path.display()))?;
            (true, Some(bytes))
        } else {
            (false, None)
        };

        turn_cp.entries.push(FileSnapshotEntry {
            path: path.to_path_buf(),
            existed_before,
            original_bytes: original_bytes.clone(),
        });

        Ok(original_bytes)
    }

    /// Atomically restores modified files and deletes newly created files for `turn_index`
    /// (or the most recent checkpointed turn if `turn_index` is `None`).
    pub fn rollback_turn(
        &mut self,
        session_id: &str,
        turn_index: Option<u64>,
    ) -> Result<RollbackReport, String> {
        let turns = self
            .checkpoints
            .get_mut(session_id)
            .ok_or_else(|| format!("No file checkpoints recorded for session '{session_id}'"))?;

        if turns.is_empty() {
            return Err(format!(
                "No file checkpoints remaining to roll back for session '{session_id}'"
            ));
        }

        let pos = match turn_index {
            Some(idx) => turns
                .iter()
                .rposition(|t| t.turn_index == idx)
                .ok_or_else(|| format!("No checkpoint found for turn {idx}"))?,
            None => turns.len() - 1,
        };

        let turn_cp = turns.remove(pos);
        let mut restored_files = Vec::new();
        let mut deleted_files = Vec::new();

        for entry in turn_cp.entries.into_iter().rev() {
            if entry.existed_before {
                if let Some(parent) = entry.path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let bytes = entry.original_bytes.unwrap_or_default();
                fs::write(&entry.path, &bytes).map_err(|e| {
                    format!("Failed to restore '{}': {e}", entry.path.display())
                })?;
                restored_files.push(entry.path.display().to_string());
            } else {
                if entry.path.exists() {
                    fs::remove_file(&entry.path).map_err(|e| {
                        format!("Failed to remove created file '{}': {e}", entry.path.display())
                    })?;
                }
                deleted_files.push(entry.path.display().to_string());
            }
        }

        Ok(RollbackReport {
            session_id: session_id.to_string(),
            turn_index: turn_cp.turn_index,
            restored_files,
            deleted_files,
        })
    }

    pub fn list_checkpoints(&self, session_id: &str) -> Vec<TurnCheckpointSummary> {
        self.checkpoints
            .get(session_id)
            .map(|turns| {
                turns
                    .iter()
                    .map(|t| TurnCheckpointSummary {
                        session_id: t.session_id.clone(),
                        turn_index: t.turn_index,
                        file_count: t.entries.len(),
                        files: t
                            .entries
                            .iter()
                            .map(|e| e.path.display().to_string())
                            .collect(),
                        created_at: t.created_at,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn from_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pending" => Some(Self::Pending),
            "in_progress" | "active" => Some(Self::InProgress),
            "completed" | "done" => Some(Self::Completed),
            "cancelled" | "canceled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_number: usize,
    pub description: String,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSpec {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TodoStore {
    pub by_session: HashMap<String, Vec<TodoItem>>,
    pub plans_by_session: HashMap<String, Vec<PlanSpec>>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self::default()
    }
}

pub async fn execute_file_tool(
    name: &str,
    args: Value,
    ctx: &HermesToolContext,
) -> HermesToolResult {
    match name {
        "read_file" => tool_read_file(args, ctx),
        "write_file" => tool_write_file(args, ctx).await,
        "patch" => tool_patch(args, ctx).await,
        "search_files" => tool_search_files(args, ctx),
        "list_dir" => tool_list_dir(args, ctx),
        "workspace_rollback" => tool_workspace_rollback(args, ctx),
        "todo_write" => tool_todo_write(args, ctx),
        "todo_read" => tool_todo_read(ctx),
        "plan_create" => tool_plan_create(args, ctx),
        _ => HermesToolResult::err(name, format!("Unknown file/workspace tool '{name}'")),
    }
}

fn validate_ctx_path(raw: &str, ctx: &HermesToolContext) -> Result<PathBuf, String> {
    let sec = ctx
        .security
        .read()
        .map_err(|_| "SecurityEngine lock poisoned".to_string())?;
    sec.validate_path(Path::new(raw))
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(8192);
    let sample = &bytes[..sample_len];
    if sample.contains(&0u8) {
        return true;
    }
    std::str::from_utf8(bytes).is_err()
}

fn tool_read_file(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(raw_path) = args.get("path").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("read_file", "Missing required 'path' parameter");
    };
    let resolved = match validate_ctx_path(raw_path, ctx) {
        Ok(p) => p,
        Err(e) => return HermesToolResult::err("read_file", e),
    };
    let bytes = match fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            return HermesToolResult::err(
                "read_file",
                format!("Failed to read '{}': {e}", resolved.display()),
            )
        }
    };
    if is_probably_binary(&bytes) {
        return HermesToolResult::ok(
            "read_file",
            format!(
                "[Binary file '{}' ({} bytes) — textual display skipped]",
                resolved.display(),
                bytes.len()
            ),
            Some(json!({
                "path": resolved.display().to_string(),
                "binary": true,
                "byte_len": bytes.len(),
            })),
        );
    }

    let raw_text = String::from_utf8_lossy(&bytes).into_owned();
    let scan = scan_content_for_injection(&raw_text);
    let is_env_file = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == ".env" || n.starts_with(".env."));
    let display_text = if is_env_file || scan.secrets_detected > 0 {
        scan.sanitized_text.clone()
    } else {
        raw_text
    };

    let all_lines: Vec<&str> = display_text.lines().collect();
    let total_lines = all_lines.len();
    let start_line = args
        .get("start_line")
        .or_else(|| args.get("offset"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(400)
        .max(1) as usize;
    let end_line = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).min(total_lines))
        .unwrap_or_else(|| (start_line + limit - 1).min(total_lines));
    let with_line_numbers = args
        .get("with_line_numbers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let mut out = String::new();
    let non_secret_injections: Vec<_> = scan
        .findings
        .iter()
        .filter(|f| f.rule_id != "secret_exposure")
        .collect();
    if !non_secret_injections.is_empty() {
        out.push_str(&format!(
            "[SECURITY NOTICE: {} potential prompt-injection pattern(s) detected in file content]\n",
            non_secret_injections.len()
        ));
    }

    if start_line <= total_lines && total_lines > 0 {
        let slice = &all_lines[(start_line - 1)..end_line.max(start_line - 1)];
        for (idx, line) in slice.iter().enumerate() {
            let line_no = start_line + idx;
            if with_line_numbers {
                out.push_str(&format!("{line_no:>5} | {line}\n"));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    HermesToolResult::ok(
        "read_file",
        out,
        Some(json!({
            "path": resolved.display().to_string(),
            "total_lines": total_lines,
            "start_line": start_line,
            "end_line": end_line,
            "secrets_redacted": scan.secrets_detected,
            "injection_flagged": !non_secret_injections.is_empty(),
        })),
    )
}

async fn snapshot_and_record(
    resolved: &Path,
    ctx: &HermesToolContext,
) -> Result<(), String> {
    let orig = {
        let mut mgr = ctx
            .checkpoints
            .lock()
            .map_err(|_| "FileCheckpointManager lock poisoned".to_string())?;
        mgr.snapshot_before_edit(&ctx.session_id, ctx.turn_index, resolved)?
    };
    if let Some(bridge) = &ctx.state_bridge {
        let path_str = resolved.display().to_string();
        let _ = bridge
            .record_checkpoint(&ctx.session_id, ctx.turn_index, &path_str, orig.as_deref())
            .await;
    }
    Ok(())
}

async fn tool_write_file(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(raw_path) = args.get("path").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("write_file", "Missing required 'path' parameter");
    };
    let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("write_file", "Missing required 'content' parameter");
    };
    let append = args.get("append").and_then(|v| v.as_bool()).unwrap_or(false);

    let resolved = match validate_ctx_path(raw_path, ctx) {
        Ok(p) => p,
        Err(e) => return HermesToolResult::err("write_file", e),
    };

    if let Err(e) = snapshot_and_record(&resolved, ctx).await {
        return HermesToolResult::err("write_file", e);
    }

    if let Some(parent) = resolved.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return HermesToolResult::err(
                "write_file",
                format!("Failed to create parent directories: {e}"),
            );
        }
    }

    let final_bytes = if append && resolved.exists() {
        let mut existing = fs::read(&resolved).unwrap_or_default();
        existing.extend_from_slice(content.as_bytes());
        existing
    } else {
        content.as_bytes().to_vec()
    };

    if let Err(e) = fs::write(&resolved, &final_bytes) {
        return HermesToolResult::err(
            "write_file",
            format!("Failed to write '{}': {e}", resolved.display()),
        );
    }

    HermesToolResult::ok(
        "write_file",
        format!(
            "Wrote {} bytes to '{}' (turn {} checkpoint recorded)",
            final_bytes.len(),
            resolved.display(),
            ctx.turn_index
        ),
        Some(json!({
            "path": resolved.display().to_string(),
            "bytes_written": final_bytes.len(),
            "turn_index": ctx.turn_index,
        })),
    )
}

async fn tool_patch(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(raw_path) = args.get("path").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("patch", "Missing required 'path' parameter");
    };
    let resolved = match validate_ctx_path(raw_path, ctx) {
        Ok(p) => p,
        Err(e) => return HermesToolResult::err("patch", e),
    };
    let original = match fs::read_to_string(&resolved) {
        Ok(s) => s,
        Err(e) => {
            return HermesToolResult::err(
                "patch",
                format!("Failed to read '{}' for patching: {e}", resolved.display()),
            )
        }
    };

    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let (patched, strategy_used, hunks_applied) = if let Some(v4a_text) =
        args.get("patch").and_then(|v| v.as_str())
    {
        match apply_v4a_or_unified_patch(&original, v4a_text) {
            Ok((out, count)) => (out, "v4a_hunk".to_string(), count),
            Err(e) => return HermesToolResult::err("patch", e),
        }
    } else if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
        let mut cur = original.clone();
        let mut applied = 0usize;
        let mut last_strat = "exact".to_string();
        for edit in edits {
            let old_t = edit
                .get("old_text")
                .or_else(|| edit.get("search"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_t = edit
                .get("new_text")
                .or_else(|| edit.get("replace"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match apply_fuzzy_search_replace(&cur, old_t, new_t, replace_all) {
                Ok((next, strat)) => {
                    cur = next;
                    last_strat = strat;
                    applied += 1;
                }
                Err(e) => return HermesToolResult::err("patch", e),
            }
        }
        (cur, last_strat, applied)
    } else {
        let old_text = args
            .get("old_text")
            .or_else(|| args.get("search"))
            .and_then(|v| v.as_str());
        let new_text = args
            .get("new_text")
            .or_else(|| args.get("replace"))
            .and_then(|v| v.as_str());
        let (Some(old_t), Some(new_t)) = (old_text, new_text) else {
            return HermesToolResult::err(
                "patch",
                "Provide either ('old_text', 'new_text'), 'edits', or 'patch' hunks",
            );
        };
        match apply_fuzzy_search_replace(&original, old_t, new_t, replace_all) {
            Ok((out, strat)) => (out, strat, 1),
            Err(e) => return HermesToolResult::err("patch", e),
        }
    };

    if let Err(e) = snapshot_and_record(&resolved, ctx).await {
        return HermesToolResult::err("patch", e);
    }

    if let Err(e) = fs::write(&resolved, patched.as_bytes()) {
        return HermesToolResult::err(
            "patch",
            format!("Failed to write patched '{}': {e}", resolved.display()),
        );
    }

    HermesToolResult::ok(
        "patch",
        format!(
            "Patched '{}' ({hunks_applied} hunk(s), strategy={strategy_used})",
            resolved.display()
        ),
        Some(json!({
            "path": resolved.display().to_string(),
            "strategy": strategy_used,
            "hunks_applied": hunks_applied,
            "turn_index": ctx.turn_index,
        })),
    )
}

/// 4-pass Hermes/V4A fuzzy search-and-replace:
/// 1) Exact match
/// 2) Line-trimmed match
/// 3) Normalized internal whitespace match
/// 4) Unicode punctuation normalization match
pub fn apply_fuzzy_search_replace(
    content: &str,
    search: &str,
    replace: &str,
    replace_all: bool,
) -> Result<(String, String), String> {
    if search.is_empty() {
        return Err("Search block cannot be empty".into());
    }

    // Pass 1: Exact substring match
    let exact_matches = content.match_indices(search).count();
    if exact_matches == 1 || (exact_matches > 1 && replace_all) {
        let updated = if replace_all {
            content.replace(search, replace)
        } else {
            content.replacen(search, replace, 1)
        };
        return Ok((updated, "exact".into()));
    } else if exact_matches > 1 && !replace_all {
        return Err(format!(
            "Ambiguous patch: found {exact_matches} exact matches; provide more context or set replace_all=true"
        ));
    }

    // Line-oriented fuzzy passes
    let newline = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let had_trailing_nl = content.ends_with('\n');
    let content_lines: Vec<&str> = content.lines().collect();
    let search_lines: Vec<&str> = search.lines().collect();
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return Err("Patch target block not found in file".into());
    }

    type LineNormFn = fn(&str) -> String;
    let passes: [(&str, LineNormFn); 3] = [
        ("line_trimmed", |s: &str| s.trim().to_string()),
        ("whitespace_normalized", normalize_ws_line),
        ("unicode_normalized", |s: &str| normalize_unicode_line(&normalize_ws_line(s))),
    ];

    for (strat_name, norm_fn) in passes {
        let norm_search: Vec<String> = search_lines.iter().map(|l| norm_fn(l)).collect();
        let norm_content: Vec<String> = content_lines.iter().map(|l| norm_fn(l)).collect();
        let mut match_starts = Vec::new();
        let window = norm_search.len();

        let mut i = 0;
        while i + window <= norm_content.len() {
            if norm_content[i..i + window] == norm_search[..] {
                match_starts.push(i);
                i += window;
            } else {
                i += 1;
            }
        }

        if match_starts.len() > 1 && !replace_all {
            return Err(format!(
                "Ambiguous fuzzy patch ({strat_name}): matched {} locations",
                match_starts.len()
            ));
        }

        if !match_starts.is_empty() {
            let replace_lines: Vec<&str> = replace.lines().collect();
            let mut result_lines: Vec<String> = Vec::with_capacity(content_lines.len());
            let mut cur = 0;
            for start in match_starts {
                while cur < start {
                    result_lines.push(content_lines[cur].to_string());
                    cur += 1;
                }
                // Preserve base indentation of the first matched line if replace block lacks it
                let matched_indent: String = content_lines[start]
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .collect();
                let search_first_indent: String = search_lines[0]
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .collect();

                for (r_idx, r_line) in replace_lines.iter().enumerate() {
                    if r_idx == 0 && search_first_indent.is_empty() && !matched_indent.is_empty() && !r_line.starts_with(&matched_indent) {
                        result_lines.push(format!("{matched_indent}{}", r_line.trim_start()));
                    } else {
                        result_lines.push((*r_line).to_string());
                    }
                }
                cur = start + window;
            }
            while cur < content_lines.len() {
                result_lines.push(content_lines[cur].to_string());
                cur += 1;
            }
            let mut out = result_lines.join(newline);
            if had_trailing_nl && !out.ends_with('\n') {
                out.push_str(newline);
            }
            return Ok((out, strat_name.into()));
        }
    }

    Err("Patch search block did not match any region in the target file (even after fuzzy whitespace/unicode passes)".into())
}

fn normalize_ws_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_unicode_line(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            '\u{00A0}' => ' ',
            _ => c,
        })
        .collect()
}

/// Applies V4A / unified diff `@@` hunks onto `content`.
pub fn apply_v4a_or_unified_patch(
    content: &str,
    patch_text: &str,
) -> Result<(String, usize), String> {
    let mut hunks: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    let mut cur_old: Vec<String> = Vec::new();
    let mut cur_new: Vec<String> = Vec::new();
    let mut in_hunk = false;

    for raw_line in patch_text.lines() {
        if raw_line.starts_with("*** Begin Patch")
            || raw_line.starts_with("*** End Patch")
            || raw_line.starts_with("*** Update File:")
            || raw_line.starts_with("--- ")
            || raw_line.starts_with("+++ ")
        {
            continue;
        }
        if raw_line.starts_with("@@") {
            if in_hunk && (!cur_old.is_empty() || !cur_new.is_empty()) {
                hunks.push((std::mem::take(&mut cur_old), std::mem::take(&mut cur_new)));
            }
            in_hunk = true;
            continue;
        }
        if !in_hunk && (raw_line.starts_with(' ') || raw_line.starts_with('-') || raw_line.starts_with('+')) {
            in_hunk = true;
        }
        if !in_hunk {
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix(' ') {
            cur_old.push(rest.to_string());
            cur_new.push(rest.to_string());
        } else if let Some(rest) = raw_line.strip_prefix('-') {
            cur_old.push(rest.to_string());
        } else if let Some(rest) = raw_line.strip_prefix('+') {
            cur_new.push(rest.to_string());
        } else {
            cur_old.push(raw_line.to_string());
            cur_new.push(raw_line.to_string());
        }
    }
    if in_hunk && (!cur_old.is_empty() || !cur_new.is_empty()) {
        hunks.push((cur_old, cur_new));
    }

    if hunks.is_empty() {
        return Err("No valid V4A or unified diff hunks found in 'patch'".into());
    }

    let mut current = content.to_string();
    let mut applied = 0usize;
    for (old_lines, new_lines) in hunks {
        if old_lines.is_empty() {
            if !current.is_empty() && !current.ends_with('\n') {
                current.push('\n');
            }
            current.push_str(&new_lines.join("\n"));
            current.push('\n');
            applied += 1;
            continue;
        }
        let search = old_lines.join("\n");
        let replace = new_lines.join("\n");
        let (next, _) = apply_fuzzy_search_replace(&current, &search, &replace, false)?;
        current = next;
        applied += 1;
    }
    Ok((current, applied))
}

fn tool_search_files(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let raw_dir = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let root = match validate_ctx_path(raw_dir, ctx) {
        Ok(p) => p,
        Err(e) => return HermesToolResult::err("search_files", e),
    };
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("content");
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let glob_pat = args.get("glob").and_then(|v| v.as_str());
    let case_insensitive = args
        .get("case_insensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_regex = args
        .get("is_regex")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(100)
        .max(1) as usize;

    let ignore_patterns = load_gitignore_patterns(&ctx.workspace_root);
    let mut files = Vec::new();
    collect_files_respecting_ignores(&root, &ctx.workspace_root, &ignore_patterns, &mut files);
    files.sort();

    if mode == "files" || (query.is_empty() && glob_pat.is_some()) {
        let mut matched = Vec::new();
        for file in files {
            let rel = file
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let file_name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let glob_ok = glob_pat.is_none_or(|g| glob_matches(g, &rel) || glob_matches(g, file_name));
            let query_ok = query.is_empty()
                || if case_insensitive {
                    rel.to_ascii_lowercase().contains(&query.to_ascii_lowercase())
                } else {
                    rel.contains(query)
                };
            if glob_ok && query_ok {
                matched.push(rel);
                if matched.len() >= max_results {
                    break;
                }
            }
        }
        let text = if matched.is_empty() {
            "No matching files found.".into()
        } else {
            matched.join("\n")
        };
        return HermesToolResult::ok(
            "search_files",
            text,
            Some(json!({ "mode": "files", "count": matched.len(), "files": matched })),
        );
    }

    if query.is_empty() {
        return HermesToolResult::err("search_files", "Missing 'query' or 'glob' parameter");
    }

    let matcher = MiniRegex::compile(query, is_regex, case_insensitive);
    let mut matches = Vec::new();

    for file in files {
        let rel = file
            .strip_prefix(&ctx.workspace_root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(g) = glob_pat {
            if !glob_matches(g, &rel) && !glob_matches(g, file_name) {
                continue;
            }
        }
        let Ok(bytes) = fs::read(&file) else {
            continue;
        };
        if is_probably_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (idx, line) in text.lines().enumerate() {
            if matcher.is_match(line) {
                matches.push(json!({
                    "file": rel,
                    "line": idx + 1,
                    "text": line.trim_end(),
                }));
                if matches.len() >= max_results {
                    break;
                }
            }
        }
        if matches.len() >= max_results {
            break;
        }
    }

    let formatted = if matches.is_empty() {
        "No matches found.".into()
    } else {
        matches
            .iter()
            .map(|m| format!("{}:{}: {}", m["file"].as_str().unwrap_or(""), m["line"], m["text"].as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    };

    HermesToolResult::ok(
        "search_files",
        formatted,
        Some(json!({ "mode": "content", "count": matches.len(), "matches": matches })),
    )
}

fn load_gitignore_patterns(workspace_root: &Path) -> Vec<String> {
    let gitignore = workspace_root.join(".gitignore");
    let Ok(content) = fs::read_to_string(gitignore) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(|l| l.trim_end_matches('/').to_string())
        .collect()
}

fn is_ignored_path(name: &str, rel: &str, ignore_patterns: &[String]) -> bool {
    const DEFAULT_IGNORES: &[&str] = &[
        ".git",
        "target",
        "node_modules",
        ".DS_Store",
        "__pycache__",
        ".venv",
    ];
    if DEFAULT_IGNORES.contains(&name) {
        return true;
    }
    ignore_patterns
        .iter()
        .any(|pat| glob_matches(pat, name) || glob_matches(pat, rel))
}

fn collect_files_respecting_ignores(
    dir: &Path,
    workspace_root: &Path,
    ignore_patterns: &[String],
    out: &mut Vec<PathBuf>,
) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if is_ignored_path(&name, &rel, ignore_patterns) {
            continue;
        }
        if path.is_dir() {
            collect_files_respecting_ignores(&path, workspace_root, ignore_patterns, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Pure-Rust glob matcher supporting `*`, `**`, `?`, and `{a,b}` brace groups.
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let pat = pattern.trim();
    if pat.contains('{') && pat.contains('}') {
        if let (Some(l), Some(r)) = (pat.find('{'), pat.find('}')) {
            if l < r {
                let prefix = &pat[..l];
                let suffix = &pat[r + 1..];
                return pat[l + 1..r]
                    .split(',')
                    .any(|alt| glob_matches(&format!("{prefix}{alt}{suffix}"), text));
            }
        }
    }
    glob_match_chars(&pat.chars().collect::<Vec<_>>(), &text.chars().collect::<Vec<_>>())
}

fn glob_match_chars(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star_pi, mut star_si) = (None, 0usize);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            // Treat `**/` or `*` flexibly
            while pi + 1 < p.len() && p[pi + 1] == '*' {
                pi += 1;
            }
            star_pi = Some(pi);
            star_si = si;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_si += 1;
            si = star_si;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Lightweight pure-Rust regex engine supporting literals, `.`, `*`, `+`, `?`, `^`, `$`,
/// `[class]`, `[^class]`, `\d`, `\w`, `\s`, `(a|b)` alternation, and literal fallback.
struct MiniRegex {
    branches: Vec<Vec<RegexAtom>>,
    anchored_start: bool,
    anchored_end: bool,
    case_insensitive: bool,
    literal_fallback: Option<String>,
}

#[derive(Clone)]
enum RegexMatcher {
    Any,
    Literal(char),
    Digit,
    Word,
    Space,
    Class { ranges: Vec<(char, char)>, negated: bool },
}

#[derive(Clone, Copy)]
enum Quantifier {
    One,
    ZeroOrOne,
    ZeroOrMore,
    OneOrMore,
}

#[derive(Clone)]
struct RegexAtom {
    matcher: RegexMatcher,
    quant: Quantifier,
}

impl MiniRegex {
    fn compile(pattern: &str, is_regex: bool, case_insensitive: bool) -> Self {
        if !is_regex {
            return Self {
                branches: Vec::new(),
                anchored_start: false,
                anchored_end: false,
                case_insensitive,
                literal_fallback: Some(pattern.to_string()),
            };
        }

        let mut p = pattern;
        let anchored_start = p.starts_with('^');
        if anchored_start {
            p = &p[1..];
        }
        let anchored_end = p.ends_with('$') && !p.ends_with("\\$");
        if anchored_end {
            p = &p[..p.len() - 1];
        }

        // Handle top-level `|` or single `(...)` alternation group
        let branch_strs = split_top_level_alternation(p);
        let mut branches = Vec::new();
        for b in branch_strs {
            match parse_atoms(&b) {
                Some(atoms) => branches.push(atoms),
                None => {
                    return Self {
                        branches: Vec::new(),
                        anchored_start: false,
                        anchored_end: false,
                        case_insensitive,
                        literal_fallback: Some(pattern.to_string()),
                    };
                }
            }
        }

        Self {
            branches,
            anchored_start,
            anchored_end,
            case_insensitive,
            literal_fallback: None,
        }
    }

    fn is_match(&self, line: &str) -> bool {
        if let Some(lit) = &self.literal_fallback {
            return if self.case_insensitive {
                line.to_ascii_lowercase().contains(&lit.to_ascii_lowercase())
            } else {
                line.contains(lit)
            };
        }
        let chars: Vec<char> = line.chars().collect();
        self.branches.iter().any(|atoms| {
            if self.anchored_start {
                match_atoms_at(atoms, &chars, 0, self.anchored_end, self.case_insensitive)
            } else {
                (0..=chars.len()).any(|start| {
                    match_atoms_at(atoms, &chars, start, self.anchored_end, self.case_insensitive)
                })
            }
        })
    }
}

fn split_top_level_alternation(p: &str) -> Vec<String> {
    let trimmed = if p.starts_with('(') && p.ends_with(')') && p.matches('(').count() == 1 && p.matches(')').count() == 1 {
        &p[1..p.len() - 1]
    } else {
        p
    };
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    let mut in_bracket = false;
    for ch in trimmed.chars() {
        if escaped {
            cur.push('\\');
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' => {
                in_bracket = true;
                cur.push(ch);
            }
            ']' => {
                in_bracket = false;
                cur.push(ch);
            }
            '|' if !in_bracket => {
                parts.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    parts
}

fn parse_atoms(branch: &str) -> Option<Vec<RegexAtom>> {
    let chars: Vec<char> = branch.chars().collect();
    let mut i = 0;
    let mut atoms = Vec::new();
    while i < chars.len() {
        let matcher = match chars[i] {
            '.' => {
                i += 1;
                RegexMatcher::Any
            }
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    return None;
                }
                let esc = chars[i];
                i += 1;
                match esc {
                    'd' => RegexMatcher::Digit,
                    'w' => RegexMatcher::Word,
                    's' => RegexMatcher::Space,
                    other => RegexMatcher::Literal(other),
                }
            }
            '[' => {
                i += 1;
                let negated = i < chars.len() && chars[i] == '^';
                if negated {
                    i += 1;
                }
                let mut ranges = Vec::new();
                while i < chars.len() && chars[i] != ']' {
                    let start = chars[i];
                    if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
                        let end = chars[i + 2];
                        ranges.push((start, end));
                        i += 3;
                    } else {
                        ranges.push((start, start));
                        i += 1;
                    }
                }
                if i >= chars.len() {
                    return None;
                }
                i += 1; // consume ']'
                RegexMatcher::Class { ranges, negated }
            }
            other => {
                i += 1;
                RegexMatcher::Literal(other)
            }
        };

        let quant = if i < chars.len() {
            match chars[i] {
                '*' => {
                    i += 1;
                    Quantifier::ZeroOrMore
                }
                '+' => {
                    i += 1;
                    Quantifier::OneOrMore
                }
                '?' => {
                    i += 1;
                    Quantifier::ZeroOrOne
                }
                _ => Quantifier::One,
            }
        } else {
            Quantifier::One
        };
        atoms.push(RegexAtom { matcher, quant });
    }
    Some(atoms)
}

fn char_matches(m: &RegexMatcher, ch: char, ci: bool) -> bool {
    match m {
        RegexMatcher::Any => ch != '\n',
        RegexMatcher::Literal(target) => {
            if ci {
                ch.to_ascii_lowercase() == target.to_ascii_lowercase()
            } else {
                ch == *target
            }
        }
        RegexMatcher::Digit => ch.is_ascii_digit(),
        RegexMatcher::Word => ch.is_ascii_alphanumeric() || ch == '_',
        RegexMatcher::Space => ch.is_whitespace(),
        RegexMatcher::Class { ranges, negated } => {
            let cmp = if ci { ch.to_ascii_lowercase() } else { ch };
            let in_range = ranges.iter().any(|&(a, b)| {
                let lo = if ci { a.to_ascii_lowercase() } else { a };
                let hi = if ci { b.to_ascii_lowercase() } else { b };
                cmp >= lo && cmp <= hi
            });
            if *negated { !in_range } else { in_range }
        }
    }
}

fn match_atoms_at(
    atoms: &[RegexAtom],
    chars: &[char],
    pos: usize,
    require_end: bool,
    ci: bool,
) -> bool {
    if atoms.is_empty() {
        return !require_end || pos == chars.len();
    }
    let first = &atoms[0];
    let rest = &atoms[1..];
    match first.quant {
        Quantifier::One => {
            pos < chars.len()
                && char_matches(&first.matcher, chars[pos], ci)
                && match_atoms_at(rest, chars, pos + 1, require_end, ci)
        }
        Quantifier::ZeroOrOne => {
            (pos < chars.len()
                && char_matches(&first.matcher, chars[pos], ci)
                && match_atoms_at(rest, chars, pos + 1, require_end, ci))
                || match_atoms_at(rest, chars, pos, require_end, ci)
        }
        Quantifier::ZeroOrMore => {
            let mut max_end = pos;
            while max_end < chars.len() && char_matches(&first.matcher, chars[max_end], ci) {
                max_end += 1;
            }
            (pos..=max_end)
                .rev()
                .any(|p| match_atoms_at(rest, chars, p, require_end, ci))
        }
        Quantifier::OneOrMore => {
            let mut max_end = pos;
            while max_end < chars.len() && char_matches(&first.matcher, chars[max_end], ci) {
                max_end += 1;
            }
            max_end > pos
                && ((pos + 1)..=max_end)
                    .rev()
                    .any(|p| match_atoms_at(rest, chars, p, require_end, ci))
        }
    }
}

fn tool_list_dir(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let raw_dir = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let resolved = match validate_ctx_path(raw_dir, ctx) {
        Ok(p) => p,
        Err(e) => return HermesToolResult::err("list_dir", e),
    };
    let recursive = args
        .get("recursive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(if recursive { 4 } else { 1 })
        .max(1) as usize;
    let include_hidden = args
        .get("include_hidden")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut entries = Vec::new();
    let mut visited = HashSet::new();
    list_dir_walk(&resolved, &resolved, 0, max_depth, include_hidden, &mut visited, &mut entries);

    let lines: Vec<String> = entries
        .iter()
        .map(|e| {
            if e["kind"] == "dir" {
                format!("[DIR]  {}/", e["path"].as_str().unwrap_or(""))
            } else {
                format!(
                    "[FILE] {} ({} bytes)",
                    e["path"].as_str().unwrap_or(""),
                    e["bytes"]
                )
            }
        })
        .collect();

    HermesToolResult::ok(
        "list_dir",
        if lines.is_empty() {
            "(empty directory)".into()
        } else {
            lines.join("\n")
        },
        Some(json!({ "root": resolved.display().to_string(), "entries": entries })),
    )
}

fn list_dir_walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    include_hidden: bool,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<Value>,
) {
    if depth >= max_depth || !visited.insert(dir.to_path_buf()) {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by(|a, b| {
        let ad = a.path().is_dir();
        let bd = b.path().is_dir();
        bd.cmp(&ad).then_with(|| a.file_name().cmp(&b.file_name()))
    });

    for item in items {
        let name = item.file_name().to_string_lossy().into_owned();
        if !include_hidden && name.starts_with('.') {
            continue;
        }
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let path = item.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let is_dir = path.is_dir();
        let size = item.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(json!({
            "path": rel,
            "kind": if is_dir { "dir" } else { "file" },
            "bytes": size,
        }));
        if is_dir && depth + 1 < max_depth {
            list_dir_walk(root, &path, depth + 1, max_depth, include_hidden, visited, out);
        }
    }
}

fn tool_workspace_rollback(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("rollback");
    let turn_index = args.get("turn_index").and_then(|v| v.as_u64());

    let Ok(mut mgr) = ctx.checkpoints.lock() else {
        return HermesToolResult::err("workspace_rollback", "FileCheckpointManager lock poisoned");
    };

    if action == "list" {
        let list = mgr.list_checkpoints(&ctx.session_id);
        return HermesToolResult::ok(
            "workspace_rollback",
            serde_json::to_string_pretty(&list).unwrap_or_default(),
            Some(json!({ "checkpoints": list })),
        );
    }

    match mgr.rollback_turn(&ctx.session_id, turn_index) {
        Ok(report) => HermesToolResult::ok(
            "workspace_rollback",
            format!(
                "Rolled back turn {}: restored {} file(s), deleted {} newly created file(s)",
                report.turn_index,
                report.restored_files.len(),
                report.deleted_files.len()
            ),
            Some(serde_json::to_value(&report).unwrap_or_default()),
        ),
        Err(e) => HermesToolResult::err("workspace_rollback", e),
    }
}

fn tool_todo_write(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(arr) = args.get("todos").and_then(|v| v.as_array()) else {
        return HermesToolResult::err("todo_write", "Missing 'todos' array");
    };
    let mut items = Vec::with_capacity(arr.len());
    for (idx, raw) in arr.iter().enumerate() {
        let content = raw
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            continue;
        }
        let status = raw
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(TodoStatus::from_str)
            .unwrap_or(TodoStatus::Pending);
        let id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("todo_{}", idx + 1));
        let priority = raw
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("medium")
            .to_string();
        items.push(TodoItem {
            id,
            content,
            status,
            priority,
        });
    }

    let Ok(mut store) = ctx.todos.lock() else {
        return HermesToolResult::err("todo_write", "TodoStore lock poisoned");
    };
    store.by_session.insert(ctx.session_id.clone(), items.clone());

    HermesToolResult::ok(
        "todo_write",
        format!("Updated {} todo item(s)", items.len()),
        Some(json!({ "todos": items })),
    )
}

fn tool_todo_read(ctx: &HermesToolContext) -> HermesToolResult {
    let Ok(store) = ctx.todos.lock() else {
        return HermesToolResult::err("todo_read", "TodoStore lock poisoned");
    };
    let items = store
        .by_session
        .get(&ctx.session_id)
        .cloned()
        .unwrap_or_default();
    let text = if items.is_empty() {
        "No todos recorded for this session.".into()
    } else {
        items
            .iter()
            .map(|t| format!("- [{}] ({}) {}", format!("{:?}", t.status).to_lowercase(), t.id, t.content))
            .collect::<Vec<_>>()
            .join("\n")
    };
    HermesToolResult::ok("todo_read", text, Some(json!({ "todos": items })))
}

fn tool_plan_create(args: Value, ctx: &HermesToolContext) -> HermesToolResult {
    let Some(title) = args.get("title").and_then(|v| v.as_str()) else {
        return HermesToolResult::err("plan_create", "Missing required 'title'");
    };
    let Some(steps_arr) = args.get("steps").and_then(|v| v.as_array()) else {
        return HermesToolResult::err("plan_create", "Missing required 'steps' array");
    };
    let goal = args
        .get("goal")
        .and_then(|v| v.as_str())
        .unwrap_or(title)
        .to_string();
    let sync_todos = args
        .get("sync_todos")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let steps: Vec<PlanStep> = steps_arr
        .iter()
        .enumerate()
        .filter_map(|(idx, v)| {
            v.as_str().map(|s| PlanStep {
                step_number: idx + 1,
                description: s.to_string(),
                completed: false,
            })
        })
        .collect();

    let plan = PlanSpec {
        id: format!("plan_{}", uuid::Uuid::new_v4().simple()),
        title: title.to_string(),
        goal,
        steps: steps.clone(),
        created_at: Utc::now(),
    };

    let Ok(mut store) = ctx.todos.lock() else {
        return HermesToolResult::err("plan_create", "TodoStore lock poisoned");
    };
    store
        .plans_by_session
        .entry(ctx.session_id.clone())
        .or_default()
        .push(plan.clone());

    if sync_todos {
        let todos: Vec<TodoItem> = steps
            .iter()
            .map(|s| TodoItem {
                id: format!("step_{}", s.step_number),
                content: s.description.clone(),
                status: TodoStatus::Pending,
                priority: "medium".into(),
            })
            .collect();
        store.by_session.insert(ctx.session_id.clone(), todos);
    }

    HermesToolResult::ok(
        "plan_create",
        format!("Created plan '{}' with {} step(s)", plan.title, plan.steps.len()),
        Some(serde_json::to_value(&plan).unwrap_or_default()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::toolsets::HermesToolExecutor;

    struct TempDirGuard(PathBuf);
    impl TempDirGuard {
        fn new(label: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "hermes_file_{}_{}",
                label,
                uuid::Uuid::new_v4().simple()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn test_write_patch_fuzzy_and_rollback() {
        let tmp = TempDirGuard::new("rollback");
        let ctx = HermesToolContext::new(&tmp.0).with_session("s1", 1);

        // Create initial file in turn 1
        let existing = tmp.0.join("src/lib.rs");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "fn hello() {\n    println!(\"old\");\n}\n").unwrap();

        // Patch using messy whitespace (fuzzy match) + create new file in turn 2
        let ctx_t2 = ctx.clone().with_session("s1", 2);
        let patch_res = HermesToolExecutor::execute(
            "patch",
            json!({
                "path": "src/lib.rs",
                "old_text": "fn hello() {\nprintln!(\"old\");\n}",
                "new_text": "fn hello() {\n    println!(\"new\");\n}"
            }),
            &ctx_t2,
        )
        .await;
        assert!(patch_res.success, "{}", patch_res.output);
        assert!(fs::read_to_string(&existing).unwrap().contains("\"new\""));

        let write_new = HermesToolExecutor::execute(
            "write_file",
            json!({ "path": "src/extra.rs", "content": "pub fn extra() {}\n" }),
            &ctx_t2,
        )
        .await;
        assert!(write_new.success);
        assert!(tmp.0.join("src/extra.rs").exists());

        // Rollback turn 2 -> restores lib.rs AND deletes extra.rs!
        let rb = HermesToolExecutor::execute("workspace_rollback", json!({}), &ctx_t2).await;
        assert!(rb.success, "{}", rb.output);
        assert!(fs::read_to_string(&existing).unwrap().contains("\"old\""));
        assert!(!tmp.0.join("src/extra.rs").exists());
    }

    #[tokio::test]
    async fn test_search_files_regex_and_glob() {
        let tmp = TempDirGuard::new("search");
        fs::write(tmp.0.join("a.rs"), "pub fn alpha_123() {}\n").unwrap();
        fs::write(tmp.0.join("b.txt"), "alpha_999\n").unwrap();
        let ctx = HermesToolContext::new(&tmp.0);

        let res = HermesToolExecutor::execute(
            "search_files",
            json!({ "query": "alpha_\\d+", "glob": "*.rs" }),
            &ctx,
        )
        .await;
        assert!(res.success);
        assert!(res.output.contains("a.rs:1:"));
        assert!(!res.output.contains("b.txt"));
    }
}
