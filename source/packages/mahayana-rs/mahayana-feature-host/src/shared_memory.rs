use chrono::NaiveDate;
use mahayana_host_protocol::{MemoryKind, MemoryProjectSummary, MemoryRecord, MemoryScope};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const PROFILE_LIMIT: usize = 100;
const RECENT_LIMIT: usize = 30;
const RECENT_CHAR_BUDGET: usize = 4_000;
const PROJECT_INJECTED_LIMIT: usize = 3;
const MEMORY_DECAY_HALF_LIFE_DAYS: f64 = 30.0;

#[derive(Clone)]
struct ScopedFact {
    agent_name: String,
    memory: MemoryRecord,
    order: usize,
}

pub(crate) fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn normalize(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

fn memory_date_ms(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

fn memory_id(content: &str) -> String {
    let digest = Sha256::digest(content.to_lowercase().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()[..16].to_string()
}

fn parse_fact(line: &str, kind: MemoryKind, order: usize) -> Option<(MemoryRecord, usize)> {
    let rest = line.trim().strip_prefix("- (")?;
    let (date, content) = rest.split_once(") ")?;
    let created_at = memory_date_ms(date)?;
    let content = normalize(content);
    if content.is_empty() {
        return None;
    }
    Some((
        MemoryRecord {
            id: memory_id(&content),
            content,
            created_at,
            kind,
        },
        order,
    ))
}

fn read_facts(memory_dir: &Path, agent_name: &str) -> Vec<ScopedFact> {
    let mut out = Vec::new();
    let profile = memory_dir.join("profile.md");
    if let Ok(raw) = fs::read_to_string(profile) {
        for line in raw.lines() {
            if let Some((memory, order)) = parse_fact(line, MemoryKind::Profile, out.len()) {
                out.push(ScopedFact {
                    agent_name: agent_name.to_string(),
                    memory,
                    order,
                });
            }
        }
    }

    let log_dir = memory_dir.join("log");
    let mut logs = fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    logs.sort();

    for path in logs {
        if let Ok(raw) = fs::read_to_string(path) {
            for line in raw.lines() {
                if let Some((memory, order)) = parse_fact(line, MemoryKind::Log, out.len()) {
                    out.push(ScopedFact {
                        agent_name: agent_name.to_string(),
                        memory,
                        order,
                    });
                }
            }
        }
    }
    out
}

fn importance(content: &str) -> f64 {
    if content.starts_with("[episode] ") {
        1.5
    } else if content.starts_with("[note] ") {
        0.5
    } else {
        1.0
    }
}

fn recall_rank(memory: &MemoryRecord) -> f64 {
    importance(&memory.content).log2()
        + memory.created_at as f64 / (MEMORY_DECAY_HALF_LIFE_DAYS * 86_400_000.0)
}

fn merge_scoped(mut facts: Vec<ScopedFact>, kind: MemoryKind, limit: usize) -> Vec<ScopedFact> {
    facts.retain(|fact| fact.memory.kind == kind);
    facts.sort_by(|left, right| {
        right
            .memory
            .created_at
            .cmp(&left.memory.created_at)
            .then_with(|| right.order.cmp(&left.order))
    });
    let mut seen = BTreeSet::new();
    facts.retain(|fact| seen.insert(fact.memory.content.to_lowercase()));
    if kind == MemoryKind::Log {
        facts.sort_by(|left, right| {
            recall_rank(&right.memory)
                .partial_cmp(&recall_rank(&left.memory))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.memory.created_at.cmp(&left.memory.created_at))
                .then_with(|| right.order.cmp(&left.order))
        });
    }
    facts.truncate(limit);
    facts
}

fn display_name(agent_id: &str, names: &BTreeMap<String, String>) -> String {
    names
        .get(agent_id)
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| agent_id.to_string())
}

fn render_scoped_sections(
    heading: &str,
    precedence: &str,
    location: &Path,
    own_shard: Option<&Path>,
    facts: Vec<ScopedFact>,
) -> String {
    let profile = merge_scoped(facts.clone(), MemoryKind::Profile, PROFILE_LIMIT);
    let recent = merge_scoped(facts, MemoryKind::Log, RECENT_LIMIT);
    if profile.is_empty() && recent.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        heading.to_string(),
        precedence.to_string(),
        format!("Shared memory root: {}.", location.to_string_lossy()),
    ];
    if let Some(own_shard) = own_shard {
        lines.push(format!("Your own shard is at {}.", own_shard.to_string_lossy()));
    }
    if !profile.is_empty() {
        lines.push("Shared profile facts:".into());
        for fact in profile {
            lines.push(format!("- [via {}] {}", fact.agent_name, fact.memory.content));
        }
    }
    if !recent.is_empty() {
        lines.push("Shared recent facts:".into());
        let mut budget = RECENT_CHAR_BUDGET;
        for fact in recent {
            let line = format!("- [via {}] {}", fact.agent_name, fact.memory.content);
            if line.len() > budget {
                break;
            }
            budget -= line.len();
            lines.push(line);
        }
    }
    lines.join("\n")
}

pub(crate) fn read_memberships(agent_root: &Path) -> BTreeSet<String> {
    let Ok(raw) = fs::read_to_string(agent_root.join("projects.json")) else {
        return BTreeSet::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return BTreeSet::new();
    };
    value
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|slug| safe_component(slug))
        .map(str::to_string)
        .collect()
}

fn project_name(project_dir: &Path, slug: &str) -> String {
    let raw = fs::read_to_string(project_dir.join("project.md")).unwrap_or_default();
    let mut in_frontmatter = false;
    for line in raw.lines().map(str::trim) {
        if line == "---" {
            if in_frontmatter {
                break;
            }
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if let Some(value) = line.strip_prefix("name:") {
                let value = value.trim().trim_matches(['"', '\'']);
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    slug.to_string()
}

fn read_sharded_root(root: &Path, names: &BTreeMap<String, String>) -> Vec<ScopedFact> {
    let mut agent_ids = fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|agent_id| safe_component(agent_id))
        .collect::<Vec<_>>();
    agent_ids.sort();

    let mut facts = Vec::new();
    for agent_id in agent_ids {
        facts.extend(read_facts(
            &root.join(&agent_id),
            &display_name(&agent_id, names),
        ));
    }
    facts
}

pub(crate) fn resolve_scoped_memory_dir(
    account_agents_root: &Path,
    own_agent_id: &str,
    scope: MemoryScope,
    project: Option<&str>,
) -> Result<PathBuf, String> {
    if !safe_component(own_agent_id) {
        return Err(format!("unsafe memory agent id: {own_agent_id}"));
    }
    match scope {
        MemoryScope::Agent => Ok(account_agents_root.join(own_agent_id).join("memory")),
        MemoryScope::User => Ok(account_agents_root
            .parent()
            .unwrap_or(account_agents_root)
            .join("user-memory")
            .join("agents")
            .join(own_agent_id)),
        MemoryScope::Project => {
            let slug = project
                .map(str::trim)
                .filter(|slug| !slug.is_empty())
                .ok_or_else(|| "project is required for project memory scope".to_string())?;
            if !safe_component(slug) {
                return Err(format!("unsafe project memory slug: {slug}"));
            }
            let sand_root = account_agents_root.parent().unwrap_or(account_agents_root);
            let project_dir = sand_root.join("projects").join(slug);
            if !project_dir.is_dir() {
                return Err(format!("unknown project memory scope: {slug}"));
            }
            if !read_memberships(&account_agents_root.join(own_agent_id)).contains(slug) {
                return Err(format!(
                    "agent {own_agent_id} has not joined project memory scope: {slug}"
                ));
            }
            Ok(project_dir.join("memory").join("agents").join(own_agent_id))
        }
    }
}

fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    fs::write(&temporary, body)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(first) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&temporary, path).map_err(|_| first)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn write_memberships(agent_root: &Path, slugs: &BTreeSet<String>) -> Result<(), String> {
    let body = serde_json::to_string_pretty(&serde_json::json!({
        "projects": slugs.iter().cloned().collect::<Vec<_>>()
    }))
    .map_err(|error| format!("serialize project membership: {error}"))?;
    write_atomic(
        &agent_root.join("projects.json"),
        format!("{body}\n").as_bytes(),
    )
    .map_err(|error| format!("write project membership: {error}"))
}

fn project_frontmatter(project_dir: &Path, slug: &str) -> MemoryProjectSummary {
    let raw = fs::read_to_string(project_dir.join("project.md")).unwrap_or_default();
    let mut in_frontmatter = false;
    let mut name = None;
    let mut description = None;
    for line in raw.lines().map(str::trim) {
        if line == "---" {
            if in_frontmatter {
                break;
            }
            in_frontmatter = true;
            continue;
        }
        if !in_frontmatter {
            continue;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches(['"', '\'']);
            if !value.is_empty() {
                name = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim().trim_matches(['"', '\'']);
            if !value.is_empty() {
                description = Some(value.to_string());
            }
        }
    }
    MemoryProjectSummary {
        slug: slug.to_string(),
        name: name.unwrap_or_else(|| slug.to_string()),
        description,
    }
}

pub(crate) fn list_joined_projects(
    account_agents_root: &Path,
    agent_id: &str,
) -> Result<Vec<MemoryProjectSummary>, String> {
    if !safe_component(agent_id) {
        return Err(format!("unsafe memory agent id: {agent_id}"));
    }
    let sand_root = account_agents_root.parent().unwrap_or(account_agents_root);
    let agent_root = account_agents_root.join(agent_id);
    let mut membership = read_memberships(&agent_root);
    let before = membership.len();
    membership.retain(|slug| sand_root.join("projects").join(slug).is_dir());
    if membership.len() != before {
        write_memberships(&agent_root, &membership)?;
    }
    Ok(membership
        .into_iter()
        .map(|slug| project_frontmatter(&sand_root.join("projects").join(&slug), &slug))
        .collect())
}

pub(crate) fn create_or_join_project(
    account_agents_root: &Path,
    agent_id: &str,
    slug: &str,
    name: &str,
    description: Option<&str>,
) -> Result<MemoryProjectSummary, String> {
    if !safe_component(agent_id) {
        return Err(format!("unsafe memory agent id: {agent_id}"));
    }
    let slug = slug.trim();
    if !safe_component(slug) {
        return Err(format!("invalid project slug: {slug}"));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("a project needs a non-empty name".into());
    }
    let sand_root = account_agents_root.parent().unwrap_or(account_agents_root);
    let project_dir = sand_root.join("projects").join(slug);
    if !project_dir.exists() {
        fs::create_dir_all(&project_dir)
            .map_err(|error| format!("create project {slug}: {error}"))?;
        let body = format!(
            "---\nname: {}\ndescription: {}\n---\n",
            name,
            description.unwrap_or_default().trim()
        );
        write_atomic(&project_dir.join("project.md"), body.as_bytes())
            .map_err(|error| format!("write project {slug}: {error}"))?;
    }
    join_project(account_agents_root, agent_id, slug)?;
    Ok(project_frontmatter(&project_dir, slug))
}

pub(crate) fn join_project(
    account_agents_root: &Path,
    agent_id: &str,
    slug: &str,
) -> Result<MemoryProjectSummary, String> {
    if !safe_component(agent_id) {
        return Err(format!("unsafe memory agent id: {agent_id}"));
    }
    let slug = slug.trim();
    if !safe_component(slug) {
        return Err(format!("invalid project slug: {slug}"));
    }
    let sand_root = account_agents_root.parent().unwrap_or(account_agents_root);
    let project_dir = sand_root.join("projects").join(slug);
    if !project_dir.is_dir() {
        return Err(format!("no project {slug} exists"));
    }
    let agent_root = account_agents_root.join(agent_id);
    let mut membership = read_memberships(&agent_root);
    membership.insert(slug.to_string());
    write_memberships(&agent_root, &membership)?;
    Ok(project_frontmatter(&project_dir, slug))
}

pub(crate) fn leave_project(
    account_agents_root: &Path,
    agent_id: &str,
    slug: &str,
) -> Result<Option<MemoryProjectSummary>, String> {
    if !safe_component(agent_id) {
        return Err(format!("unsafe memory agent id: {agent_id}"));
    }
    let slug = slug.trim();
    if !safe_component(slug) {
        return Err(format!("invalid project slug: {slug}"));
    }
    let sand_root = account_agents_root.parent().unwrap_or(account_agents_root);
    let project_dir = sand_root.join("projects").join(slug);
    let summary = project_dir
        .is_dir()
        .then(|| project_frontmatter(&project_dir, slug));
    let agent_root = account_agents_root.join(agent_id);
    let mut membership = read_memberships(&agent_root);
    membership.remove(slug);
    write_memberships(&agent_root, &membership)?;
    Ok(summary)
}

pub(crate) fn render_shared_memory_prompt(
    account_agents_root: &Path,
    own_agent_id: &str,
    names: &BTreeMap<String, String>,
) -> String {
    if !safe_component(own_agent_id) {
        return String::new();
    }
    let sand_root = account_agents_root.parent().unwrap_or(account_agents_root);
    let mut sections = Vec::new();

    let user_root = sand_root.join("user-memory");
    let user_shards = user_root.join("agents");
    let user = read_sharded_root(&user_shards, names);
    let rendered_user = render_scoped_sections(
        "User memory: durable facts shared across every assistant this user runs.",
        "Precedence: when a shared user fact conflicts with your OWN memory, prefer your own memory.",
        &user_root,
        Some(&user_shards.join(own_agent_id)),
        user,
    );
    if !rendered_user.is_empty() {
        sections.push(rendered_user);
    }

    let own_agent_root = account_agents_root.join(own_agent_id);
    let memberships = read_memberships(&own_agent_root);
    let projects_root = sand_root.join("projects");
    let mut blocks = Vec::new();
    for slug in memberships {
        let project_dir = projects_root.join(&slug);
        if !project_dir.is_dir() {
            continue;
        }
        let shards = project_dir.join("memory").join("agents");
        let facts = read_sharded_root(&shards, names);
        let newest = facts.iter().map(|fact| fact.memory.created_at).max().unwrap_or(0);
        blocks.push((
            slug.clone(),
            project_name(&project_dir, &slug),
            newest,
            facts,
            shards.join(own_agent_id),
        ));
    }
    blocks.sort_by(|left, right| {
        let left_has = !left.3.is_empty();
        let right_has = !right.3.is_empty();
        right_has
            .cmp(&left_has)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut project_blocks = Vec::new();
    let mut tail = Vec::new();
    for (index, (slug, name, _, facts, own_shard)) in blocks.into_iter().enumerate() {
        if index >= PROJECT_INJECTED_LIMIT {
            tail.push(format!("{name} ({slug})"));
            continue;
        }
        let rendered = render_scoped_sections(
            &format!("Project \"{name}\" ({slug})"),
            "Precedence: prefer your OWN memory first, then project memory, then user memory.",
            &projects_root,
            Some(&own_shard),
            facts,
        );
        if !rendered.is_empty() {
            project_blocks.push(rendered);
        } else {
            tail.push(format!("{name} ({slug})"));
        }
    }
    if !project_blocks.is_empty() || !tail.is_empty() {
        let mut project = vec![
            "Project memory: durable facts shared by every assistant that has joined a project."
                .to_string(),
            "Precedence: prefer your OWN memory first, then project memory, then user memory."
                .to_string(),
        ];
        project.extend(project_blocks);
        if !tail.is_empty() {
            project.push(format!("Also a member of: {}", tail.join(", ")));
        }
        sections.push(project.join("\n"));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-shared-memory-{label}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos()
        ))
    }

    fn write_memory(dir: &Path, profile: &str, log: &str) {
        fs::create_dir_all(dir.join("log")).expect("memory dirs");
        if !profile.is_empty() {
            fs::write(
                dir.join("profile.md"),
                format!("# About the user\n\n- (2026-10-01) {profile}\n"),
            )
            .expect("profile");
        }
        if !log.is_empty() {
            fs::write(
                dir.join("log").join("2026-10.md"),
                format!("# Memory log\n\n- (2026-10-02) {log}\n"),
            )
            .expect("log");
        }
    }

    #[test]
    fn shared_user_memory_dedupes_by_newest_shard_and_preserves_provenance() {
        let sand = fixture_root("user");
        let agents = sand.join("agents");
        fs::create_dir_all(agents.join("agent-a")).expect("agent");
        write_memory(&sand.join("user-memory/agents/agent-a"), "The user prefers concise answers", "");
        write_memory(&sand.join("user-memory/agents/agent-b"), "THE USER PREFERS CONCISE ANSWERS", "[episode] booked Tokyo");
        let names = BTreeMap::from([
            ("agent-a".into(), "Researcher".into()),
            ("agent-b".into(), "Planner".into()),
        ]);
        let rendered = render_shared_memory_prompt(&agents, "agent-a", &names);
        assert!(rendered.contains("User memory: durable facts shared"));
        assert!(rendered.contains("prefer your own memory"));
        assert_eq!(
            rendered.to_lowercase().matches("the user prefers concise answers").count(),
            1
        );
        assert!(rendered.contains("[via Planner]"));
        assert!(rendered.contains("[episode] booked Tokyo"));
        let _ = fs::remove_dir_all(sand);
    }

    #[test]
    fn project_membership_create_join_leave_and_prune_are_durable() {
        let sand = fixture_root("membership");
        let agents = sand.join("agents");
        fs::create_dir_all(agents.join("agent-a")).expect("agent");

        let created = create_or_join_project(
            &agents,
            "agent-a",
            "alpha",
            "Alpha Project",
            Some("shared work"),
        )
        .expect("create");
        assert_eq!(created.slug, "alpha");
        assert_eq!(created.name, "Alpha Project");
        assert!(read_memberships(&agents.join("agent-a")).contains("alpha"));

        fs::create_dir_all(sand.join("projects/beta")).expect("beta");
        fs::write(
            sand.join("projects/beta/project.md"),
            "---\nname: Beta\ndescription: second\n---\n",
        )
        .expect("beta metadata");
        join_project(&agents, "agent-a", "beta").expect("join");
        let projects = list_joined_projects(&agents, "agent-a").expect("list");
        assert_eq!(projects.iter().map(|project| project.slug.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta"]);

        leave_project(&agents, "agent-a", "alpha").expect("leave");
        assert!(!read_memberships(&agents.join("agent-a")).contains("alpha"));

        fs::remove_dir_all(sand.join("projects/beta")).expect("remove beta");
        assert!(list_joined_projects(&agents, "agent-a").expect("prune").is_empty());
        assert!(read_memberships(&agents.join("agent-a")).is_empty());
        let _ = fs::remove_dir_all(sand);
    }

    #[test]
    fn scoped_memory_dir_preserves_account_isolation_and_project_membership_gate() {
        let sand = fixture_root("scope");
        let agents = sand.join("agents");
        let own = agents.join("agent-a");
        fs::create_dir_all(&own).expect("agent");
        fs::create_dir_all(sand.join("projects/alpha")).expect("project");
        fs::write(own.join("projects.json"), r#"{"projects":["alpha"]}"#).expect("membership");

        assert_eq!(
            resolve_scoped_memory_dir(&agents, "agent-a", MemoryScope::Agent, None)
                .expect("agent scope"),
            agents.join("agent-a/memory")
        );
        assert_eq!(
            resolve_scoped_memory_dir(&agents, "agent-a", MemoryScope::User, None)
                .expect("user scope"),
            sand.join("user-memory/agents/agent-a")
        );
        assert_eq!(
            resolve_scoped_memory_dir(&agents, "agent-a", MemoryScope::Project, Some("alpha"))
                .expect("joined project scope"),
            sand.join("projects/alpha/memory/agents/agent-a")
        );
        assert!(
            resolve_scoped_memory_dir(
                &agents,
                "agent-a",
                MemoryScope::Project,
                Some("../escape")
            )
            .is_err()
        );
        fs::create_dir_all(sand.join("projects/beta")).expect("second project");
        assert!(
            resolve_scoped_memory_dir(&agents, "agent-a", MemoryScope::Project, Some("beta"))
                .is_err()
        );
        let _ = fs::remove_dir_all(sand);
    }

    #[test]
    fn project_memory_is_membership_scoped_ranked_and_bounded_to_three_blocks() {
        let sand = fixture_root("projects");
        let agents = sand.join("agents");
        let own = agents.join("agent-a");
        fs::create_dir_all(&own).expect("agent");
        fs::write(
            own.join("projects.json"),
            r#"{"projects":["alpha","beta","gamma","delta","../escape"]}"#,
        )
        .expect("membership");
        let names = BTreeMap::from([
            ("agent-a".into(), "Researcher".into()),
            ("agent-b".into(), "Planner".into()),
        ]);
        for (index, slug) in ["alpha", "beta", "gamma", "delta"].into_iter().enumerate() {
            let project = sand.join("projects").join(slug);
            fs::create_dir_all(&project).expect("project");
            fs::write(project.join("project.md"), format!("---\nname: Project {slug}\n---\n"))
                .expect("metadata");
            write_memory(
                &project.join("memory/agents/agent-b"),
                "",
                &format!("shared project fact {index}"),
            );
        }
        let rendered = render_shared_memory_prompt(&agents, "agent-a", &names);
        assert!(rendered.contains("Project memory: durable facts shared"));
        assert!(rendered.contains("prefer your OWN memory first, then project memory, then user memory"));
        assert!(!rendered.contains("../escape"));
        assert_eq!(rendered.matches("Shared memory root:").count(), 3);
        assert!(rendered.contains("Also a member of:"));
        let _ = fs::remove_dir_all(sand);
    }
}
