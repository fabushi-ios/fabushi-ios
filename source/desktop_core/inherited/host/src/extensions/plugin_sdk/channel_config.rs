use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelConfigUiHint {
    pub label: String,
    pub help: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelConfigUiHintsParams {
    pub channel_label: String,
    pub dm_policy: Option<DmPolicyHintParams>,
    pub config_writes: bool,
    pub mention_patterns: Option<MentionPatternHintParams>,
    pub native_commands: bool,
    pub implicit_mentions: bool,
    pub progress: Option<ProgressHintParams>,
    pub streaming: Option<HashMap<String, String>>,
    pub retry: bool,
}

#[derive(Debug, Clone)]
pub struct DmPolicyHintParams {
    pub channel_key: String,
    pub include_legacy_nested_policy: bool,
    pub legacy_nested_policy_order: Option<String>, // "before" | "after"
}

#[derive(Debug, Clone)]
pub struct MentionPatternHintParams {
    pub target_description: String,
    pub policy_target_description: Option<String>,
    pub policy_note: Option<String>,
    pub deny_note: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProgressHintParams {
    pub include_commentary: bool,
    pub commentary_order: Option<String>, // "before-command" | "after-command"
    pub labels: Option<String>,           // "openclaw"
    pub title_wording: bool,
}

fn streaming_label_lookup<'a>(k: &'a str) -> &'a str {
    match k {
        "" | "mode" => "Streaming Mode",
        "chunkMode" => "Chunk Mode",
        "block.enabled" => "Block Streaming Enabled",
        "block.coalesce" => "Block Streaming Coalesce",
        "nativeTransport" => "Native Streaming",
        "preview.chunk.minChars" => "Draft Chunk Min Chars",
        "preview.chunk.maxChars" => "Draft Chunk Max Chars",
        "preview.chunk.breakPreference" => "Draft Chunk Break Preference",
        "preview.toolProgress" => "Draft Tool Progress",
        "preview.commandText" => "Draft Command Text",
        "progress.style" => "Progress Style",
        "progress.nativeTaskCards" => "Native Progress Task Cards",
        _ => k,
    }
}

/// Create UI hints map for channel configuration schema fields.
pub fn create_channel_config_ui_hints(
    params: &ChannelConfigUiHintsParams,
) -> BTreeMap<String, ChannelConfigUiHint> {
    let mut hints = BTreeMap::new();
    let channel_label = &params.channel_label;

    if let Some(dm) = &params.dm_policy {
        let help_base = format!(
            "Direct message access control (\"pairing\" recommended). \"open\" requires channels.{}.allowFrom=[\"*\"].",
            dm.channel_key
        );
        let hint = ChannelConfigUiHint {
            label: format!("{channel_label} DM Policy"),
            help: help_base.clone(),
        };

        let legacy_hint = ChannelConfigUiHint {
            label: hint.label.clone(),
            help: format!(
                "{} (legacy: channels.{}.dm.allowFrom).",
                &help_base[..help_base.len() - 1],
                dm.channel_key
            ),
        };

        if !dm.include_legacy_nested_policy {
            hints.insert("dmPolicy".to_string(), hint);
        } else if dm.legacy_nested_policy_order.as_deref() == Some("after") {
            hints.insert("dmPolicy".to_string(), hint);
            hints.insert("dm.policy".to_string(), legacy_hint);
        } else {
            hints.insert("dm.policy".to_string(), legacy_hint);
            hints.insert("dmPolicy".to_string(), hint);
        }
    }

    if params.config_writes {
        hints.insert(
            "configWrites".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Config Writes"),
                help: format!(
                    "Allow {channel_label} to write config in response to channel events/commands (default: true)."
                ),
            },
        );
    }

    if let Some(mp) = &params.mention_patterns {
        let policy_suffix = mp.policy_note.as_ref().map(|n| format!(" {n}")).unwrap_or_default();
        let deny_suffix = mp.deny_note.as_ref().map(|n| format!(" {n}")).unwrap_or_default();
        let target_desc = mp.policy_target_description.as_ref().unwrap_or(&mp.target_description);

        hints.insert(
            "mentionPatterns".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Mention Pattern Policy"),
                help: format!("Scopes configured groupChat mentionPatterns to selected {target_desc}.{policy_suffix}"),
            },
        );
        hints.insert(
            "mentionPatterns.mode".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Mention Pattern Mode"),
                help: "\"allow\" enables configured regex mention patterns unless denyIn matches; \"deny\" disables them unless allowIn matches.".to_string(),
            },
        );
        hints.insert(
            "mentionPatterns.allowIn".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Mention Pattern Allowlist"),
                help: format!("{} where configured regex mention patterns are enabled when mode is deny.", mp.target_description),
            },
        );
        hints.insert(
            "mentionPatterns.denyIn".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Mention Pattern Denylist"),
                help: format!("{} where configured regex mention patterns are disabled.{deny_suffix}", mp.target_description),
            },
        );
    }

    if params.native_commands {
        hints.insert(
            "commands.native".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Native Commands"),
                help: format!("Override native commands for {channel_label} (bool or \"auto\")."),
            },
        );
        hints.insert(
            "commands.nativeSkills".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Native Skill Commands"),
                help: format!("Override native skill commands for {channel_label} (bool or \"auto\")."),
            },
        );
    }

    if params.implicit_mentions {
        hints.insert(
            "implicitMentions".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Implicit Mentions"),
                help: format!("Control which {channel_label} reply, quote, and thread-participation signals count as mentions. Unset flags preserve the channel defaults."),
            },
        );
        hints.insert(
            "implicitMentions.replyToBot".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Replies to Bot"),
                help: "Treat replies to the bot's own messages as implicit mentions when the channel reports that signal.".to_string(),
            },
        );
        hints.insert(
            "implicitMentions.quotedBot".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Quoted Bot Messages"),
                help: "Treat messages quoting the bot as implicit mentions when the channel reports that signal.".to_string(),
            },
        );
        hints.insert(
            "implicitMentions.threadParticipation".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Thread Participation"),
                help: "Treat follow-ups in threads where the bot participated as implicit mentions when the channel reports that signal.".to_string(),
            },
        );
    }

    if let Some(prog) = &params.progress {
        let title_word = if prog.title_wording { "title" } else { "draft title" };
        let label_word = if prog.title_wording { "progress title" } else { "draft label" };
        let cmd_word = if prog.title_wording { "" } else { " draft" };

        hints.insert(
            "streaming.progress.label".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Progress Label"),
                help: format!("Initial progress {title_word}. Use \"auto\" for built-in single-word labels, a custom string, or false to hide the title."),
            },
        );
        hints.insert(
            "streaming.progress.labels".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Progress Label Pool"),
                help: if prog.labels.as_deref() == Some("openclaw") {
                    "Custom single-word labels cycled during long operations (defaults to OpenClaw's rotating pool).".to_string()
                } else {
                    "Custom single-word labels cycled during long operations.".to_string()
                },
            },
        );
        hints.insert(
            "streaming.progress.maxLines".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Progress Max Lines"),
                help: format!("Maximum number of compact progress lines to keep below the {label_word} (default: 8)."),
            },
        );
        hints.insert(
            "streaming.progress.maxLineChars".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Progress Max Line Chars"),
                help: "Maximum characters per compact progress line before truncation (default: 120). Prose cuts at word boundaries; commands and paths keep useful suffixes.".to_string(),
            },
        );
        hints.insert(
            "streaming.progress.toolProgress".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Progress Tool Lines"),
                help: "Show individual tool activity, including intermediate failures, in progress drafts (default: false). Quiet drafts retain plans, approval requests, and authored progress text. Terminal task errors remain visible.".to_string(),
            },
        );
        if prog.include_commentary && prog.commentary_order.as_deref() != Some("after-command") {
            hints.insert(
                "streaming.progress.commentary".to_string(),
                ChannelConfigUiHint {
                    label: format!("{channel_label} Progress Commentary"),
                    help: "Show assistant commentary/preamble text in the temporary progress draft. Final answer delivery is unchanged.".to_string(),
                },
            );
        }
        hints.insert(
            "streaming.progress.commandText".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Progress Command Text"),
                help: format!("Command/exec detail in progress{cmd_word} lines: \"status\" is the safe default; \"raw\" opts into command text."),
            },
        );
        if prog.include_commentary && prog.commentary_order.as_deref() == Some("after-command") {
            hints.insert(
                "streaming.progress.commentary".to_string(),
                ChannelConfigUiHint {
                    label: format!("{channel_label} Progress Commentary"),
                    help: "Show assistant commentary/preamble text in the temporary progress draft. Final answer delivery is unchanged.".to_string(),
                },
            );
        }
    }

    if let Some(st) = &params.streaming {
        for (k, v) in st {
            let full_key = if k.is_empty() { "streaming".to_string() } else { format!("streaming.{k}") };
            hints.insert(
                full_key,
                ChannelConfigUiHint {
                    label: format!("{channel_label} {}", streaming_label_lookup(k)),
                    help: v.clone(),
                },
            );
        }
    }

    if params.retry {
        hints.insert(
            "retry.attempts".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Retry Attempts"),
                help: format!("Max retry attempts for outbound {channel_label} API calls (default: 3)."),
            },
        );
        hints.insert(
            "retry.minDelayMs".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Retry Min Delay (ms)"),
                help: format!("Minimum retry delay in ms for {channel_label} outbound calls."),
            },
        );
        hints.insert(
            "retry.maxDelayMs".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Retry Max Delay (ms)"),
                help: format!("Maximum retry delay cap in ms for {channel_label} outbound calls."),
            },
        );
        hints.insert(
            "retry.jitter".to_string(),
            ChannelConfigUiHint {
                label: format!("{channel_label} Retry Jitter"),
                help: format!("Jitter factor (0-1) applied to {channel_label} retry delays."),
            },
        );
    }

    hints
}

/// Map allow from array entries to strings.
pub fn map_allow_from_entries(allow_from: Option<&[Value]>) -> Vec<String> {
    allow_from
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| match e {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Normalize user-facing allowlist entries by trimming and deduplicating.
pub fn format_trimmed_allow_from_entries(entries: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for e in entries {
        let trimmed = e.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// Collapse nullable config scalars into a trimmed optional string.
pub fn resolve_optional_config_string(value: Option<&str>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Check if config writes are enabled for a target channel and account in the configuration.
pub fn resolve_channel_config_writes(
    cfg: &Value,
    channel_id: Option<&str>,
    account_id: Option<&str>,
) -> bool {
    let channels = match cfg.get("channels") {
        Some(c) => c,
        None => return true,
    };

    let ch_id = match channel_id {
        Some(id) => id,
        None => return true,
    };

    let channel_cfg = match channels.get(ch_id) {
        Some(c) => c,
        None => return true,
    };

    // Check account-level override first
    if let Some(acc) = account_id {
        if let Some(accounts) = channel_cfg.get("accounts") {
            if let Some(account_cfg) = accounts.get(acc) {
                if let Some(cw) = account_cfg.get("configWrites").and_then(|v| v.as_bool()) {
                    return cw;
                }
            }
        }
    }

    // Check channel-level setting
    if let Some(cw) = channel_cfg.get("configWrites").and_then(|v| v.as_bool()) {
        return cw;
    }

    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDmAccessResult {
    pub allow_from: Vec<String>,
    pub dm_policy: Option<String>,
}

/// Resolve channel direct-message policy and allowlist from account or fallback parent.
pub fn resolve_channel_dm_access(
    account: &Value,
    parent: Option<&Value>,
) -> ChannelDmAccessResult {
    let account_dm = account.get("dm");
    let account_dm_allow_from = account_dm
        .and_then(|dm| dm.get("allowFrom"))
        .and_then(|af| af.as_array());

    let allow_from = if let Some(af) = account_dm_allow_from {
        map_allow_from_entries(Some(af))
    } else if let Some(af) = account.get("allowFrom").and_then(|v| v.as_array()) {
        map_allow_from_entries(Some(af))
    } else if let Some(p) = parent {
        if let Some(af) = p.get("allowFrom").and_then(|v| v.as_array()) {
            map_allow_from_entries(Some(af))
        } else if let Some(af) = p.get("dm").and_then(|dm| dm.get("allowFrom")).and_then(|v| v.as_array()) {
            map_allow_from_entries(Some(af))
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let dm_policy = account
        .get("dmPolicy")
        .and_then(|v| v.as_str())
        .or_else(|| account_dm.and_then(|dm| dm.get("policy")).and_then(|v| v.as_str()))
        .or_else(|| parent.and_then(|p| p.get("dmPolicy")).and_then(|v| v.as_str()))
        .or_else(|| parent.and_then(|p| p.get("dm")).and_then(|dm| dm.get("policy")).and_then(|v| v.as_str()))
        .map(String::from);

    ChannelDmAccessResult {
        allow_from,
        dm_policy,
    }
}

/// Normalizes DM policy to canonical values: "open" | "pairing" | "allowlist" | "disabled".
pub fn normalize_channel_dm_policy(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "open" => "open",
        "pairing" => "pairing",
        "allowlist" | "whitelist" => "allowlist",
        "disabled" | "deny" | "off" => "disabled",
        _ => "pairing",
    }
}

/// Set canonical DM allowlist into an entry, moving from legacy nested dm.allowFrom.
pub fn set_canonical_dm_allow_from(
    entry: &mut serde_json::Map<String, Value>,
    allow_from: Vec<String>,
    path_prefix: &str,
    changes: &mut Vec<String>,
    reason: &str,
) {
    if let Some(dm) = entry.get_mut("dm").and_then(|v| v.as_object_mut()) {
        if dm.remove("allowFrom").is_some() {
            changes.push(format!(
                "- {path_prefix}.dm.allowFrom: removed after moving allowlist to {path_prefix}.allowFrom"
            ));
        }
    }

    entry.insert(
        "allowFrom".to_string(),
        Value::Array(allow_from.into_iter().map(Value::String).collect()),
    );
    changes.push(format!("- {path_prefix}.allowFrom: {reason}"));
}

/// When DM policy is "open", ensures allowFrom contains wildcard "*".
pub fn ensure_open_dm_policy_allow_from_wildcard(
    entry: &mut serde_json::Map<String, Value>,
    path_prefix: &str,
    changes: &mut Vec<String>,
) -> bool {
    let policy = entry
        .get("dmPolicy")
        .and_then(|v| v.as_str())
        .or_else(|| entry.get("dm").and_then(|dm| dm.get("policy")).and_then(|v| v.as_str()));

    if policy != Some("open") {
        return false;
    }

    let mut current = match entry.get("allowFrom").and_then(|v| v.as_array()) {
        Some(arr) => map_allow_from_entries(Some(arr)),
        None => Vec::new(),
    };

    if !current.iter().any(|s| s == "*") {
        current.push("*".to_string());
        entry.insert(
            "allowFrom".to_string(),
            Value::Array(current.into_iter().map(Value::String).collect()),
        );
        changes.push(format!("- {path_prefix}.allowFrom: added wildcard for open DM policy"));
        return true;
    }

    false
}
