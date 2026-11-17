use crate::hermes::state_db::{HermesStateStore, now_ms};
use gpui_telegram_core::error::{Result, TelegramError};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Maximum character count (Unicode scalar values) for `MEMORY.md`.
pub const MEMORY_MAX_CHARS: usize = 2200;
/// Maximum character count (Unicode scalar values) for `USER.md`.
pub const USER_MAX_CHARS: usize = 1375;
/// Canonical delimiter separating entries in `MEMORY.md` and `USER.md`.
pub const MEMORY_DELIMITER: &str = "\n§\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTarget {
    Memory,
    User,
}

impl MemoryTarget {
    pub fn file_name(&self) -> &'static str {
        match self {
            Self::Memory => "MEMORY.md",
            Self::User => "USER.md",
        }
    }

    pub fn char_limit(&self) -> usize {
        match self {
            Self::Memory => MEMORY_MAX_CHARS,
            Self::User => USER_MAX_CHARS,
        }
    }

    pub fn kv_key(&self) -> &'static str {
        match self {
            Self::Memory => "memory_md",
            Self::User => "user_md",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "memory" | "memory.md" | "agent" => Some(Self::Memory),
            "user" | "user.md" | "profile" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryReadResult {
    pub target: MemoryTarget,
    pub raw: String,
    pub entries: Vec<String>,
    pub char_count: usize,
    pub char_limit: usize,
    pub remaining_chars: usize,
    pub usage_percent: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryMutationResult {
    pub target: MemoryTarget,
    pub action: String,
    pub entries: Vec<String>,
    pub raw: String,
    pub char_count: usize,
    pub char_limit: usize,
    pub remaining_chars: usize,
    pub usage_percent: f32,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub session_id: String,
    pub memory_raw: String,
    pub user_raw: String,
    pub memory_entries: Vec<String>,
    pub user_entries: Vec<String>,
    pub captured_at_ms: i64,
}

impl MemorySnapshot {
    pub fn is_empty(&self) -> bool {
        self.memory_entries.is_empty() && self.user_entries.is_empty()
    }

    pub fn render_prompt_block(&self) -> String {
        let mut sections = Vec::new();
        if !self.memory_entries.is_empty() {
            let bullets = self
                .memory_entries
                .iter()
                .map(|e| format!("- {e}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!(
                "### MEMORY.md (Frozen Snapshot: {}/{} chars)\n{bullets}",
                self.memory_raw.chars().count(),
                MEMORY_MAX_CHARS
            ));
        }
        if !self.user_entries.is_empty() {
            let bullets = self
                .user_entries
                .iter()
                .map(|e| format!("- {e}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!(
                "### USER.md (Frozen Snapshot: {}/{} chars)\n{bullets}",
                self.user_raw.chars().count(),
                USER_MAX_CHARS
            ));
        }
        sections.join("\n\n")
    }
}

/// Manages bounded `MEMORY.md` (2200 chars), `USER.md` (1375 chars),
/// frozen per-session snapshots for prompt caching, and `SOUL.md`.
#[derive(Clone)]
pub struct MemoryManager {
    base_dir: Option<PathBuf>,
    store: Option<HermesStateStore>,
    scope_key: String,
    in_memory_files: Arc<RwLock<HashMap<String, String>>>,
    snapshots: Arc<RwLock<HashMap<String, MemorySnapshot>>>,
}

impl std::fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryManager")
            .field("base_dir", &self.base_dir)
            .field("has_store", &self.store.is_some())
            .field("scope_key", &self.scope_key)
            .finish()
    }
}

impl MemoryManager {
    pub fn in_memory() -> Self {
        Self {
            base_dir: None,
            store: None,
            scope_key: "default".into(),
            in_memory_files: Arc::new(RwLock::new(HashMap::new())),
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_dir(dir: impl AsRef<Path>) -> Self {
        Self {
            base_dir: Some(dir.as_ref().to_path_buf()),
            store: None,
            scope_key: "default".into(),
            in_memory_files: Arc::new(RwLock::new(HashMap::new())),
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_store(store: HermesStateStore, scope_key: impl Into<String>) -> Self {
        Self {
            base_dir: None,
            store: Some(store),
            scope_key: scope_key.into(),
            in_memory_files: Arc::new(RwLock::new(HashMap::new())),
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_dir_and_store(
        dir: impl AsRef<Path>,
        store: HermesStateStore,
        scope_key: impl Into<String>,
    ) -> Self {
        Self {
            base_dir: Some(dir.as_ref().to_path_buf()),
            store: Some(store),
            scope_key: scope_key.into(),
            in_memory_files: Arc::new(RwLock::new(HashMap::new())),
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn base_dir(&self) -> Option<&Path> {
        self.base_dir.as_deref()
    }

    pub fn scope_key(&self) -> &str {
        &self.scope_key
    }

    fn scoped_kv_key(&self, key: &str) -> String {
        format!("{}:{}", self.scope_key, key)
    }

    fn read_blob(&self, file_name: &str, kv_key: &str) -> Result<String> {
        if let Some(dir) = &self.base_dir {
            let path = dir.join(file_name);
            if path.exists() {
                let raw = std::fs::read_to_string(&path)?;
                return Ok(raw);
            }
        }
        if let Some(store) = &self.store {
            if let Some(val) = store.kv_get("memory", &self.scoped_kv_key(kv_key))? {
                return Ok(val);
            }
        }
        let guard = self.in_memory_files.read();
        Ok(guard.get(kv_key).cloned().unwrap_or_default())
    }

    fn write_blob(&self, file_name: &str, kv_key: &str, content: &str) -> Result<()> {
        if let Some(dir) = &self.base_dir {
            std::fs::create_dir_all(dir)?;
            let target_path = dir.join(file_name);
            let tmp_path = dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4().simple()));
            std::fs::write(&tmp_path, content)?;
            std::fs::rename(&tmp_path, &target_path)?;
        }
        if let Some(store) = &self.store {
            store.kv_set("memory", &self.scoped_kv_key(kv_key), content)?;
        }
        let mut guard = self.in_memory_files.write();
        guard.insert(kv_key.to_string(), content.to_string());
        Ok(())
    }

    fn parse_entries(raw: &str) -> Vec<String> {
        if raw.trim().is_empty() {
            return Vec::new();
        }
        if raw.contains('§') {
            raw.split(MEMORY_DELIMITER)
                .flat_map(|chunk| chunk.split('§'))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            vec![raw.trim().to_string()]
        }
    }

    fn join_entries(entries: &[String]) -> String {
        entries.join(MEMORY_DELIMITER)
    }

    fn sanitize_entry(entry: &str) -> Result<String> {
        let cleaned = entry.trim().replace('§', "|");
        if cleaned.is_empty() {
            return Err(TelegramError::Validation(
                "Memory entry cannot be empty".into(),
            ));
        }
        Ok(cleaned)
    }

    fn build_read_result(target: MemoryTarget, raw: String, entries: Vec<String>) -> MemoryReadResult {
        let char_limit = target.char_limit();
        let char_count = raw.chars().count();
        let remaining_chars = char_limit.saturating_sub(char_count);
        let usage_percent = ((char_count as f32 / char_limit as f32) * 100.0).min(100.0);
        MemoryReadResult {
            target,
            raw,
            entries,
            char_count,
            char_limit,
            remaining_chars,
            usage_percent,
        }
    }

    fn commit_entries(
        &self,
        target: MemoryTarget,
        action: &str,
        entries: Vec<String>,
        changed: bool,
    ) -> Result<MemoryMutationResult> {
        let raw = Self::join_entries(&entries);
        let char_limit = target.char_limit();
        let char_count = raw.chars().count();
        if char_count > char_limit {
            return Err(TelegramError::Validation(format!(
                "{} capacity exceeded: {char_count}/{char_limit} chars (need to remove at least {} chars or replace existing entries)",
                target.file_name(),
                char_count - char_limit
            )));
        }

        if changed {
            self.write_blob(target.file_name(), target.kv_key(), &raw)?;
        }

        let remaining_chars = char_limit.saturating_sub(char_count);
        let usage_percent = (char_count as f32 / char_limit as f32) * 100.0;
        Ok(MemoryMutationResult {
            target,
            action: action.to_string(),
            entries,
            raw,
            char_count,
            char_limit,
            remaining_chars,
            usage_percent,
            changed,
        })
    }

    pub fn read(&self, target: MemoryTarget) -> Result<MemoryReadResult> {
        let raw = self.read_blob(target.file_name(), target.kv_key())?;
        let entries = Self::parse_entries(&raw);
        let canonical_raw = Self::join_entries(&entries);
        Ok(Self::build_read_result(target, canonical_raw, entries))
    }

    pub fn add(&self, target: MemoryTarget, entry: &str) -> Result<MemoryMutationResult> {
        let cleaned = Self::sanitize_entry(entry)?;
        let current = self.read(target)?;
        if current.entries.iter().any(|e| e == &cleaned) {
            return self.commit_entries(target, "add_noop_duplicate", current.entries, false);
        }
        let mut entries = current.entries;
        entries.push(cleaned);
        self.commit_entries(target, "add", entries, true)
    }

    pub fn replace(
        &self,
        target: MemoryTarget,
        old_match: &str,
        new_entry: &str,
    ) -> Result<MemoryMutationResult> {
        let needle = old_match.trim();
        if needle.is_empty() {
            return Err(TelegramError::Validation(
                "Replacement match pattern cannot be empty".into(),
            ));
        }
        let cleaned_new = Self::sanitize_entry(new_entry)?;
        let mut entries = self.read(target)?.entries;

        if let Some(idx) = entries.iter().position(|e| e == needle) {
            entries[idx] = cleaned_new;
            return self.commit_entries(target, "replace", entries, true);
        }

        let matching_indices: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter_map(|(idx, e)| if e.contains(needle) { Some(idx) } else { None })
            .collect();

        match matching_indices.as_slice() {
            [] => Err(TelegramError::NotFound(format!(
                "No entry matching '{}' found in {}",
                needle,
                target.file_name()
            ))),
            [idx] => {
                entries[*idx] = cleaned_new;
                self.commit_entries(target, "replace", entries, true)
            }
            many => Err(TelegramError::Validation(format!(
                "Ambiguous match '{}' matched {} entries in {}; provide a more specific substring",
                needle,
                many.len(),
                target.file_name()
            ))),
        }
    }

    pub fn remove(&self, target: MemoryTarget, match_substr: &str) -> Result<MemoryMutationResult> {
        let needle = match_substr.trim();
        if needle.is_empty() {
            return Err(TelegramError::Validation(
                "Removal match pattern cannot be empty".into(),
            ));
        }
        let mut entries = self.read(target)?.entries;

        if let Some(idx) = entries.iter().position(|e| e == needle) {
            entries.remove(idx);
            return self.commit_entries(target, "remove", entries, true);
        }

        let matching_indices: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter_map(|(idx, e)| if e.contains(needle) { Some(idx) } else { None })
            .collect();

        match matching_indices.as_slice() {
            [] => Err(TelegramError::NotFound(format!(
                "No entry matching '{}' found in {}",
                needle,
                target.file_name()
            ))),
            [idx] => {
                entries.remove(*idx);
                self.commit_entries(target, "remove", entries, true)
            }
            many => Err(TelegramError::Validation(format!(
                "Ambiguous match '{}' matched {} entries in {}; provide a more specific substring",
                needle,
                many.len(),
                target.file_name()
            ))),
        }
    }

    pub fn clear(&self, target: MemoryTarget) -> Result<MemoryMutationResult> {
        self.commit_entries(target, "clear", Vec::new(), true)
    }

    /// Replaces the entire file content (e.g. from Hermes Studio editor) while validating char limit.
    pub fn set_raw(&self, target: MemoryTarget, raw_content: &str) -> Result<MemoryMutationResult> {
        let entries = Self::parse_entries(raw_content);
        self.commit_entries(target, "set_raw", entries, true)
    }

    // =========================================================================
    // Frozen Snapshot Pattern (Prompt Cache Stability)
    // =========================================================================

    /// Captures and freezes `MEMORY.md` and `USER.md` for `session_id`.
    /// Subsequent live edits to disk/DB will not alter this frozen snapshot until
    /// `reload_session_snapshot` (`/reload-memory`) or a new session starts.
    pub fn capture_session_snapshot(&self, session_id: &str) -> Result<MemorySnapshot> {
        let mem = self.read(MemoryTarget::Memory)?;
        let user = self.read(MemoryTarget::User)?;
        let snapshot = MemorySnapshot {
            session_id: session_id.to_string(),
            memory_raw: mem.raw,
            user_raw: user.raw,
            memory_entries: mem.entries,
            user_entries: user.entries,
            captured_at_ms: now_ms(),
        };

        if let Some(store) = &self.store {
            let json = serde_json::to_string(&snapshot)?;
            store.kv_set("memory_snapshot", &self.scoped_kv_key(session_id), &json)?;
        }
        self.snapshots
            .write()
            .insert(session_id.to_string(), snapshot.clone());
        Ok(snapshot)
    }

    pub fn get_session_snapshot(&self, session_id: &str) -> Option<MemorySnapshot> {
        if let Some(snap) = self.snapshots.read().get(session_id).cloned() {
            return Some(snap);
        }
        if let Some(store) = &self.store {
            if let Ok(Some(json)) = store.kv_get("memory_snapshot", &self.scoped_kv_key(session_id))
            {
                if let Ok(snap) = serde_json::from_str::<MemorySnapshot>(&json) {
                    self.snapshots
                        .write()
                        .insert(session_id.to_string(), snap.clone());
                    return Some(snap);
                }
            }
        }
        None
    }

    pub fn get_or_capture_session_snapshot(&self, session_id: &str) -> Result<MemorySnapshot> {
        if let Some(existing) = self.get_session_snapshot(session_id) {
            return Ok(existing);
        }
        self.capture_session_snapshot(session_id)
    }

    /// Refreshes the frozen snapshot from live `MEMORY.md` and `USER.md` (`/reload-memory`).
    pub fn reload_session_snapshot(&self, session_id: &str) -> Result<MemorySnapshot> {
        self.capture_session_snapshot(session_id)
    }

    pub fn clear_session_snapshot(&self, session_id: &str) {
        self.snapshots.write().remove(session_id);
        if let Some(store) = &self.store {
            let _ = store.kv_delete("memory_snapshot", &self.scoped_kv_key(session_id));
        }
    }

    // =========================================================================
    // SOUL.md Persona Persistence
    // =========================================================================

    pub fn has_soul(&self) -> bool {
        self.read_blob("SOUL.md", "soul_md")
            .map(|raw| !raw.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn read_soul(&self) -> Result<SoulProfile> {
        let raw = self.read_blob("SOUL.md", "soul_md")?;
        if raw.trim().is_empty() {
            return Ok(SoulProfile::default());
        }
        Ok(SoulProfile::parse(&raw))
    }

    pub fn write_soul(&self, profile: &SoulProfile) -> Result<()> {
        let markdown = profile.to_markdown();
        self.write_blob("SOUL.md", "soul_md", &markdown)
    }

    pub fn write_soul_raw(&self, markdown: &str) -> Result<SoulProfile> {
        self.write_blob("SOUL.md", "soul_md", markdown)?;
        Ok(SoulProfile::parse(markdown))
    }
}

// =============================================================================
// SOUL.md Persona Profile
// =============================================================================

/// Customizable bot identity & persona parsed from/rendered to `SOUL.md`.
/// Contains zero hardcoded legacy branding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoulProfile {
    pub name: String,
    pub role: String,
    pub tone: String,
    pub persona: String,
    pub directives: Vec<String>,
    pub traits: Vec<String>,
    pub raw_markdown: String,
}

impl Default for SoulProfile {
    fn default() -> Self {
        let mut profile = Self {
            name: "Hermes".into(),
            role: "Personal AI Assistant".into(),
            tone: "Concise, accurate, collaborative, and transparent".into(),
            persona: "An adaptable autonomous assistant focused on solving user tasks safely with tools, persistent memory, and modular skills.".into(),
            directives: vec![
                "Respect workspace boundaries and security approvals before destructive actions.".into(),
                "Use verified tool outputs rather than fabricating results.".into(),
                "Adapt response language and depth to the user's request.".into(),
            ],
            traits: vec![
                "Tool-grounded".into(),
                "Memory-aware".into(),
                "Verification-driven".into(),
            ],
            raw_markdown: String::new(),
        };
        profile.raw_markdown = profile.to_markdown();
        profile
    }
}

impl SoulProfile {
    pub fn new(name: impl Into<String>, role: impl Into<String>, persona: impl Into<String>) -> Self {
        let mut profile = Self {
            name: name.into(),
            role: role.into(),
            tone: "Clear, helpful, and direct".into(),
            persona: persona.into(),
            directives: Vec::new(),
            traits: Vec::new(),
            raw_markdown: String::new(),
        };
        profile.raw_markdown = profile.to_markdown();
        profile
    }

    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }

        let mut name: Option<String> = None;
        let mut role: Option<String> = None;
        let mut tone: Option<String> = None;
        let mut persona_lines: Vec<String> = Vec::new();
        let mut directives: Vec<String> = Vec::new();
        let mut traits: Vec<String> = Vec::new();

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Section {
            None,
            Persona,
            Role,
            Tone,
            Directives,
            Traits,
        }

        let mut section = Section::None;
        for line in trimmed.lines() {
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            if let Some(h1) = l.strip_prefix("# ") {
                let heading = h1.trim();
                if name.is_none() && !heading.eq_ignore_ascii_case("SOUL") {
                    name = Some(
                        heading
                            .strip_prefix("SOUL:")
                            .or_else(|| heading.strip_prefix("Soul:"))
                            .unwrap_or(heading)
                            .trim()
                            .to_string(),
                    );
                }
                section = Section::Persona;
                continue;
            }
            if let Some(h2) = l.strip_prefix("## ") {
                let lower = h2.trim().to_ascii_lowercase();
                section = if lower.contains("role") || lower.contains("identity") {
                    Section::Role
                } else if lower.contains("tone") || lower.contains("style") || lower.contains("voice") {
                    Section::Tone
                } else if lower.contains("directive") || lower.contains("rule") || lower.contains("guideline") {
                    Section::Directives
                } else if lower.contains("trait") || lower.contains("value") {
                    Section::Traits
                } else {
                    Section::Persona
                };
                continue;
            }

            if let Some((k, v)) = l.split_once(':') {
                let kl = k.trim().trim_start_matches('-').trim().to_ascii_lowercase();
                let val = v.trim();
                if !val.is_empty() {
                    match kl.as_str() {
                        "name" | "bot_name" => {
                            name = Some(val.to_string());
                            continue;
                        }
                        "role" => {
                            role = Some(val.to_string());
                            continue;
                        }
                        "tone" | "style" => {
                            tone = Some(val.to_string());
                            continue;
                        }
                        _ => {}
                    }
                }
            }

            let bullet = l
                .strip_prefix("- ")
                .or_else(|| l.strip_prefix("* "))
                .map(str::trim);

            match section {
                Section::Role => {
                    if role.is_none() {
                        role = Some(bullet.unwrap_or(l).to_string());
                    }
                }
                Section::Tone => {
                    if tone.is_none() {
                        tone = Some(bullet.unwrap_or(l).to_string());
                    }
                }
                Section::Directives => {
                    directives.push(bullet.unwrap_or(l).to_string());
                }
                Section::Traits => {
                    traits.push(bullet.unwrap_or(l).to_string());
                }
                Section::Persona | Section::None => {
                    persona_lines.push(bullet.unwrap_or(l).to_string());
                }
            }
        }

        let default = Self::default();
        let persona_text = if persona_lines.is_empty() {
            trimmed.to_string()
        } else {
            persona_lines.join("\n")
        };

        Self {
            name: name.unwrap_or(default.name),
            role: role.unwrap_or(default.role),
            tone: tone.unwrap_or(default.tone),
            persona: persona_text,
            directives,
            traits,
            raw_markdown: raw.to_string(),
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut out = format!("# {}\n\n", self.name);
        out.push_str(&format!("- Name: {}\n", self.name));
        out.push_str(&format!("- Role: {}\n", self.role));
        out.push_str(&format!("- Tone: {}\n\n", self.tone));
        out.push_str("## Persona\n");
        out.push_str(self.persona.trim());
        out.push('\n');
        if !self.traits.is_empty() {
            out.push_str("\n## Traits\n");
            for t in &self.traits {
                out.push_str(&format!("- {t}\n"));
            }
        }
        if !self.directives.is_empty() {
            out.push_str("\n## Directives\n");
            for d in &self.directives {
                out.push_str(&format!("- {d}\n"));
            }
        }
        out
    }

    pub fn render_prompt_block(&self) -> String {
        let mut block = format!(
            "Identity: {} — {}\nTone: {}\nPersona:\n{}",
            self.name, self.role, self.tone, self.persona
        );
        if !self.traits.is_empty() {
            block.push_str(&format!("\nTraits: {}", self.traits.join(", ")));
        }
        if !self.directives.is_empty() {
            block.push_str("\nDirectives:");
            for d in &self.directives {
                block.push_str(&format!("\n- {d}"));
            }
        }
        block
    }
}

// =============================================================================
// Honcho Local Dialectic User Modeling
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationCategory {
    Preference,
    Goal,
    CommunicationStyle,
    Fact,
    Skill,
}

impl ObservationCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Goal => "goal",
            Self::CommunicationStyle => "communication_style",
            Self::Fact => "fact",
            Self::Skill => "skill",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "preference" | "pref" => Self::Preference,
            "goal" | "objective" => Self::Goal,
            "communication_style" | "style" | "tone" => Self::CommunicationStyle,
            "skill" | "expertise" => Self::Skill,
            _ => Self::Fact,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserObservation {
    pub id: String,
    pub category: ObservationCategory,
    pub content: String,
    pub confidence: f32,
    pub session_id: Option<String>,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialecticProfile {
    pub user_id: String,
    pub summary: String,
    pub preferences: Vec<String>,
    pub recurring_goals: Vec<String>,
    pub communication_style: Vec<String>,
    pub facts: Vec<String>,
    pub skills: Vec<String>,
    pub observations: Vec<UserObservation>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialecticQueryResult {
    pub user_id: String,
    pub query: String,
    pub answer: String,
    pub matched_observations: Vec<UserObservation>,
}

/// Pure-Rust local dialectic user modeling inspired by Honcho.
/// Extracts user preferences, recurring goals, communication style, and facts across sessions.
#[derive(Clone)]
pub struct HonchoDialectics {
    store: Option<HermesStateStore>,
    profiles: Arc<RwLock<HashMap<String, DialecticProfile>>>,
}

impl std::fmt::Debug for HonchoDialectics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HonchoDialectics")
            .field("has_store", &self.store.is_some())
            .finish()
    }
}

impl Default for HonchoDialectics {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl HonchoDialectics {
    pub fn in_memory() -> Self {
        Self {
            store: None,
            profiles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_store(store: HermesStateStore) -> Self {
        Self {
            store: Some(store),
            profiles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn load_or_default(&self, user_id: &str) -> Result<DialecticProfile> {
        if let Some(p) = self.profiles.read().get(user_id).cloned() {
            return Ok(p);
        }
        if let Some(store) = &self.store {
            if let Some(json) = store.kv_get("honcho_profile", user_id)? {
                if let Ok(p) = serde_json::from_str::<DialecticProfile>(&json) {
                    self.profiles.write().insert(user_id.to_string(), p.clone());
                    return Ok(p);
                }
            }
        }
        Ok(DialecticProfile {
            user_id: user_id.to_string(),
            summary: String::new(),
            preferences: Vec::new(),
            recurring_goals: Vec::new(),
            communication_style: Vec::new(),
            facts: Vec::new(),
            skills: Vec::new(),
            observations: Vec::new(),
            updated_at_ms: now_ms(),
        })
    }

    fn save_profile(&self, profile: &DialecticProfile) -> Result<()> {
        if let Some(store) = &self.store {
            let json = serde_json::to_string(profile)?;
            store.kv_set("honcho_profile", &profile.user_id, &json)?;
        }
        self.profiles
            .write()
            .insert(profile.user_id.clone(), profile.clone());
        Ok(())
    }

    pub fn record_observation(
        &self,
        user_id: &str,
        category: ObservationCategory,
        content: &str,
        confidence: f32,
        session_id: Option<&str>,
    ) -> Result<UserObservation> {
        let cleaned = content.trim();
        if cleaned.is_empty() {
            return Err(TelegramError::Validation(
                "Observation content cannot be empty".into(),
            ));
        }

        let mut profile = self.load_or_default(user_id)?;
        let now = now_ms();
        let clamped_conf = confidence.clamp(0.0, 1.0);

        if let Some(existing) = profile
            .observations
            .iter_mut()
            .find(|o| o.category == category && o.content.eq_ignore_ascii_case(cleaned))
        {
            existing.confidence = existing.confidence.max(clamped_conf);
            existing.observed_at_ms = now;
            if session_id.is_some() {
                existing.session_id = session_id.map(ToString::to_string);
            }
            let cloned = existing.clone();
            Self::rebuild_summary(&mut profile);
            profile.updated_at_ms = now;
            self.save_profile(&profile)?;
            return Ok(cloned);
        }

        let obs = UserObservation {
            id: format!("obs_{}", uuid::Uuid::new_v4().simple()),
            category,
            content: cleaned.to_string(),
            confidence: clamped_conf,
            session_id: session_id.map(ToString::to_string),
            observed_at_ms: now,
        };
        profile.observations.push(obs.clone());

        let target_vec = match category {
            ObservationCategory::Preference => &mut profile.preferences,
            ObservationCategory::Goal => &mut profile.recurring_goals,
            ObservationCategory::CommunicationStyle => &mut profile.communication_style,
            ObservationCategory::Fact => &mut profile.facts,
            ObservationCategory::Skill => &mut profile.skills,
        };
        if !target_vec.iter().any(|item| item.eq_ignore_ascii_case(cleaned)) {
            target_vec.push(cleaned.to_string());
        }

        Self::rebuild_summary(&mut profile);
        profile.updated_at_ms = now;
        self.save_profile(&profile)?;
        Ok(obs)
    }

    /// Deterministically extracts user modeling signals from a natural user message.
    pub fn ingest_user_message(
        &self,
        user_id: &str,
        session_id: Option<&str>,
        text: &str,
    ) -> Result<Vec<UserObservation>> {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') {
            return Ok(Vec::new());
        }

        let lower = trimmed.to_lowercase();
        let mut recorded = Vec::new();

        // Communication style cues
        let style_markers = [
            ("concise", "Prefers concise responses"),
            ("brief", "Prefers brief responses"),
            ("detailed", "Prefers detailed explanations"),
            ("step by step", "Prefers step-by-step explanations"),
            ("用中文", "Prefers communicating in Chinese"),
            ("reply in chinese", "Prefers communicating in Chinese"),
            ("reply in english", "Prefers communicating in English"),
            ("简洁", "Prefers concise Chinese responses"),
            ("详细", "Prefers detailed explanations"),
        ];
        for (marker, summary) in style_markers {
            if lower.contains(marker) {
                recorded.push(self.record_observation(
                    user_id,
                    ObservationCategory::CommunicationStyle,
                    summary,
                    0.82,
                    session_id,
                )?);
            }
        }

        // Preference cues
        let pref_prefixes = [
            "i prefer ",
            "always use ",
            "please always ",
            "don't use ",
            "never use ",
            "我喜欢",
            "我偏好",
            "以后请",
        ];
        for sentence in trimmed.split(['.', '\n', '。', '!', ';']) {
            let s = sentence.trim();
            let sl = s.to_lowercase();
            if s.chars().count() < 6 {
                continue;
            }
            if pref_prefixes.iter().any(|p| sl.contains(p)) {
                recorded.push(self.record_observation(
                    user_id,
                    ObservationCategory::Preference,
                    s,
                    0.85,
                    session_id,
                )?);
            } else if sl.contains("working on ")
                || sl.contains("building ")
                || sl.contains("my goal is ")
                || sl.contains("正在做")
                || sl.contains("目标是")
            {
                recorded.push(self.record_observation(
                    user_id,
                    ObservationCategory::Goal,
                    s,
                    0.78,
                    session_id,
                )?);
            } else if sl.contains("i work with ")
                || sl.contains("i use rust")
                || sl.contains("i am a ")
                || sl.contains("my name is ")
                || sl.contains("我是")
            {
                recorded.push(self.record_observation(
                    user_id,
                    ObservationCategory::Fact,
                    s,
                    0.80,
                    session_id,
                )?);
            }
        }

        Ok(recorded)
    }

    pub fn profile(&self, user_id: &str) -> Result<DialecticProfile> {
        self.load_or_default(user_id)
    }

    pub fn query(&self, user_id: &str, question: &str) -> Result<DialecticQueryResult> {
        let profile = self.load_or_default(user_id)?;
        let q_lower = question.trim().to_lowercase();
        let terms: Vec<&str> = q_lower
            .split(|c: char| !c.is_alphanumeric() && !is_cjk_char(c))
            .filter(|t| !t.is_empty())
            .collect();

        let mut scored: Vec<(usize, UserObservation)> = profile
            .observations
            .iter()
            .filter_map(|obs| {
                let hay = format!("{} {}", obs.category.as_str(), obs.content).to_lowercase();
                let mut score = 0usize;
                if !q_lower.is_empty() && hay.contains(&q_lower) {
                    score += 5;
                }
                for term in &terms {
                    if hay.contains(term) {
                        score += 2;
                    }
                }
                if score > 0 || q_lower.is_empty() {
                    Some((score, obs.clone()))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.observed_at_ms.cmp(&a.1.observed_at_ms))
        });

        let matched_observations: Vec<UserObservation> =
            scored.into_iter().take(10).map(|(_, o)| o).collect();

        let answer = if matched_observations.is_empty() {
            if profile.summary.is_empty() {
                format!("No recorded dialectic observations yet for user '{user_id}'.")
            } else {
                format!(
                    "No exact match for '{question}', current profile summary: {}",
                    profile.summary
                )
            }
        } else {
            let bullets = matched_observations
                .iter()
                .map(|o| format!("[{} | {:.0}%] {}", o.category.as_str(), o.confidence * 100.0, o.content))
                .collect::<Vec<_>>()
                .join("; ");
            format!("Dialectic insights for '{user_id}': {bullets}")
        };

        Ok(DialecticQueryResult {
            user_id: user_id.to_string(),
            query: question.to_string(),
            answer,
            matched_observations,
        })
    }

    pub fn render_prompt_summary(&self, user_id: &str) -> Result<String> {
        let profile = self.load_or_default(user_id)?;
        Ok(profile.summary)
    }

    fn rebuild_summary(profile: &mut DialecticProfile) {
        let mut parts = Vec::new();
        if !profile.communication_style.is_empty() {
            parts.push(format!("Style: {}", profile.communication_style.join("; ")));
        }
        if !profile.preferences.is_empty() {
            parts.push(format!("Preferences: {}", profile.preferences.join("; ")));
        }
        if !profile.recurring_goals.is_empty() {
            parts.push(format!("Goals: {}", profile.recurring_goals.join("; ")));
        }
        if !profile.facts.is_empty() {
            parts.push(format!("Facts: {}", profile.facts.join("; ")));
        }
        if !profile.skills.is_empty() {
            parts.push(format!("Skills: {}", profile.skills.join("; ")));
        }
        profile.summary = parts.join("\n");
    }
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

// =============================================================================
// MemoryProvider Trait & Registry
// =============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchItem {
    pub provider: String,
    pub category: String,
    pub content: String,
    pub relevance: f32,
}

pub trait MemoryProvider: Send + Sync {
    fn name(&self) -> &str;
    fn recall(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<MemorySearchItem>>;
    fn store(&self, user_id: &str, category: &str, content: &str) -> Result<()>;
    fn summarize(&self, user_id: &str) -> Result<String>;
}

/// Built-in provider backed by `MemoryManager` (`MEMORY.md` / `USER.md`).
pub struct LocalFileMemoryProvider {
    manager: MemoryManager,
}

impl LocalFileMemoryProvider {
    pub fn new(manager: MemoryManager) -> Self {
        Self { manager }
    }
}

impl MemoryProvider for LocalFileMemoryProvider {
    fn name(&self) -> &str {
        "local_file"
    }

    fn recall(&self, _user_id: &str, query: &str, limit: usize) -> Result<Vec<MemorySearchItem>> {
        let q = query.trim().to_lowercase();
        let mut results = Vec::new();
        for (target, label) in [(MemoryTarget::Memory, "memory"), (MemoryTarget::User, "user")] {
            let read = self.manager.read(target)?;
            for entry in read.entries {
                if q.is_empty() || entry.to_lowercase().contains(&q) {
                    results.push(MemorySearchItem {
                        provider: self.name().into(),
                        category: label.into(),
                        content: entry,
                        relevance: if q.is_empty() { 0.5 } else { 0.9 },
                    });
                }
            }
        }
        results.truncate(limit);
        Ok(results)
    }

    fn store(&self, _user_id: &str, category: &str, content: &str) -> Result<()> {
        let target = MemoryTarget::parse(category).unwrap_or(MemoryTarget::Memory);
        self.manager.add(target, content)?;
        Ok(())
    }

    fn summarize(&self, _user_id: &str) -> Result<String> {
        let mem = self.manager.read(MemoryTarget::Memory)?;
        let user = self.manager.read(MemoryTarget::User)?;
        Ok(format!(
            "MEMORY ({}/{}): {} entries | USER ({}/{}): {} entries",
            mem.char_count,
            mem.char_limit,
            mem.entries.len(),
            user.char_count,
            user.char_limit,
            user.entries.len()
        ))
    }
}

/// Built-in provider backed by `HermesStateStore` (`kv_state` + `session_search`).
pub struct SqliteMemoryProvider {
    store: HermesStateStore,
}

impl SqliteMemoryProvider {
    pub fn new(store: HermesStateStore) -> Self {
        Self { store }
    }
}

impl MemoryProvider for SqliteMemoryProvider {
    fn name(&self) -> &str {
        "sqlite_store"
    }

    fn recall(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<MemorySearchItem>> {
        let mut items = Vec::new();
        let ns = format!("user_mem:{user_id}");
        let q = query.trim().to_lowercase();
        for (k, v) in self.store.kv_list(&ns)? {
            if q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q) {
                items.push(MemorySearchItem {
                    provider: self.name().into(),
                    category: k,
                    content: v,
                    relevance: 0.85,
                });
            }
        }
        if !q.is_empty() && items.len() < limit {
            let remaining = limit - items.len();
            for hit in self.store.session_search(query, None, remaining)? {
                items.push(MemorySearchItem {
                    provider: self.name().into(),
                    category: format!("session:{}", hit.session_id),
                    content: hit.snippet,
                    relevance: 0.75,
                });
            }
        }
        items.truncate(limit);
        Ok(items)
    }

    fn store(&self, user_id: &str, category: &str, content: &str) -> Result<()> {
        let ns = format!("user_mem:{user_id}");
        let key = format!("{category}:{}", uuid::Uuid::new_v4().simple());
        self.store.kv_set(&ns, &key, content)
    }

    fn summarize(&self, user_id: &str) -> Result<String> {
        let ns = format!("user_mem:{user_id}");
        let entries = self.store.kv_list(&ns)?;
        Ok(format!("SQLite stored user memories: {}", entries.len()))
    }
}

/// Built-in provider backed by `HonchoDialectics`.
pub struct HonchoMemoryProvider {
    honcho: HonchoDialectics,
}

impl HonchoMemoryProvider {
    pub fn new(honcho: HonchoDialectics) -> Self {
        Self { honcho }
    }
}

impl MemoryProvider for HonchoMemoryProvider {
    fn name(&self) -> &str {
        "honcho"
    }

    fn recall(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<MemorySearchItem>> {
        let res = self.honcho.query(user_id, query)?;
        Ok(res
            .matched_observations
            .into_iter()
            .take(limit)
            .map(|o| MemorySearchItem {
                provider: self.name().into(),
                category: o.category.as_str().to_string(),
                content: o.content,
                relevance: o.confidence,
            })
            .collect())
    }

    fn store(&self, user_id: &str, category: &str, content: &str) -> Result<()> {
        self.honcho.record_observation(
            user_id,
            ObservationCategory::parse(category),
            content,
            0.85,
            None,
        )?;
        Ok(())
    }

    fn summarize(&self, user_id: &str) -> Result<String> {
        self.honcho.render_prompt_summary(user_id)
    }
}

/// Registry orchestrating multiple `MemoryProvider` plugins.
#[derive(Clone, Default)]
pub struct MemoryProviderRegistry {
    providers: Arc<RwLock<HashMap<String, Arc<dyn MemoryProvider>>>>,
    active_order: Arc<RwLock<Vec<String>>>,
}

impl std::fmt::Debug for MemoryProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryProviderRegistry")
            .field("providers", &self.list_providers())
            .finish()
    }
}

impl MemoryProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin(
        memory: MemoryManager,
        store: Option<HermesStateStore>,
        honcho: HonchoDialectics,
    ) -> Self {
        let registry = Self::new();
        registry.register(Arc::new(LocalFileMemoryProvider::new(memory)));
        if let Some(s) = store {
            registry.register(Arc::new(SqliteMemoryProvider::new(s)));
        }
        registry.register(Arc::new(HonchoMemoryProvider::new(honcho)));
        registry
    }

    pub fn register(&self, provider: Arc<dyn MemoryProvider>) {
        let name = provider.name().to_string();
        self.providers.write().insert(name.clone(), provider);
        let mut order = self.active_order.write();
        if !order.contains(&name) {
            order.push(name);
        }
    }

    pub fn list_providers(&self) -> Vec<String> {
        self.active_order.read().clone()
    }

    pub fn recall_all(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<MemorySearchItem>> {
        let order = self.active_order.read().clone();
        let providers = self.providers.read();
        let mut combined = Vec::new();
        let mut seen = BTreeSet::new();
        for name in order {
            if let Some(provider) = providers.get(&name) {
                for item in provider.recall(user_id, query, limit)? {
                    let dedup_key = format!("{}:{}", item.category, item.content);
                    if seen.insert(dedup_key) {
                        combined.push(item);
                    }
                }
            }
        }
        combined.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        combined.truncate(limit);
        Ok(combined)
    }
}
