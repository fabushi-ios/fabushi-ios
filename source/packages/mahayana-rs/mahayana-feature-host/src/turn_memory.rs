use chrono::TimeZone;
use mahayana_host_protocol::MemoryKind;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const MEMORY_EXTRACTION_PROMPT_MARKER: &str = "<<SAND_MEMORY_EXTRACTION>>";
pub(crate) const MEMORY_EPISODE_PROMPT_MARKER: &str = "<<SAND_MEMORY_EPISODE>>";
pub(crate) const MEMORY_EPISODE_PREFIX: &str = "[episode] ";
pub(crate) const MEMORY_NOTE_PREFIX: &str = "[note] ";
pub(crate) const MEMORY_EXTRACTION_NONE_SENTINEL: &str = "NONE";
pub(crate) const DEFAULT_EPISODE_INTERVAL: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnExchange {
    pub user: String,
    pub agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EpisodeTurn {
    pub ts: i64,
    pub user: String,
    pub agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryAddition {
    pub content: String,
    pub kind: MemoryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MemoryExtraction {
    pub additions: Vec<MemoryAddition>,
    pub removals: Vec<String>,
}

pub(crate) fn build_turn_memory_exchange<I, S>(
    user: impl Into<String>,
    agent_messages: I,
    final_agent: impl Into<String>,
) -> TurnExchange
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let user = user.into();
    let mut agent = agent_messages
        .into_iter()
        .map(|message| message.as_ref().trim().to_string())
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>();
    let final_agent = final_agent.into();
    if !final_agent.trim().is_empty() {
        agent.push(final_agent.trim().to_string());
    }
    TurnExchange {
        user: user.trim().to_string(),
        agent: agent.join("\n"),
    }
}

pub(crate) fn episode_interval(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_EPISODE_INTERVAL)
}

pub(crate) fn is_memorable_exchange(user_message: &str) -> bool {
    let user = user_message.trim();
    if user.is_empty() {
        return false;
    }
    if user.chars().count() > 40 || user.contains('?') {
        return true;
    }
    let normalized = user
        .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, '!' | '.' | '…' | ',' | '~' | ')' | ']'))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    !matches!(
        normalized.as_str(),
        "hi" | "hey" | "hello" | "yo" | "sup" | "thanks" | "thank you" | "ty" | "thx"
            | "ok" | "okay" | "k" | "kk" | "cool" | "nice" | "great" | "awesome"
            | "perfect" | "yes" | "yep" | "yeah" | "no" | "nope" | "sure" | "got it"
            | "gotcha" | "lol" | "haha" | "np" | "done" | "good" | "bye"
    )
}

fn pending_episode_path(memory_dir: &Path) -> PathBuf {
    memory_dir.join(".dreaming").join("pending-episode-turns.json")
}

pub(crate) fn load_pending_episode_turns(memory_dir: &Path) -> Vec<EpisodeTurn> {
    fs::read(pending_episode_path(memory_dir))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Vec<EpisodeTurn>>(&raw).ok())
        .unwrap_or_default()
}

pub(crate) fn record_pending_episode_turn(
    memory_dir: &Path,
    turn: &EpisodeTurn,
) -> io::Result<()> {
    let path = pending_episode_path(memory_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut turns = load_pending_episode_turns(memory_dir);
    turns.push(turn.clone());
    let raw = serde_json::to_vec(&turns).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw)?;
    fs::rename(tmp, path)
}

pub(crate) fn clear_pending_episode_turns(memory_dir: &Path) -> io::Result<()> {
    let path = pending_episode_path(memory_dir);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn build_extraction_system_prompt() -> String {
    [
        MEMORY_EXTRACTION_PROMPT_MARKER,
        "You maintain the long-term memory of a personal assistant. Read the latest exchange and decide what — if anything — is worth remembering for future, unrelated conversations.",
        "",
        "Tag each fact you keep with a category:",
        "- \"profile\": enduring facts about who the user is and how to work with them.",
        "- \"log\": substantive history worth keeping — ongoing projects, decisions, commitments, and time-bound details.",
        "- \"note\": minor, low-stakes details that might help someday.",
        "",
        "Do NOT record one-off request mechanics, what the assistant did this turn, general knowledge, or anything already present in the existing memory list.",
        "For a superseded fact, output \"remove: <the exact existing fact text>\" and then the corrected fact.",
        "Write one change per line as profile:, log:, note:, or remove:.",
        "Output exactly NONE when there is nothing to add or remove.",
    ].join("\n")
}

pub(crate) fn build_extraction_user_prompt(
    exchange: &TurnExchange,
    existing_memories: &[String],
) -> String {
    let existing = if existing_memories.is_empty() {
        "(empty)".to_string()
    } else {
        existing_memories.iter().map(|memory| format!("- {memory}")).collect::<Vec<_>>().join("\n")
    };
    format!(
        "Existing memory:\n{existing}\n\nLatest exchange:\nUser: {}\nAssistant: {}",
        non_empty_or(exchange.user.trim(), "(no message)"),
        non_empty_or(exchange.agent.trim(), "(no message)")
    )
}

pub(crate) fn parse_extracted_memories(raw: &str, existing_memories: &[String]) -> MemoryExtraction {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(MEMORY_EXTRACTION_NONE_SENTINEL) {
        return MemoryExtraction::default();
    }
    let mut seen = existing_memories.iter().map(|memory| dedupe_key(memory)).collect::<HashSet<_>>();
    let known = existing_memories.iter().map(|memory| dedupe_key(memory)).collect::<HashSet<_>>();
    let mut output = MemoryExtraction::default();
    for line in trimmed.lines() {
        let line = strip_list_prefix(line.trim());
        let (tag, value) = split_category(line);
        let bare = normalize(value);
        if bare.is_empty() || bare.eq_ignore_ascii_case(MEMORY_EXTRACTION_NONE_SENTINEL) {
            continue;
        }
        if tag == Some("remove") {
            if known.contains(&dedupe_key(&bare)) && !output.removals.iter().any(|value| dedupe_key(value) == dedupe_key(&bare)) {
                output.removals.push(bare);
            }
            continue;
        }
        let content = if tag == Some("note") {
            normalize(&format!("{MEMORY_NOTE_PREFIX}{bare}"))
        } else {
            bare
        };
        if !seen.insert(dedupe_key(&content)) {
            continue;
        }
        output.additions.push(MemoryAddition {
            content,
            kind: if tag == Some("profile") { MemoryKind::Profile } else { MemoryKind::Log },
        });
    }
    output
}

pub(crate) fn build_episode_system_prompt() -> String {
    [
        MEMORY_EPISODE_PROMPT_MARKER,
        "You maintain the long-term memory of a personal assistant named Fabushi.",
        "Write ONE short journal-style sentence (two at most) capturing the throughline, key decisions, and outcomes across the supplied turns.",
        "Anchor time references with the absolute dates shown. Drop greetings and ephemeral details. Never invent details.",
        "Output exactly NONE if nothing is worth remembering.",
    ].join("\n")
}

pub(crate) fn build_episode_user_prompt(turns: &[EpisodeTurn]) -> String {
    let rendered = turns.iter().map(|turn| {
        let date = chrono::Utc.timestamp_millis_opt(turn.ts).single()
            .map(|value| value.date_naive().to_string())
            .unwrap_or_else(|| "1970-01-01".to_string());
        let mut lines = vec![format!("({date})")];
        if !turn.user.trim().is_empty() { lines.push(format!("User: {}", turn.user.trim())); }
        if !turn.agent.trim().is_empty() { lines.push(format!("Fabushi: {}", turn.agent.trim())); }
        lines.join("\n")
    }).collect::<Vec<_>>().join("\n\n");
    format!("Recent turns, oldest first:\n\n{rendered}")
}

pub(crate) fn normalize_episode_summary(raw: &str) -> Option<String> {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized.eq_ignore_ascii_case(MEMORY_EXTRACTION_NONE_SENTINEL) {
        None
    } else {
        Some(format!("{MEMORY_EPISODE_PREFIX}{normalized}"))
    }
}

fn normalize(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(500).collect()
}
fn dedupe_key(raw: &str) -> String { normalize(raw).to_lowercase() }
fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str { if value.is_empty() { fallback } else { value } }
fn split_category(line: &str) -> (Option<&'static str>, &str) {
    for tag in ["profile", "log", "note", "remove"] {
        if line.len() > tag.len()
            && line.get(..tag.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(tag))
            && line.as_bytes().get(tag.len()) == Some(&b':')
        {
            return (Some(tag), line.get(tag.len()+1..).unwrap_or_default().trim());
        }
    }
    (None, line)
}
fn strip_list_prefix(line: &str) -> &str {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")).or_else(|| line.strip_prefix("• ")) {
        return rest.trim_start();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchange_preserves_agent_messages_before_final_and_omits_blanks() {
        let exchange = build_turn_memory_exchange(
            " user ",
            ["peer one", " ", "peer two"],
            " final ",
        );
        assert_eq!(exchange.user, "user");
        assert_eq!(exchange.agent, "peer one\npeer two\nfinal");
        let tool_only = build_turn_memory_exchange("request", ["sent to peer"], "");
        assert_eq!(tool_only.agent, "sent to peer");
    }

    #[test]
    fn evidence_mode_cleanup_removes_pending_episode_state() {
        let root = std::env::temp_dir().join(format!("fabushi-turn-memory-{}-{}", std::process::id(), crate::implementation::now_millis()));
        let memory_dir = root.join("memory");
        let turn = EpisodeTurn { ts: 1, user: "u".into(), agent: "a".into() };
        record_pending_episode_turn(&memory_dir, &turn).expect("record");
        assert_eq!(load_pending_episode_turns(&memory_dir), vec![turn]);
        clear_pending_episode_turns(&memory_dir).expect("clear");
        assert!(load_pending_episode_turns(&memory_dir).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interval_memorable_parser_and_episode_fallback_match_contract() {
        assert_eq!(episode_interval(None), 6);
        assert_eq!(episode_interval(Some("2")), 2);
        assert_eq!(episode_interval(Some("0")), 6);
        assert!(!is_memorable_exchange("thanks"));
        assert!(is_memorable_exchange("What should we do next?"));

        let existing = vec!["Lives in Paris".to_string()];
        let parsed = parse_extracted_memories(
            "remove: Lives in Paris\nprofile: Lives in Lyon\nnote: likes window seats",
            &existing,
        );
        assert_eq!(parsed.removals, vec!["Lives in Paris"]);
        assert_eq!(parsed.additions.len(), 2);
        assert_eq!(normalize_episode_summary("  NONE  "), None);
        assert_eq!(
            normalize_episode_summary(" shipped   the migration "),
            Some("[episode] shipped the migration".into())
        );
    }

    #[test]
    fn prompts_are_model_only_contracts() {
        assert!(build_extraction_system_prompt().contains(MEMORY_EXTRACTION_PROMPT_MARKER));
        assert!(build_episode_system_prompt().contains(MEMORY_EPISODE_PROMPT_MARKER));
        let exchange = TurnExchange { user: "u".into(), agent: "a".into() };
        assert!(build_extraction_user_prompt(&exchange, &[]).contains("Latest exchange"));
        assert!(build_episode_user_prompt(&[EpisodeTurn { ts: 0, user: "u".into(), agent: "a".into() }]).contains("1970-01-01"));
    }
}
