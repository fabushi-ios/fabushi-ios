use crate::hermes::memory::{MemorySnapshot, SoulProfile};
use crate::hermes::skills::SkillsManager;
use gpui_telegram_core::error::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

// =============================================================================
// Workspace Context Discovery (.hermes.md, AGENTS.md, CLAUDE.md, SOUL.md)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFileKind {
    HermesMd,
    AgentsMd,
    ClaudeMd,
    SoulMd,
}

impl ContextFileKind {
    pub fn file_name(&self) -> &'static str {
        match self {
            Self::HermesMd => ".hermes.md",
            Self::AgentsMd => "AGENTS.md",
            Self::ClaudeMd => "CLAUDE.md",
            Self::SoulMd => "SOUL.md",
        }
    }

    pub fn all() -> [Self; 4] {
        [
            Self::HermesMd,
            Self::AgentsMd,
            Self::ClaudeMd,
            Self::SoulMd,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredContextFile {
    pub kind: ContextFileKind,
    pub path: PathBuf,
    pub content: String,
    pub distance_from_cwd: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredWorkspaceContext {
    pub start_dir: PathBuf,
    pub files: Vec<DiscoveredContextFile>,
}

impl DiscoveredWorkspaceContext {
    /// Returns the closest `SOUL.md` content if one was discovered.
    pub fn soul_markdown(&self) -> Option<&str> {
        self.files
            .iter()
            .filter(|f| f.kind == ContextFileKind::SoulMd)
            .min_by_key(|f| f.distance_from_cwd)
            .map(|f| f.content.as_str())
    }

    /// Renders discovered workspace instruction files (`.hermes.md`, `AGENTS.md`, `CLAUDE.md`)
    /// ordered from outermost ancestor to innermost directory.
    pub fn render_context_block(&self) -> String {
        let relevant: Vec<&DiscoveredContextFile> = self
            .files
            .iter()
            .filter(|f| f.kind != ContextFileKind::SoulMd && !f.content.trim().is_empty())
            .collect();
        if relevant.is_empty() {
            return String::new();
        }
        let mut out = String::from("## Workspace Instructions\n");
        for file in relevant {
            out.push_str(&format!(
                "\n### {} ({})\n{}\n",
                file.kind.file_name(),
                file.path.display(),
                file.content.trim()
            ));
        }
        out
    }
}

pub struct WorkspaceContextDiscovery;

impl WorkspaceContextDiscovery {
    /// Walks upward from `start_dir` (stopping at `stop_at` if provided, or root, max 64 levels)
    /// to discover `.hermes.md`, `AGENTS.md`, `CLAUDE.md`, and `SOUL.md`.
    pub fn discover(
        start_dir: impl AsRef<Path>,
        stop_at: Option<&Path>,
    ) -> Result<DiscoveredWorkspaceContext> {
        let start = start_dir.as_ref();
        let canonical_start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
        let canonical_stop = stop_at.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));

        let mut layers: Vec<Vec<DiscoveredContextFile>> = Vec::new();
        let mut seen_paths = BTreeSet::new();
        let mut cursor = Some(canonical_start.as_path());
        let mut distance = 0usize;

        while let Some(dir) = cursor {
            if distance > 64 {
                break;
            }
            let mut layer = Vec::new();
            for kind in ContextFileKind::all() {
                let candidate = dir.join(kind.file_name());
                if candidate.is_file() {
                    let canon = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
                    if seen_paths.insert(canon.clone()) {
                        if let Ok(content) = std::fs::read_to_string(&canon) {
                            layer.push(DiscoveredContextFile {
                                kind,
                                path: canon,
                                content,
                                distance_from_cwd: distance,
                            });
                        }
                    }
                }
            }
            if !layer.is_empty() {
                layers.push(layer);
            }

            if let Some(ref stop) = canonical_stop {
                if dir == stop.as_path() {
                    break;
                }
            }
            cursor = dir.parent();
            distance += 1;
        }

        // Reverse so outermost ancestor appears first and innermost overrides/extends it.
        layers.reverse();
        let files = layers.into_iter().flatten().collect();

        Ok(DiscoveredWorkspaceContext {
            start_dir: canonical_start,
            files,
        })
    }
}

// =============================================================================
// `@` Context References Resolver (@file, @dir, @diff, @staged, @git:n, @url)
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRefKind {
    File,
    Dir,
    Diff,
    Staged,
    GitLog,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedContextRef {
    pub raw_token: String,
    pub kind: ContextRefKind,
    pub target: String,
    pub resolved_content: String,
    pub truncated: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedMessageContext {
    pub original_text: String,
    pub expanded_prompt: String,
    pub references: Vec<ResolvedContextRef>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextRefResolver {
    workspace_root: PathBuf,
    max_file_bytes: usize,
    max_output_chars: usize,
}

impl ContextRefResolver {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        let root = workspace_root.as_ref();
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        Self {
            workspace_root: canonical,
            max_file_bytes: 128 * 1024,
            max_output_chars: 12_000,
        }
    }

    pub fn with_limits(mut self, max_file_bytes: usize, max_output_chars: usize) -> Self {
        self.max_file_bytes = max_file_bytes;
        self.max_output_chars = max_output_chars;
        self
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Parses and synchronously resolves all `@file:`, `@dir:`, `@diff`, `@staged`, `@git:<n>`,
    /// and `@url:` tokens in `text` strictly inside `workspace_root`.
    pub fn resolve(&self, text: &str) -> ResolvedMessageContext {
        let tokens = extract_at_tokens(text);
        let mut references = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = BTreeSet::new();

        for raw in tokens {
            if !seen.insert(raw.clone()) {
                continue;
            }
            if let Some(resolved) = self.resolve_single_token(&raw) {
                if let Some(err) = &resolved.error {
                    warnings.push(format!("{raw}: {err}"));
                }
                references.push(resolved);
            }
        }

        let expanded_prompt = if references.is_empty() {
            text.to_string()
        } else {
            let mut blocks = String::new();
            for r in &references {
                if let Some(err) = &r.error {
                    blocks.push_str(&format!(
                        "\n<context_ref token=\"{}\" status=\"error\">{}</context_ref>",
                        r.raw_token, err
                    ));
                } else {
                    blocks.push_str(&format!(
                        "\n<context_ref token=\"{}\" target=\"{}\">\n{}\n</context_ref>",
                        r.raw_token, r.target, r.resolved_content
                    ));
                }
            }
            format!("{}\n\n[Resolved Context References]{blocks}", text.trim_end())
        };

        ResolvedMessageContext {
            original_text: text.to_string(),
            expanded_prompt,
            references,
            warnings,
        }
    }

    fn resolve_single_token(&self, raw: &str) -> Option<ResolvedContextRef> {
        if let Some(path_spec) = raw.strip_prefix("@file:") {
            return Some(self.resolve_file_ref(raw, path_spec));
        }
        if let Some(dir_spec) = raw.strip_prefix("@dir:") {
            return Some(self.resolve_dir_ref(raw, dir_spec));
        }
        if raw == "@diff" {
            return Some(self.resolve_git_command(raw, ContextRefKind::Diff, &["diff"]));
        }
        if raw == "@staged" {
            return Some(self.resolve_git_command(
                raw,
                ContextRefKind::Staged,
                &["diff", "--cached"],
            ));
        }
        if let Some(count_str) = raw.strip_prefix("@git:") {
            let n = count_str.trim().parse::<usize>().unwrap_or(5).clamp(1, 50);
            let n_arg = n.to_string();
            return Some(self.resolve_git_command(
                raw,
                ContextRefKind::GitLog,
                &["log", "-n", &n_arg, "--oneline", "--stat"],
            ));
        }
        if let Some(url) = raw.strip_prefix("@url:") {
            return Some(self.resolve_url_ref(raw, url));
        }
        None
    }

    fn resolve_workspace_path(&self, rel_or_abs: &str) -> std::result::Result<PathBuf, String> {
        let trimmed = rel_or_abs.trim();
        if trimmed.is_empty() {
            return Err("Empty path reference".into());
        }
        let candidate = Path::new(trimmed);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.workspace_root.join(candidate)
        };

        let normalized = lexical_normalize(&joined);
        if !normalized.starts_with(&self.workspace_root) {
            return Err(format!(
                "Path '{}' escapes workspace root '{}'",
                trimmed,
                self.workspace_root.display()
            ));
        }

        if normalized.exists() {
            let canon = std::fs::canonicalize(&normalized)
                .map_err(|e| format!("Failed to canonicalize '{}': {e}", normalized.display()))?;
            if !canon.starts_with(&self.workspace_root) {
                return Err(format!(
                    "Symlink target '{}' escapes workspace root",
                    canon.display()
                ));
            }
            return Ok(canon);
        }

        Err(format!("Path '{trimmed}' does not exist in workspace"))
    }

    fn resolve_file_ref(&self, raw: &str, spec: &str) -> ResolvedContextRef {
        let (path_part, line_range) = split_optional_line_range(spec);
        match self.resolve_workspace_path(path_part) {
            Ok(path) => {
                if !path.is_file() {
                    return ResolvedContextRef {
                        raw_token: raw.to_string(),
                        kind: ContextRefKind::File,
                        target: path_part.to_string(),
                        resolved_content: String::new(),
                        truncated: false,
                        error: Some(format!("'{}' is not a regular file", path_part)),
                    };
                }
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let slice = if bytes.len() > self.max_file_bytes {
                            &bytes[..self.max_file_bytes]
                        } else {
                            &bytes[..]
                        };
                        if slice.contains(&0) {
                            return ResolvedContextRef {
                                raw_token: raw.to_string(),
                                kind: ContextRefKind::File,
                                target: path_part.to_string(),
                                resolved_content: format!("[binary file: {} bytes]", bytes.len()),
                                truncated: false,
                                error: None,
                            };
                        }
                        let full_text = String::from_utf8_lossy(slice);
                        let lines: Vec<&str> = full_text.lines().collect();
                        let (start_line, end_line) = match line_range {
                            Some((s, e)) => (
                                s.saturating_sub(1).min(lines.len()),
                                e.min(lines.len()),
                            ),
                            None => (0, lines.len()),
                        };
                        let numbered = lines[start_line..end_line]
                            .iter()
                            .enumerate()
                            .map(|(i, l)| format!("{:>4}: {l}", start_line + i + 1))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let (content, truncated) = cap_chars(&numbered, self.max_output_chars);
                        ResolvedContextRef {
                            raw_token: raw.to_string(),
                            kind: ContextRefKind::File,
                            target: path_part.to_string(),
                            resolved_content: content,
                            truncated: truncated || bytes.len() > self.max_file_bytes,
                            error: None,
                        }
                    }
                    Err(e) => ResolvedContextRef {
                        raw_token: raw.to_string(),
                        kind: ContextRefKind::File,
                        target: path_part.to_string(),
                        resolved_content: String::new(),
                        truncated: false,
                        error: Some(format!("Failed to read file: {e}")),
                    },
                }
            }
            Err(err) => ResolvedContextRef {
                raw_token: raw.to_string(),
                kind: ContextRefKind::File,
                target: path_part.to_string(),
                resolved_content: String::new(),
                truncated: false,
                error: Some(err),
            },
        }
    }

    fn resolve_dir_ref(&self, raw: &str, spec: &str) -> ResolvedContextRef {
        let target = if spec.trim().is_empty() { "." } else { spec.trim() };
        match self.resolve_workspace_path(target) {
            Ok(path) => {
                if !path.is_dir() {
                    return ResolvedContextRef {
                        raw_token: raw.to_string(),
                        kind: ContextRefKind::Dir,
                        target: target.to_string(),
                        resolved_content: String::new(),
                        truncated: false,
                        error: Some(format!("'{}' is not a directory", target)),
                    };
                }
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut lines = Vec::new();
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let meta = entry.metadata().ok();
                            let suffix = if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
                                "/"
                            } else {
                                ""
                            };
                            let size = meta.map(|m| m.len()).unwrap_or(0);
                            lines.push(format!("{name}{suffix} ({size} bytes)"));
                        }
                        lines.sort();
                        let joined = lines.join("\n");
                        let (content, truncated) = cap_chars(&joined, self.max_output_chars);
                        ResolvedContextRef {
                            raw_token: raw.to_string(),
                            kind: ContextRefKind::Dir,
                            target: target.to_string(),
                            resolved_content: content,
                            truncated,
                            error: None,
                        }
                    }
                    Err(e) => ResolvedContextRef {
                        raw_token: raw.to_string(),
                        kind: ContextRefKind::Dir,
                        target: target.to_string(),
                        resolved_content: String::new(),
                        truncated: false,
                        error: Some(format!("Failed to list directory: {e}")),
                    },
                }
            }
            Err(err) => ResolvedContextRef {
                raw_token: raw.to_string(),
                kind: ContextRefKind::Dir,
                target: target.to_string(),
                resolved_content: String::new(),
                truncated: false,
                error: Some(err),
            },
        }
    }

    fn resolve_git_command(
        &self,
        raw: &str,
        kind: ContextRefKind,
        args: &[&str],
    ) -> ResolvedContextRef {
        match Command::new("git")
            .args(args)
            .current_dir(&self.workspace_root)
            .output()
        {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let text = if stdout.trim().is_empty() {
                    "(no changes / empty git output)".to_string()
                } else {
                    stdout
                };
                let (content, truncated) = cap_chars(&text, self.max_output_chars);
                ResolvedContextRef {
                    raw_token: raw.to_string(),
                    kind,
                    target: args.join(" "),
                    resolved_content: content,
                    truncated,
                    error: None,
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                ResolvedContextRef {
                    raw_token: raw.to_string(),
                    kind,
                    target: args.join(" "),
                    resolved_content: String::new(),
                    truncated: false,
                    error: Some(if stderr.is_empty() {
                        "git command failed or workspace is not a git repository".into()
                    } else {
                        stderr
                    }),
                }
            }
            Err(e) => ResolvedContextRef {
                raw_token: raw.to_string(),
                kind,
                target: args.join(" "),
                resolved_content: String::new(),
                truncated: false,
                error: Some(format!("git unavailable: {e}")),
            },
        }
    }

    fn resolve_url_ref(&self, raw: &str, url: &str) -> ResolvedContextRef {
        let trimmed = url.trim();
        let valid_scheme =
            trimmed.starts_with("https://") || trimmed.starts_with("http://");
        if !valid_scheme || trimmed.len() < 10 {
            return ResolvedContextRef {
                raw_token: raw.to_string(),
                kind: ContextRefKind::Url,
                target: trimmed.to_string(),
                resolved_content: String::new(),
                truncated: false,
                error: Some("URL reference must start with http:// or https://".into()),
            };
        }

        ResolvedContextRef {
            raw_token: raw.to_string(),
            kind: ContextRefKind::Url,
            target: trimmed.to_string(),
            resolved_content: format!(
                "URL reference queued for web_extract/fetch: {trimmed}"
            ),
            truncated: false,
            error: None,
        }
    }
}

fn extract_at_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw_piece in text.split_whitespace() {
        let cleaned = raw_piece
            .trim_start_matches(['(', '[', '{', '"', '\''])
            .trim_end_matches([',', '.', ';', ')', ']', '}', '"', '\'']);
        if cleaned.starts_with("@file:")
            || cleaned.starts_with("@dir:")
            || cleaned == "@diff"
            || cleaned == "@staged"
            || cleaned.starts_with("@git:")
            || cleaned.starts_with("@url:")
        {
            out.push(cleaned.to_string());
        }
    }
    out
}

fn split_optional_line_range(spec: &str) -> (&str, Option<(usize, usize)>) {
    if let Some((path, range_part)) = spec.rsplit_once(':') {
        if let Some((start_s, end_s)) = range_part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start_s.parse::<usize>(), end_s.parse::<usize>()) {
                if s >= 1 && e >= s {
                    return (path, Some((s, e)));
                }
            }
        } else if let Ok(line) = range_part.parse::<usize>() {
            if line >= 1 {
                return (path, Some((line, line)));
            }
        }
    }
    (spec, None)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::RootDir)) {
                    out.pop();
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

fn cap_chars(text: &str, max_chars: usize) -> (String, bool) {
    let total = text.chars().count();
    if total <= max_chars {
        (text.to_string(), false)
    } else {
        let mut s: String = text.chars().take(max_chars).collect();
        s.push_str("\n...[truncated]");
        (s, true)
    }
}

// =============================================================================
// Context Compressor & Token Estimation
// =============================================================================

/// Estimates token usage in pure Rust, accounting for both ASCII (~4 chars/token)
/// and CJK characters (~1.5 tokens/char).
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut ascii_chars = 0usize;
    let mut cjk_chars = 0usize;
    let mut other_chars = 0usize;

    for ch in text.chars() {
        if ch.is_ascii() {
            ascii_chars += 1;
        } else if is_cjk(ch) {
            cjk_chars += 1;
        } else {
            other_chars += 1;
        }
    }

    let ascii_toks = (ascii_chars + 3) / 4;
    let cjk_toks = (cjk_chars * 3 + 1) / 2;
    let other_toks = (other_chars + 1) / 2;
    (ascii_toks + cjk_toks + other_toks).max(1)
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub pinned: bool,
}

impl ContextMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
            pinned: false,
        }
    }

    pub fn estimated_tokens(&self) -> usize {
        estimate_tokens(&self.content) + 4
    }
}

pub fn estimate_messages_tokens(messages: &[ContextMessage]) -> usize {
    messages.iter().map(ContextMessage::estimated_tokens).sum()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionConfig {
    pub context_window_tokens: usize,
    pub trigger_ratio: f32,
    pub preserve_head_turns: usize,
    pub preserve_tail_turns: usize,
    pub max_summary_chars: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            context_window_tokens: 128_000,
            trigger_ratio: 0.78,
            preserve_head_turns: 1,
            preserve_tail_turns: 4,
            max_summary_chars: 4000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionOutcome {
    pub compressed: bool,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub summarized_message_count: usize,
    pub summary_markdown: String,
    pub messages: Vec<ContextMessage>,
}

/// Compresses middle conversation turns into a structured summary while preserving
/// system prompts, head context, and recent tail turns (never splitting tool call pairs).
#[derive(Debug, Clone, Default)]
pub struct ContextCompressor {
    pub config: CompressionConfig,
}

impl ContextCompressor {
    pub fn new(config: CompressionConfig) -> Self {
        Self { config }
    }

    pub fn threshold_tokens(&self) -> usize {
        ((self.config.context_window_tokens as f32) * self.config.trigger_ratio).round() as usize
    }

    pub fn should_compress(&self, messages: &[ContextMessage]) -> bool {
        let total = estimate_messages_tokens(messages);
        total >= self.threshold_tokens() && messages.len() > (self.config.preserve_head_turns + self.config.preserve_tail_turns + 2)
    }

    pub fn compress(&self, messages: &[ContextMessage], force: bool) -> CompressionOutcome {
        let original_tokens = estimate_messages_tokens(messages);
        if !force && !self.should_compress(messages) {
            return CompressionOutcome {
                compressed: false,
                original_tokens,
                compressed_tokens: original_tokens,
                summarized_message_count: 0,
                summary_markdown: String::new(),
                messages: messages.to_vec(),
            };
        }

        // Keep leading system messages + preserve_head_turns non-system messages.
        let mut head_end = 0usize;
        while head_end < messages.len() && messages[head_end].role == "system" {
            head_end += 1;
        }
        let mut head_non_sys = 0usize;
        while head_end < messages.len() && head_non_sys < self.config.preserve_head_turns {
            head_end += 1;
            head_non_sys += 1;
        }

        let mut tail_start = messages.len().saturating_sub(self.config.preserve_tail_turns);
        if tail_start <= head_end {
            return CompressionOutcome {
                compressed: false,
                original_tokens,
                compressed_tokens: original_tokens,
                summarized_message_count: 0,
                summary_markdown: String::new(),
                messages: messages.to_vec(),
            };
        }

        // Never start tail in the middle of tool responses: back up to include calling assistant turn.
        while tail_start > head_end && messages[tail_start].role == "tool" {
            tail_start -= 1;
        }
        if tail_start <= head_end {
            return CompressionOutcome {
                compressed: false,
                original_tokens,
                compressed_tokens: original_tokens,
                summarized_message_count: 0,
                summary_markdown: String::new(),
                messages: messages.to_vec(),
            };
        }

        let head = &messages[..head_end];
        let middle = &messages[head_end..tail_start];
        let tail = &messages[tail_start..];

        let mut pinned_middle = Vec::new();
        let mut to_summarize = Vec::new();
        for msg in middle {
            if msg.pinned {
                pinned_middle.push(msg.clone());
            } else {
                to_summarize.push(msg);
            }
        }

        if to_summarize.is_empty() {
            return CompressionOutcome {
                compressed: false,
                original_tokens,
                compressed_tokens: original_tokens,
                summarized_message_count: 0,
                summary_markdown: String::new(),
                messages: messages.to_vec(),
            };
        }

        let summary_markdown = build_structured_middle_summary(&to_summarize, self.config.max_summary_chars);

        let mut compacted = Vec::with_capacity(head.len() + pinned_middle.len() + 1 + tail.len());
        compacted.extend_from_slice(head);
        compacted.push(ContextMessage {
            role: "system".into(),
            content: summary_markdown.clone(),
            tool_call_id: None,
            tool_name: None,
            pinned: true,
        });
        compacted.extend(pinned_middle);
        compacted.extend_from_slice(tail);

        let compressed_tokens = estimate_messages_tokens(&compacted);
        CompressionOutcome {
            compressed: true,
            original_tokens,
            compressed_tokens,
            summarized_message_count: to_summarize.len(),
            summary_markdown,
            messages: compacted,
        }
    }
}

fn build_structured_middle_summary(messages: &[&ContextMessage], max_chars: usize) -> String {
    let mut goals = Vec::new();
    let mut progress = Vec::new();
    let mut decisions = Vec::new();
    let mut files_touched = BTreeSet::new();
    let mut next_steps = Vec::new();

    for msg in messages {
        let preview = first_line_preview(&msg.content, 160);
        collect_file_paths(&msg.content, &mut files_touched);

        match msg.role.as_str() {
            "user" => {
                if !preview.is_empty() && goals.len() < 4 {
                    goals.push(preview);
                }
            }
            "assistant" => {
                let lower = msg.content.to_lowercase();
                if (lower.contains("decided")
                    || lower.contains("chose")
                    || lower.contains("approach")
                    || lower.contains("architecture")
                    || lower.contains("决定")
                    || lower.contains("方案"))
                    && decisions.len() < 5
                    && !preview.is_empty()
                {
                    decisions.push(preview.clone());
                } else if (lower.contains("next") || lower.contains("todo") || lower.contains("remaining") || lower.contains("下一步"))
                    && next_steps.len() < 4
                    && !preview.is_empty()
                {
                    next_steps.push(preview.clone());
                } else if !preview.is_empty() && progress.len() < 6 {
                    progress.push(preview);
                }
            }
            "tool" => {
                if let Some(tool) = &msg.tool_name {
                    if progress.len() < 8 {
                        progress.push(format!("Executed `{tool}`: {preview}"));
                    }
                }
            }
            _ => {}
        }
    }

    if goals.is_empty() {
        goals.push("Continue fulfilling the active user request in the current workspace.".into());
    }
    if progress.is_empty() {
        progress.push(format!("Processed {} earlier conversation turns.", messages.len()));
    }
    if decisions.is_empty() {
        decisions.push("Preserved tool-grounded execution and workspace safety checks.".into());
    }
    if next_steps.is_empty() {
        next_steps.push("Proceed using the recent tail turns as immediate context.".into());
    }

    let mut out = format!(
        "[Context Compression Summary — {} turns compacted]\n\n## Goal\n{}\n\n## Progress\n{}\n\n## Key Decisions\n{}\n",
        messages.len(),
        goals.iter().map(|g| format!("- {g}")).collect::<Vec<_>>().join("\n"),
        progress.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"),
        decisions.iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n"),
    );

    out.push_str("\n## Files Touched\n");
    if files_touched.is_empty() {
        out.push_str("- (none recorded)\n");
    } else {
        for f in files_touched.iter().take(20) {
            out.push_str(&format!("- `{f}`\n"));
        }
    }

    out.push_str("\n## Next Steps\n");
    for s in &next_steps {
        out.push_str(&format!("- {s}\n"));
    }

    cap_chars(&out, max_chars).0
}

fn first_line_preview(text: &str, max_chars: usize) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.chars().take(max_chars).collect();
        }
    }
    String::new()
}

fn collect_file_paths(text: &str, out: &mut BTreeSet<String>) {
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| {
            matches!(c, '`' | '\'' | '"' | ',' | ';' | '(' | ')' | '[' | ']' | '<' | '>')
        });
        if cleaned.contains('/') && !cleaned.starts_with("http://") && !cleaned.starts_with("https://") {
            let candidate = cleaned.strip_prefix("@file:").unwrap_or(cleaned);
            if candidate.len() >= 3 && candidate.len() <= 160 && candidate.contains('.') {
                out.insert(candidate.to_string());
            }
        }
    }
}

// =============================================================================
// 3-Tier Prompt Assembly (Tier 1: Stable, Tier 2: Context, Tier 3: Volatile)
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheControlMarker {
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControlMarker {
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptTier {
    Tier1Stable,
    Tier2Context,
    Tier3Volatile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptBlock {
    pub tier: PromptTier,
    pub content: String,
    pub cache_control: Option<CacheControlMarker>,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolatileRuntimeContext {
    pub session_id: String,
    pub chat_id: Option<i64>,
    pub bot_id: Option<String>,
    pub source: String,
    pub model: String,
    pub timestamp_utc: Option<String>,
    pub voice_mode: bool,
    pub verbose_mode: bool,
    pub reasoning_mode: bool,
    pub yolo_mode: bool,
    pub ephemeral_prefetch: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PromptBuildInput<'a> {
    pub soul: Option<&'a SoulProfile>,
    pub active_toolsets: &'a [String],
    pub skills: Option<&'a SkillsManager>,
    pub memory_snapshot: Option<&'a MemorySnapshot>,
    pub workspace_context: Option<&'a DiscoveredWorkspaceContext>,
    pub honcho_summary: Option<&'a str>,
    pub volatile: VolatileRuntimeContext,
    pub enable_prompt_caching: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssembledPrompt {
    pub tier1_stable: PromptBlock,
    pub tier2_context: PromptBlock,
    pub tier3_volatile: PromptBlock,
    pub combined_system_prompt: String,
    pub tier1_hash: String,
    pub estimated_tokens: usize,
}

impl AssembledPrompt {
    /// Returns the blocks suitable for Anthropic Messages `system` array with `cache_control`.
    pub fn to_anthropic_system_blocks(&self) -> Vec<serde_json::Value> {
        [&self.tier1_stable, &self.tier2_context, &self.tier3_volatile]
            .into_iter()
            .filter(|b| !b.content.trim().is_empty())
            .map(|b| {
                let mut obj = serde_json::json!({
                    "type": "text",
                    "text": b.content,
                });
                if let Some(cc) = &b.cache_control {
                    obj["cache_control"] = serde_json::json!({ "type": cc.cache_type });
                }
                obj
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PromptBuilder;

impl PromptBuilder {
    pub fn new() -> Self {
        Self
    }

    /// Build the 3-tier system prompt.
    ///
    /// Prompt Caching Invariant: `tier1_stable` and `tier2_context` MUST remain strictly
    /// byte-stable across iterations within a session — never embed per-second wall-clock
    /// timestamps or transient loop counters here. All volatile runtime fields belong in
    /// `tier3_volatile`.
    pub fn build(&self, input: PromptBuildInput<'_>) -> AssembledPrompt {
        let default_soul = SoulProfile::default();
        let soul = input.soul.unwrap_or(&default_soul);

        // Tier 1: Stable Identity + Operational Doctrine + Tooling + Progressive Skill Index
        let mut t1_parts = Vec::new();
        t1_parts.push(format!("# System Identity\n{}", soul.render_prompt_block()));

        t1_parts.push(
            "## Autonomous Execution & Self-Learning Doctrine\n\
             - Multi-step execution loop: follow `inspect -> execute -> verify -> report`. Chain tool calls iteratively across steps until the actual state is verified rather than guessing or stopping mid-task.\n\
             - Proactive tool usage: actively inspect and verify via `computer_*` (`computer_list`, `computer_status`, `computer_exec`, `computer_file`, `computer_screen`, `computer_use`), `terminal`, file tools (`read_file`, `write_file`, `patch`, `search_files`), web/browser tools, `memory`, and `skill_list` / `skill_view` / `skill_manage` instead of asking the user to run manual checks.\n\
             - Frictionless computer control: in authorized/`Yolo` sessions or bound online computers, invoke `computer_exec`, `computer_file`, `computer_screen`, or `computer_use` directly—control leases are acquired automatically when omitted.\n\
             - Resilient error recovery: when any tool returns a non-fatal error envelope (`{\"ok\": false, ...}`), inspect `error` and `hint`, adjust parameters or switch strategy, and continue.\n\
             - Self-learning loop: after complex multi-step workflows (5+ tool calls) or when discovering durable facts/preferences/procedures, persist facts via `memory` and create or refine reusable procedural skills via `skill_list` / `skill_view` / `skill_manage`."
                .to_string(),
        );

        if !input.active_toolsets.is_empty() {
            t1_parts.push(format!(
                "## Active Toolsets\n{}\nUse tools intentionally, cite verified paths/outputs, and ask for approval on dangerous operations.",
                input.active_toolsets.join(", ")
            ));
        }

        if let Some(skills_mgr) = input.skills {
            let idx = skills_mgr.render_progressive_index();
            if !idx.is_empty() {
                t1_parts.push(format!(
                    "## Progressive Skills Index\nLoad full instructions via `skill_view` (or `skill_manage` with `action=\"view\"`) only when a skill matches the task:\n{idx}"
                ));
            }
        }

        let t1_content = t1_parts.join("\n\n");
        let cache_marker = if input.enable_prompt_caching {
            Some(CacheControlMarker::ephemeral())
        } else {
            None
        };
        let tier1_stable = PromptBlock {
            tier: PromptTier::Tier1Stable,
            estimated_tokens: estimate_tokens(&t1_content),
            content: t1_content,
            cache_control: cache_marker.clone(),
        };

        // Tier 2: Context (Frozen MEMORY.md + USER.md Snapshot, Workspace Docs, Honcho Summary)
        let mut t2_parts = Vec::new();
        if let Some(snap) = input.memory_snapshot {
            let mem_block = snap.render_prompt_block();
            if !mem_block.is_empty() {
                t2_parts.push(format!("## Frozen Memory Snapshot\n{mem_block}"));
            }
        }
        if let Some(ws) = input.workspace_context {
            let ws_block = ws.render_context_block();
            if !ws_block.is_empty() {
                t2_parts.push(ws_block);
            }
        }
        if let Some(honcho) = input.honcho_summary {
            if !honcho.trim().is_empty() {
                t2_parts.push(format!("## User Dialectic Profile (Honcho)\n{}", honcho.trim()));
            }
        }

        let t2_content = t2_parts.join("\n\n");
        let tier2_context = PromptBlock {
            tier: PromptTier::Tier2Context,
            estimated_tokens: estimate_tokens(&t2_content),
            content: t2_content,
            cache_control: cache_marker,
        };

        // Tier 3: Volatile (Timestamp, Session/Chat Context, Modes, Ephemeral Prefetch)
        let ts = input
            .volatile
            .timestamp_utc
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let mut t3_lines = vec![
            format!("Timestamp (UTC): {ts}"),
            format!("Session: {}", input.volatile.session_id),
            format!("Source: {} | Model: {}", input.volatile.source, input.volatile.model),
        ];
        if let Some(chat_id) = input.volatile.chat_id {
            t3_lines.push(format!("Chat ID: {chat_id}"));
        }
        if let Some(bot_id) = &input.volatile.bot_id {
            t3_lines.push(format!("Bot ID: {bot_id}"));
        }

        let mut active_modes = Vec::new();
        if input.volatile.voice_mode {
            active_modes.push("voice (keep responses natural, spoken, and concise)");
        }
        if input.volatile.verbose_mode {
            active_modes.push("verbose (include step-by-step tool diagnostics)");
        }
        if input.volatile.reasoning_mode {
            active_modes.push("reasoning (surface structured thinking steps)");
        }
        if input.volatile.yolo_mode {
            active_modes.push("yolo (auto-approve non-blocked commands)");
        }
        if !active_modes.is_empty() {
            t3_lines.push(format!("Modes: {}", active_modes.join("; ")));
        }

        if !input.volatile.ephemeral_prefetch.is_empty() {
            t3_lines.push("## Ephemeral Prefetch Hints".into());
            for hint in &input.volatile.ephemeral_prefetch {
                if !hint.trim().is_empty() {
                    t3_lines.push(format!("- {}", hint.trim()));
                }
            }
        }

        let t3_content = format!("## Runtime Context (Volatile)\n{}", t3_lines.join("\n"));
        let tier3_volatile = PromptBlock {
            tier: PromptTier::Tier3Volatile,
            estimated_tokens: estimate_tokens(&t3_content),
            content: t3_content,
            cache_control: None,
        };

        // Hash stable prefix (Tier 1 + Tier 2) so prompt cache stability can be tracked.
        let mut hasher = Sha256::new();
        hasher.update(tier1_stable.content.as_bytes());
        hasher.update(b"\n---\n");
        hasher.update(tier2_context.content.as_bytes());
        let tier1_hash = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();

        let mut non_empty_tiers = Vec::new();
        if !tier1_stable.content.is_empty() {
            non_empty_tiers.push(tier1_stable.content.as_str());
        }
        if !tier2_context.content.is_empty() {
            non_empty_tiers.push(tier2_context.content.as_str());
        }
        if !tier3_volatile.content.is_empty() {
            non_empty_tiers.push(tier3_volatile.content.as_str());
        }
        let combined_system_prompt = non_empty_tiers.join("\n\n");
        let estimated_tokens = estimate_tokens(&combined_system_prompt);

        AssembledPrompt {
            tier1_stable,
            tier2_context,
            tier3_volatile,
            combined_system_prompt,
            tier1_hash,
            estimated_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_builder_tier1_byte_stable_across_timestamps() {
        let builder = PromptBuilder::new();
        let toolsets = vec![
            "core".to_string(),
            "file".to_string(),
            "computer_use".to_string(),
            "browser".to_string(),
        ];

        let p1 = builder.build(PromptBuildInput {
            active_toolsets: &toolsets,
            volatile: VolatileRuntimeContext {
                session_id: "sess_cache_1".into(),
                source: "telegram".into(),
                model: "gpt-4o".into(),
                timestamp_utc: Some("2026-11-15T10:00:00Z".into()),
                ..Default::default()
            },
            enable_prompt_caching: true,
            ..Default::default()
        });

        let p2 = builder.build(PromptBuildInput {
            active_toolsets: &toolsets,
            volatile: VolatileRuntimeContext {
                session_id: "sess_cache_1".into(),
                source: "telegram".into(),
                model: "gpt-4o".into(),
                timestamp_utc: Some("2026-11-15T10:00:05Z".into()),
                ..Default::default()
            },
            enable_prompt_caching: true,
            ..Default::default()
        });

        assert_eq!(p1.tier1_stable.content, p2.tier1_stable.content);
        assert_eq!(p1.tier2_context.content, p2.tier2_context.content);
        assert_eq!(p1.tier1_hash, p2.tier1_hash);
        assert_ne!(p1.tier3_volatile.content, p2.tier3_volatile.content);
        assert!(!p1.tier1_stable.content.contains("2026-11-15T10:00:00Z"));
    }

    #[test]
    fn test_prompt_builder_preserves_hermes_doctrine_with_custom_soul() {
        let builder = PromptBuilder::new();
        let custom_soul = SoulProfile::parse("# Bot Alpha\nYou are a Telegram ops assistant.");
        let toolsets = vec!["computer_use".to_string(), "skills".to_string()];

        let prompt = builder.build(PromptBuildInput {
            soul: Some(&custom_soul),
            active_toolsets: &toolsets,
            volatile: VolatileRuntimeContext {
                session_id: "sess_doctrine".into(),
                source: "telegram".into(),
                model: "mock".into(),
                timestamp_utc: Some("2026-11-15T10:00:00Z".into()),
                ..Default::default()
            },
            enable_prompt_caching: true,
            ..Default::default()
        });

        assert!(prompt.tier1_stable.content.contains("Telegram ops assistant"));
        assert!(prompt
            .tier1_stable
            .content
            .contains("Autonomous Execution & Self-Learning Doctrine"));
        assert!(prompt
            .tier1_stable
            .content
            .contains("inspect -> execute -> verify -> report"));
        assert!(prompt.tier1_stable.content.contains("computer_exec"));
        assert!(prompt.tier1_stable.content.contains("skill_manage"));
        assert!(prompt.tier1_stable.content.contains("5+ tool calls"));
    }
}
