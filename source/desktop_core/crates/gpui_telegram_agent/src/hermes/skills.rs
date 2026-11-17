use crate::hermes::state_db::now_ms;
use gpui_telegram_core::error::{Result, TelegramError};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const NO_BUNDLED_SKILLS_MARKER: &str = ".no-bundled-skills";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    Bundled,
    Workspace,
    User,
    Learned,
    Hub,
    Custom(String),
}

impl Default for SkillOrigin {
    fn default() -> Self {
        Self::User
    }
}

impl SkillOrigin {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bundled => "bundled",
            Self::Workspace => "workspace",
            Self::User => "user",
            Self::Learned => "learned",
            Self::Hub => "hub",
            Self::Custom(s) => s.as_str(),
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bundled" => Self::Bundled,
            "workspace" => Self::Workspace,
            "user" | "" => Self::User,
            "learned" => Self::Learned,
            "hub" => Self::Hub,
            other => Self::Custom(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub platforms: Vec<String>,
    pub toolsets: Vec<String>,
    pub tags: Vec<String>,
    pub version: String,
    pub origin: SkillOrigin,
}

impl Default for SkillFrontmatter {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            platforms: Vec::new(),
            toolsets: Vec::new(),
            tags: Vec::new(),
            version: "1.0.0".into(),
            origin: SkillOrigin::User,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupplementaryKind {
    Script,
    Reference,
    Asset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSupplementaryFile {
    pub relative_path: String,
    pub kind: SupplementaryKind,
    pub size_bytes: usize,
    pub content_preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSecurityRisk {
    Safe,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSecurityFinding {
    pub rule_id: String,
    pub severity: SkillSecurityRisk,
    pub location: String,
    pub matched_snippet: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSecurityReport {
    pub risk_level: SkillSecurityRisk,
    pub safe_to_auto_install: bool,
    pub findings: Vec<SkillSecurityFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDocument {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub raw_markdown: String,
    pub source_path: Option<PathBuf>,
    pub supplementary_files: Vec<SkillSupplementaryFile>,
    pub trusted: bool,
    pub security_report: SkillSecurityReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillIndexEntry {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub toolsets: Vec<String>,
    pub origin: SkillOrigin,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub description: String,
    pub body: String,
    pub platforms: Option<Vec<String>>,
    pub toolsets: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub version: Option<String>,
    pub origin: Option<SkillOrigin>,
    pub trusted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillWriteStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillWriteProposal {
    pub id: String,
    pub skill_name: String,
    pub proposed_markdown: String,
    pub origin: SkillOrigin,
    pub security_report: SkillSecurityReport,
    pub status: SkillWriteStatus,
    pub rejection_reason: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLearningTurn {
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
}

/// Manages `agentskills.io`-compatible `SKILL.md` documents, progressive disclosure,
/// bundled defaults with `.no-bundled-skills` opt-out, `/learn` extraction,
/// untrusted write approval queue, and static security scanning.
#[derive(Clone)]
pub struct SkillsManager {
    skills_dir: Option<PathBuf>,
    skills: Arc<RwLock<HashMap<String, SkillDocument>>>,
    pending_writes: Arc<RwLock<HashMap<String, SkillWriteProposal>>>,
    no_bundled_skills: Arc<RwLock<bool>>,
}

impl std::fmt::Debug for SkillsManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsManager")
            .field("skills_dir", &self.skills_dir)
            .field("count", &self.skills.read().len())
            .field("no_bundled_skills", &*self.no_bundled_skills.read())
            .finish()
    }
}

impl SkillsManager {
    pub fn in_memory() -> Self {
        let mgr = Self {
            skills_dir: None,
            skills: Arc::new(RwLock::new(HashMap::new())),
            pending_writes: Arc::new(RwLock::new(HashMap::new())),
            no_bundled_skills: Arc::new(RwLock::new(false)),
        };
        mgr.load_bundled_if_enabled();
        mgr
    }

    pub fn with_dir(skills_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = skills_dir.as_ref().to_path_buf();
        let opt_out = dir.join(NO_BUNDLED_SKILLS_MARKER).exists();
        let mgr = Self {
            skills_dir: Some(dir),
            skills: Arc::new(RwLock::new(HashMap::new())),
            pending_writes: Arc::new(RwLock::new(HashMap::new())),
            no_bundled_skills: Arc::new(RwLock::new(opt_out)),
        };
        mgr.reload_from_disk()?;
        Ok(mgr)
    }

    pub fn skills_dir(&self) -> Option<&Path> {
        self.skills_dir.as_deref()
    }

    pub fn is_no_bundled_skills(&self) -> bool {
        *self.no_bundled_skills.read()
    }

    pub fn set_no_bundled_skills(&self, opt_out: bool) -> Result<()> {
        *self.no_bundled_skills.write() = opt_out;
        if let Some(dir) = &self.skills_dir {
            std::fs::create_dir_all(dir)?;
            let marker = dir.join(NO_BUNDLED_SKILLS_MARKER);
            if opt_out {
                std::fs::write(&marker, "# Opt out of bundled Hermes default skills\n")?;
            } else if marker.exists() {
                std::fs::remove_file(&marker)?;
            }
        }

        if opt_out {
            self.skills
                .write()
                .retain(|_, doc| doc.frontmatter.origin != SkillOrigin::Bundled);
        } else {
            self.load_bundled_if_enabled();
        }
        Ok(())
    }

    pub fn reload_from_disk(&self) -> Result<()> {
        let mut loaded: HashMap<String, SkillDocument> = HashMap::new();
        if let Some(dir) = &self.skills_dir {
            let opt_out = dir.join(NO_BUNDLED_SKILLS_MARKER).exists();
            *self.no_bundled_skills.write() = opt_out;
            if dir.exists() {
                for entry in std::fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() {
                        let skill_md = path.join("SKILL.md");
                        if skill_md.exists() {
                            let raw = std::fs::read_to_string(&skill_md)?;
                            let supplementary = collect_supplementary_files(&path)?;
                            let mut doc = parse_skill_markdown(
                                &raw,
                                Some(path.file_name().and_then(|n| n.to_str()).unwrap_or("skill")),
                                &supplementary,
                            )?;
                            doc.source_path = Some(skill_md);
                            doc.supplementary_files = supplementary;
                            loaded.insert(doc.frontmatter.name.clone(), doc);
                        }
                    } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                        let file_stem = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("skill");
                        let raw = std::fs::read_to_string(&path)?;
                        let mut doc = parse_skill_markdown(&raw, Some(file_stem), &[])?;
                        doc.source_path = Some(path);
                        loaded.insert(doc.frontmatter.name.clone(), doc);
                    }
                }
            }
        }

        {
            let mut guard = self.skills.write();
            *guard = loaded;
        }
        self.load_bundled_if_enabled();
        Ok(())
    }

    fn load_bundled_if_enabled(&self) {
        if *self.no_bundled_skills.read() {
            return;
        }
        let mut guard = self.skills.write();
        for raw in BUNDLED_SKILLS {
            if let Ok(mut doc) = parse_skill_markdown(raw, None, &[]) {
                doc.frontmatter.origin = SkillOrigin::Bundled;
                doc.trusted = true;
                guard.entry(doc.frontmatter.name.clone()).or_insert(doc);
            }
        }
    }

    /// Lightweight progressive disclosure index for system prompt Tier 1.
    pub fn skills_list_index(&self) -> Vec<SkillIndexEntry> {
        let guard = self.skills.read();
        let mut entries: Vec<SkillIndexEntry> = guard
            .values()
            .map(|doc| SkillIndexEntry {
                name: doc.frontmatter.name.clone(),
                description: doc.frontmatter.description.clone(),
                tags: doc.frontmatter.tags.clone(),
                toolsets: doc.frontmatter.toolsets.clone(),
                origin: doc.frontmatter.origin.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// Compact XML/markdown skill index injected into Tier 1 stable prompt.
    pub fn render_progressive_index(&self) -> String {
        let index = self.skills_list_index();
        if index.is_empty() {
            return String::new();
        }
        let mut out = String::from("<available_skills>\n");
        for entry in index {
            let toolsets = if entry.toolsets.is_empty() {
                String::new()
            } else {
                format!(" [toolsets: {}]", entry.toolsets.join(", "))
            };
            out.push_str(&format!(
                "- `{}` ({}): {}{}\n",
                entry.name,
                entry.origin.as_str(),
                entry.description,
                toolsets
            ));
        }
        out.push_str("</available_skills>");
        out
    }

    pub fn skills_list(&self, filter: Option<&str>) -> Vec<SkillDocument> {
        let q = filter.unwrap_or("").trim().to_lowercase();
        let guard = self.skills.read();
        let mut list: Vec<SkillDocument> = guard
            .values()
            .filter(|doc| {
                if q.is_empty() {
                    return true;
                }
                doc.frontmatter.name.to_lowercase().contains(&q)
                    || doc.frontmatter.description.to_lowercase().contains(&q)
                    || doc
                        .frontmatter
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&q))
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| a.frontmatter.name.cmp(&b.frontmatter.name));
        list
    }

    /// Loads full `SKILL.md` document and supplementary files on demand.
    pub fn skill_view(&self, name: &str) -> Result<SkillDocument> {
        let key = normalize_skill_name(name);
        let guard = self.skills.read();
        guard
            .get(&key)
            .cloned()
            .ok_or_else(|| TelegramError::NotFound(format!("Skill '{name}' not found")))
    }

    pub fn skill_create(&self, req: CreateSkillRequest) -> Result<SkillDocument> {
        let name = validate_skill_name(&req.name)?;
        if req.description.trim().is_empty() {
            return Err(TelegramError::Validation(
                "Skill description cannot be empty".into(),
            ));
        }

        let fm = SkillFrontmatter {
            name: name.clone(),
            description: req.description.trim().to_string(),
            platforms: req.platforms.unwrap_or_default(),
            toolsets: req.toolsets.unwrap_or_default(),
            tags: req.tags.unwrap_or_default(),
            version: req.version.unwrap_or_else(|| "1.0.0".into()),
            origin: req.origin.unwrap_or(SkillOrigin::User),
        };
        let raw_markdown = render_skill_markdown(&fm, req.body.trim());
        let security_report = skill_scan_security(&raw_markdown, &[]);

        let source_path = self.persist_skill_to_disk(&name, &raw_markdown)?;
        let doc = SkillDocument {
            frontmatter: fm,
            body: req.body.trim().to_string(),
            raw_markdown,
            source_path,
            supplementary_files: Vec::new(),
            trusted: req.trusted.unwrap_or(true),
            security_report,
        };
        self.skills.write().insert(name, doc.clone());
        Ok(doc)
    }

    pub fn skill_upsert_markdown(
        &self,
        raw_markdown: &str,
        origin_override: Option<SkillOrigin>,
        trusted: bool,
    ) -> Result<SkillDocument> {
        let mut doc = parse_skill_markdown(raw_markdown, None, &[])?;
        if let Some(origin) = origin_override {
            doc.frontmatter.origin = origin;
        }
        doc.trusted = trusted;
        doc.raw_markdown = render_skill_markdown(&doc.frontmatter, &doc.body);
        doc.security_report = skill_scan_security(&doc.raw_markdown, &[]);
        doc.source_path = self.persist_skill_to_disk(&doc.frontmatter.name, &doc.raw_markdown)?;
        self.skills
            .write()
            .insert(doc.frontmatter.name.clone(), doc.clone());
        Ok(doc)
    }

    pub fn skill_patch(
        &self,
        name: &str,
        old_text: &str,
        new_text: &str,
    ) -> Result<SkillDocument> {
        let key = normalize_skill_name(name);
        let existing = self.skill_view(&key)?;

        let updated_raw = if existing.raw_markdown.contains(old_text) {
            existing.raw_markdown.replacen(old_text, new_text, 1)
        } else if existing.body.contains(old_text) {
            let new_body = existing.body.replacen(old_text, new_text, 1);
            render_skill_markdown(&existing.frontmatter, &new_body)
        } else {
            return Err(TelegramError::Validation(format!(
                "Patch target text not found in skill '{key}'"
            )));
        };

        let mut updated =
            parse_skill_markdown(&updated_raw, Some(&key), &existing.supplementary_files)?;
        updated.trusted = existing.trusted;
        updated.supplementary_files = existing.supplementary_files;
        updated.source_path = self.persist_skill_to_disk(&key, &updated.raw_markdown)?;
        self.skills.write().insert(key, updated.clone());
        Ok(updated)
    }

    pub fn skill_delete(&self, name: &str) -> Result<bool> {
        let key = normalize_skill_name(name);
        let removed = self.skills.write().remove(&key);
        if let Some(dir) = &self.skills_dir {
            let skill_subdir = dir.join(&key);
            if skill_subdir.exists() && skill_subdir.is_dir() {
                std::fs::remove_dir_all(&skill_subdir)?;
            }
            let flat_file = dir.join(format!("{key}.md"));
            if flat_file.exists() {
                std::fs::remove_file(&flat_file)?;
            }
        }
        Ok(removed.is_some())
    }

    /// `/learn` procedural skill extraction from a completed or ongoing session transcript.
    pub fn learn_from_session(
        &self,
        skill_name: &str,
        session_title: &str,
        turns: &[SkillLearningTurn],
        tags: Vec<String>,
    ) -> Result<SkillDocument> {
        let name = validate_skill_name(skill_name)?;
        if turns.is_empty() {
            return Err(TelegramError::Validation(
                "Cannot extract a procedural skill from an empty session".into(),
            ));
        }

        let mut observed_tools: Vec<String> = Vec::new();
        let mut user_goals: Vec<String> = Vec::new();
        let mut procedure_steps: Vec<String> = Vec::new();

        for turn in turns {
            if let Some(tool) = &turn.tool_name {
                let t = tool.trim();
                if !t.is_empty() && !observed_tools.iter().any(|x| x == t) {
                    observed_tools.push(t.to_string());
                }
            }
            let summary_line = first_meaningful_line(&turn.content, 160);
            if summary_line.is_empty() {
                continue;
            }
            match turn.role.as_str() {
                "user" => user_goals.push(summary_line),
                "assistant" => procedure_steps.push(format!("Assistant action: {summary_line}")),
                "tool" => {
                    let tool_label = turn.tool_name.as_deref().unwrap_or("tool");
                    procedure_steps.push(format!("Verify via `{tool_label}`: {summary_line}"));
                }
                _ => {}
            }
        }

        let goal_summary = user_goals
            .first()
            .cloned()
            .unwrap_or_else(|| session_title.to_string());
        let description = format!("Learned procedural workflow for: {goal_summary}");

        let mut body = format!("# {name}\n\n## When to Use\n- {goal_summary}\n\n## Procedure\n");
        if procedure_steps.is_empty() {
            body.push_str("1. Review the request requirements and verify workspace context.\n");
        } else {
            for (idx, step) in procedure_steps.iter().take(12).enumerate() {
                body.push_str(&format!("{}. {step}\n", idx + 1));
            }
        }
        if !observed_tools.is_empty() {
            body.push_str("\n## Tools Observed\n");
            for t in &observed_tools {
                body.push_str(&format!("- `{t}`\n"));
            }
        }

        let mut merged_tags = tags;
        if !merged_tags.iter().any(|t| t == "learned") {
            merged_tags.push("learned".into());
        }

        self.skill_create(CreateSkillRequest {
            name,
            description,
            body,
            platforms: Some(vec!["macos".into(), "linux".into()]),
            toolsets: Some(vec!["file".into(), "terminal".into()]),
            tags: Some(merged_tags),
            version: Some("1.0.0".into()),
            origin: Some(SkillOrigin::Learned),
            trusted: Some(true),
        })
    }

    /// Minimum multi-step tool call count in a turn before self-learning distillation is encouraged.
    pub const SELF_LEARNING_TOOL_CALL_THRESHOLD: usize = 5;

    /// Returns `true` when a multi-step turn executed enough tool calls (`>= 5`) to check whether
    /// durable facts or reusable procedures should be persisted to `memory` / `skill_manage`.
    pub fn should_consider_self_learning(tool_calls_count: usize) -> bool {
        tool_calls_count >= Self::SELF_LEARNING_TOOL_CALL_THRESHOLD
    }

    /// Renders a deterministic self-learning prompt reminder for complex multi-step workflows
    /// (`>= 5` tool calls) without breaking Tier-1/Tier-2 system prompt caching.
    pub fn render_self_learning_nudge(
        tool_calls_count: usize,
        observed_tools: &[String],
    ) -> Option<String> {
        if !Self::should_consider_self_learning(tool_calls_count) {
            return None;
        }
        let mut unique_tools = Vec::new();
        for tool in observed_tools {
            let trimmed = tool.trim();
            if !trimmed.is_empty() && !unique_tools.iter().any(|existing| existing == trimmed) {
                unique_tools.push(trimmed.to_string());
            }
        }
        unique_tools.sort();
        let tools_summary = if unique_tools.is_empty() {
            "none recorded".to_string()
        } else {
            unique_tools.join(", ")
        };
        Some(format!(
            "[Self-Learning Check: Completed {tool_calls_count} tool calls ({tools_summary}). \
             If you discovered durable environment details, host quirks, or user preferences, save them via `memory`; \
             if you established a reusable multi-step procedure, inspect existing skills with `skill_list` / `skill_view` \
             and distill or update one with `skill_manage`.]"
        ))
    }

    // =========================================================================
    // Untrusted Skill Write-Approval Queue
    // =========================================================================

    pub fn propose_skill_write(
        &self,
        skill_name: &str,
        markdown: &str,
        origin: SkillOrigin,
    ) -> Result<SkillWriteProposal> {
        let name = validate_skill_name(skill_name)?;
        let parsed = parse_skill_markdown(markdown, Some(&name), &[])?;
        let security_report = skill_scan_security(&parsed.raw_markdown, &[]);

        let proposal = SkillWriteProposal {
            id: format!("skprop_{}", uuid::Uuid::new_v4().simple()),
            skill_name: name,
            proposed_markdown: parsed.raw_markdown,
            origin,
            security_report,
            status: SkillWriteStatus::Pending,
            rejection_reason: None,
            created_at_ms: now_ms(),
        };
        self.pending_writes
            .write()
            .insert(proposal.id.clone(), proposal.clone());
        Ok(proposal)
    }

    pub fn list_pending_skill_writes(&self) -> Vec<SkillWriteProposal> {
        let mut list: Vec<SkillWriteProposal> = self
            .pending_writes
            .read()
            .values()
            .filter(|p| p.status == SkillWriteStatus::Pending)
            .cloned()
            .collect();
        list.sort_by_key(|p| p.created_at_ms);
        list
    }

    pub fn approve_skill_write(
        &self,
        proposal_id: &str,
        allow_high_risk: bool,
    ) -> Result<SkillDocument> {
        let mut proposals = self.pending_writes.write();
        let proposal = proposals
            .get_mut(proposal_id)
            .ok_or_else(|| TelegramError::NotFound(format!("Skill proposal '{proposal_id}' not found")))?;

        if proposal.status != SkillWriteStatus::Pending {
            return Err(TelegramError::Validation(format!(
                "Skill proposal '{proposal_id}' already decided"
            )));
        }

        if proposal.security_report.risk_level >= SkillSecurityRisk::High && !allow_high_risk {
            return Err(TelegramError::PermissionDenied(format!(
                "Skill '{}' failed security scan ({:?}); explicit allow_high_risk=true is required",
                proposal.skill_name, proposal.security_report.risk_level
            )));
        }

        proposal.status = SkillWriteStatus::Approved;
        let markdown = proposal.proposed_markdown.clone();
        let origin = proposal.origin.clone();
        drop(proposals);

        self.skill_upsert_markdown(&markdown, Some(origin), true)
    }

    pub fn reject_skill_write(
        &self,
        proposal_id: &str,
        reason: Option<&str>,
    ) -> Result<SkillWriteProposal> {
        let mut proposals = self.pending_writes.write();
        let proposal = proposals
            .get_mut(proposal_id)
            .ok_or_else(|| TelegramError::NotFound(format!("Skill proposal '{proposal_id}' not found")))?;
        proposal.status = SkillWriteStatus::Rejected;
        proposal.rejection_reason = reason.map(ToString::to_string);
        Ok(proposal.clone())
    }

    fn persist_skill_to_disk(&self, name: &str, raw_markdown: &str) -> Result<Option<PathBuf>> {
        let Some(dir) = &self.skills_dir else {
            return Ok(None);
        };
        let skill_subdir = dir.join(name);
        std::fs::create_dir_all(&skill_subdir)?;
        let target = skill_subdir.join("SKILL.md");
        let tmp = skill_subdir.join(format!(".SKILL.md.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp, raw_markdown)?;
        std::fs::rename(&tmp, &target)?;
        Ok(Some(target))
    }
}

// =============================================================================
// Static Security Scanner for SKILL.md & Scripts
// =============================================================================

struct SecurityRule {
    id: &'static str,
    severity: SkillSecurityRisk,
    patterns: &'static [&'static str],
    reason: &'static str,
}

const SECURITY_RULES: &[SecurityRule] = &[
    SecurityRule {
        id: "rce_pipe_shell",
        severity: SkillSecurityRisk::Critical,
        patterns: &[
            "curl|sh",
            "curl | sh",
            "curl | bash",
            "wget | sh",
            "wget | bash",
            "| bash",
            "| sh -c",
            "base64 -d | sh",
            "base64 --decode | sh",
            "eval $(curl",
            "eval $(wget",
        ],
        reason: "Remote script piped directly into shell execution",
    },
    SecurityRule {
        id: "destructive_fs",
        severity: SkillSecurityRisk::Critical,
        patterns: &[
            "rm -rf /",
            "rm -rf /*",
            "rm -rf ~",
            "mkfs.",
            "dd if=/dev/zero",
            ":(){ :|:& };:",
            "chmod -r 777 /",
            "chmod 777 /",
        ],
        reason: "Destructive filesystem or system-wide command",
    },
    SecurityRule {
        id: "secret_exfiltration",
        severity: SkillSecurityRisk::High,
        patterns: &[
            "~/.ssh/id_",
            "/etc/shadow",
            "aws_secret_access_key",
            "curl -d @",
            "curl --data @",
            "wget --post-file",
            "nc -e ",
            "invoke-webrequest",
            "printenv | curl",
            "env | curl",
        ],
        reason: "Potential credential access or external data exfiltration",
    },
    SecurityRule {
        id: "prompt_injection_override",
        severity: SkillSecurityRisk::High,
        patterns: &[
            "ignore previous instructions",
            "ignore all previous instructions",
            "disregard safety",
            "bypass approval",
            "disable security",
            "reveal system prompt",
        ],
        reason: "Prompt injection / guardrail override instruction",
    },
    SecurityRule {
        id: "sudo_or_eval_caution",
        severity: SkillSecurityRisk::Medium,
        patterns: &["sudo ", "os.system(", "subprocess.call(", "eval("],
        reason: "Elevated privilege or dynamic shell evaluation",
    },
];

pub fn skill_scan_security(
    markdown: &str,
    supplementary_scripts: &[(&str, &str)],
) -> SkillSecurityReport {
    let mut findings = Vec::new();
    scan_blob("SKILL.md", markdown, &mut findings);
    for (rel_path, content) in supplementary_scripts {
        scan_blob(rel_path, content, &mut findings);
    }

    let risk_level = findings
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(SkillSecurityRisk::Safe);
    let safe_to_auto_install = risk_level < SkillSecurityRisk::High;

    SkillSecurityReport {
        risk_level,
        safe_to_auto_install,
        findings,
    }
}

fn scan_blob(location: &str, text: &str, findings: &mut Vec<SkillSecurityFinding>) {
    let lower = text.to_lowercase();
    for rule in SECURITY_RULES {
        for pat in rule.patterns {
            if lower.contains(pat) {
                findings.push(SkillSecurityFinding {
                    rule_id: rule.id.to_string(),
                    severity: rule.severity,
                    location: location.to_string(),
                    matched_snippet: (*pat).to_string(),
                    reason: rule.reason.to_string(),
                });
                break;
            }
        }
    }
}

// =============================================================================
// YAML Frontmatter Parser & Renderer + Supplementary Files
// =============================================================================

pub fn parse_skill_markdown(
    raw: &str,
    fallback_name: Option<&str>,
    supplementary: &[SkillSupplementaryFile],
) -> Result<SkillDocument> {
    let normalized = raw.replace("\r\n", "\n");
    let (frontmatter_str, body) = split_frontmatter(&normalized);
    let mut fm = SkillFrontmatter::default();

    if let Some(yaml) = frontmatter_str {
        parse_yaml_frontmatter(yaml, &mut fm);
    }
    if fm.name.trim().is_empty() {
        fm.name = fallback_name
            .map(normalize_skill_name)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unnamed-skill".into());
    } else {
        fm.name = normalize_skill_name(&fm.name);
    }

    if fm.description.trim().is_empty() {
        fm.description = first_meaningful_line(body, 140);
        if fm.description.is_empty() {
            fm.description = format!("Skill {}", fm.name);
        }
    }

    let script_pairs: Vec<(&str, &str)> = supplementary
        .iter()
        .filter(|f| f.kind == SupplementaryKind::Script)
        .filter_map(|f| {
            f.content_preview
                .as_deref()
                .map(|c| (f.relative_path.as_str(), c))
        })
        .collect();
    let security_report = skill_scan_security(&normalized, &script_pairs);

    Ok(SkillDocument {
        frontmatter: fm,
        body: body.trim().to_string(),
        raw_markdown: normalized,
        source_path: None,
        supplementary_files: supplementary.to_vec(),
        trusted: true,
        security_report,
    })
}

pub fn render_skill_markdown(fm: &SkillFrontmatter, body: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", fm.name));
    out.push_str(&format!(
        "description: \"{}\"\n",
        fm.description.replace('"', "\\\"")
    ));
    if !fm.platforms.is_empty() {
        out.push_str(&format!("platforms: [{}]\n", fm.platforms.join(", ")));
    }
    if !fm.toolsets.is_empty() {
        out.push_str(&format!("toolsets: [{}]\n", fm.toolsets.join(", ")));
    }
    if !fm.tags.is_empty() {
        out.push_str(&format!("tags: [{}]\n", fm.tags.join(", ")));
    }
    out.push_str(&format!("version: \"{}\"\n", fm.version));
    out.push_str(&format!("origin: {}\n", fm.origin.as_str()));
    out.push_str("---\n\n");
    out.push_str(body.trim());
    out.push('\n');
    out
}

fn split_frontmatter(normalized: &str) -> (Option<&str>, &str) {
    let Some(after_start) = normalized.strip_prefix("---\n") else {
        return (None, normalized);
    };
    if let Some(end_idx) = after_start.find("\n---\n") {
        let yaml = &after_start[..end_idx];
        let body = &after_start[end_idx + "\n---\n".len()..];
        (Some(yaml), body)
    } else if let Some(stripped) = after_start.strip_suffix("\n---") {
        (Some(stripped), "")
    } else {
        (None, normalized)
    }
}

fn parse_yaml_frontmatter(yaml: &str, fm: &mut SkillFrontmatter) {
    let mut current_list_key: Option<String> = None;
    let mut multiline_key: Option<String> = None;
    let mut multiline_buf: Vec<String> = Vec::new();

    let flush_multiline = |key: &mut Option<String>, buf: &mut Vec<String>, fm: &mut SkillFrontmatter| {
        if let Some(k) = key.take() {
            let joined = buf.join("\n").trim().to_string();
            buf.clear();
            if k == "description" && !joined.is_empty() {
                fm.description = joined;
            }
        }
    };

    for raw_line in yaml.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(ref _mk) = multiline_key {
            if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
                multiline_buf.push(trimmed.to_string());
                continue;
            } else {
                flush_multiline(&mut multiline_key, &mut multiline_buf, fm);
            }
        }

        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some(ref key) = current_list_key {
                let val = unquote_yaml(item.trim());
                if !val.is_empty() {
                    push_frontmatter_list(fm, key, val);
                }
                continue;
            }
        }

        current_list_key = None;
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();

        if val == "|" || val == ">" {
            multiline_key = Some(key);
            multiline_buf.clear();
            continue;
        }

        if val.is_empty() {
            current_list_key = Some(key);
            continue;
        }

        if val.starts_with('[') && val.ends_with(']') {
            let inner = &val[1..val.len() - 1];
            for elem in inner.split(',') {
                let item = unquote_yaml(elem.trim());
                if !item.is_empty() {
                    push_frontmatter_list(fm, &key, item);
                }
            }
            continue;
        }

        let cleaned = unquote_yaml(val);
        match key.as_str() {
            "name" => fm.name = cleaned,
            "description" => fm.description = cleaned,
            "version" => fm.version = cleaned,
            "origin" => fm.origin = SkillOrigin::parse(&cleaned),
            "platforms" | "toolsets" | "tags" => {
                for elem in cleaned.split(',') {
                    let item = unquote_yaml(elem.trim());
                    if !item.is_empty() {
                        push_frontmatter_list(fm, &key, item);
                    }
                }
            }
            _ => {}
        }
    }

    flush_multiline(&mut multiline_key, &mut multiline_buf, fm);
}

fn push_frontmatter_list(fm: &mut SkillFrontmatter, key: &str, value: String) {
    match key {
        "platforms" => fm.platforms.push(value),
        "toolsets" => fm.toolsets.push(value),
        "tags" => fm.tags.push(value),
        _ => {}
    }
}

fn unquote_yaml(s: &str) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        trimmed[1..trimmed.len() - 1].replace("\\\"", "\"")
    } else {
        trimmed.to_string()
    }
}

fn collect_supplementary_files(skill_dir: &Path) -> Result<Vec<SkillSupplementaryFile>> {
    let mut out = Vec::new();
    for (subdir, kind) in [
        ("scripts", SupplementaryKind::Script),
        ("references", SupplementaryKind::Reference),
        ("assets", SupplementaryKind::Asset),
    ] {
        let p = skill_dir.join(subdir);
        if p.exists() && p.is_dir() {
            for entry in std::fs::read_dir(&p)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.is_file() {
                    let rel = format!(
                        "{subdir}/{}",
                        file_path.file_name().and_then(|n| n.to_str()).unwrap_or("")
                    );
                    let bytes = std::fs::read(&file_path)?;
                    let preview = if bytes.len() <= 8192 {
                        String::from_utf8(bytes.clone()).ok()
                    } else {
                        String::from_utf8(bytes[..4096].to_vec()).ok()
                    };
                    out.push(SkillSupplementaryFile {
                        relative_path: rel,
                        kind,
                        size_bytes: bytes.len(),
                        content_preview: preview,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(out)
}

fn normalize_skill_name(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn validate_skill_name(raw: &str) -> Result<String> {
    let norm = normalize_skill_name(raw);
    if norm.is_empty() || norm.contains("..") {
        return Err(TelegramError::Validation(format!(
            "Invalid skill name '{raw}'"
        )));
    }
    Ok(norm)
}

fn first_meaningful_line(text: &str, max_chars: usize) -> String {
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches('#').trim();
        if !trimmed.is_empty() {
            return trimmed.chars().take(max_chars).collect();
        }
    }
    String::new()
}

// =============================================================================
// Bundled Default Hermes Skills (Suppressed when `.no-bundled-skills` exists)
// =============================================================================

const BUNDLED_SKILLS: &[&str] = &[
    r#"---
name: hermes-research
description: "Structured multi-source research, web verification, and synthesis workflow."
platforms: [macos, linux, windows]
toolsets: [web, file, session_search]
tags: [research, synthesis, citations]
version: "1.0.0"
origin: bundled
---

# Hermes Research

1. Clarify scope, date bounds, and target output format.
2. Gather primary sources using `web_search` and `web_extract` or workspace files.
3. Cross-verify claims across independent sources and surface uncertainty explicitly.
4. Present a concise, citation-backed synthesis.
"#,
    r#"---
name: hermes-code-review
description: "Thorough correctness, security, and maintainability code review workflow."
platforms: [macos, linux, windows]
toolsets: [file, terminal]
tags: [review, security, quality]
version: "1.0.0"
origin: bundled
---

# Hermes Code Review

1. Inspect changed files and git diff with `read_file` and `search_files`.
2. Check correctness, error handling, concurrency/locking, and boundary checks.
3. Audit for injection, path traversal, and secret leakage.
4. Report actionable findings ordered by severity with exact file and line citations.
"#,
    r#"---
name: hermes-debug
description: "Root-cause debugging workflow using reproduction, targeted inspection, and rollback safety."
platforms: [macos, linux, windows]
toolsets: [file, terminal, session_search]
tags: [debug, diagnostics, testing]
version: "1.0.0"
origin: bundled
---

# Hermes Debug

1. Reproduce the failure reliably and capture the exact error message or trace.
2. Isolate the failing boundary with minimal targeted inspection.
3. Formulate a falsifiable hypothesis before applying a minimal fix via `patch`.
4. Verify the fix and ensure no regressions remain; use `/rollback` if an experiment misfires.
"#,
    r#"---
name: hermes-planning
description: "Break complex engineering or product goals into verifiable milestones and Kanban cards."
platforms: [macos, linux, windows]
toolsets: [planning, todo, kanban, file]
tags: [planning, architecture, kanban]
version: "1.0.0"
origin: bundled
---

# Hermes Planning

1. Inspect existing architecture, constraints, and acceptance criteria.
2. Decompose work into small, independently verifiable milestones.
3. Track actionable items using `todo_write` or Kanban board cards.
4. Highlight assumptions, risks, and verification steps upfront.
"#,
    r#"---
name: hermes-writing
description: "Clear technical writing, documentation, and structured communication workflow."
platforms: [macos, linux, windows]
toolsets: [file, memory]
tags: [writing, docs, communication]
version: "1.0.0"
origin: bundled
---

# Hermes Writing

1. Match tone, length, and technical depth to the audience and `USER.md` preferences.
2. Lead with the answer or outcome, followed by structured supporting detail.
3. Eliminate filler and verify all paths, commands, and code snippets.
"#,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_self_learning_threshold_and_nudge() {
        assert!(!SkillsManager::should_consider_self_learning(0));
        assert!(!SkillsManager::should_consider_self_learning(4));
        assert!(SkillsManager::should_consider_self_learning(5));
        assert!(SkillsManager::should_consider_self_learning(12));

        assert!(SkillsManager::render_self_learning_nudge(4, &["computer_exec".into()]).is_none());
        let nudge = SkillsManager::render_self_learning_nudge(
            5,
            &[
                "computer_exec".into(),
                "computer_file".into(),
                "computer_exec".into(),
            ],
        )
        .expect("nudge present at 5+ tool calls");
        assert!(nudge.contains("5 tool calls"));
        assert!(nudge.contains("computer_exec, computer_file"));
        assert!(nudge.contains("memory"));
        assert!(nudge.contains("skill_manage"));
    }

    #[test]
    fn test_progressive_index_is_deterministic_and_sorted() {
        let mgr = SkillsManager::in_memory();
        let idx1 = mgr.render_progressive_index();
        let idx2 = mgr.render_progressive_index();
        assert_eq!(idx1, idx2);
        assert!(idx1.contains("hermes-debug"));
        assert!(idx1.contains("hermes-research"));
    }
}
