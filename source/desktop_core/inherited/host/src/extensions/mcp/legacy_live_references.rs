use std::collections::HashSet;
use std::path::Path;

use crate::workflows::workflow_library::{GlobalWorkflowLibrary, get_global_workflows_dir};

use super::plugin_skills_cache::{get_plugin_skills_dir, read_plugin_skills_cache};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSkillCatalogEntry {
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCatalogEntry {
    pub plugin_id: String,
    pub skills: Vec<PluginSkillCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedPluginSkill {
    pub plugin_id: String,
    pub file_path: String,
}

pub fn is_live_reference_pointer_body(body: &str) -> bool {
    body.contains("is a live reference to the skill at")
}

pub fn normalize_plugin_skill_source_url(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if segments.len() < 5 || segments[2] != "blob" {
        return raw.to_string();
    }
    let owner = segments[0];
    let repo = segments[1];
    let path = segments[4..].join("/");
    if path.is_empty() {
        return raw.to_string();
    }
    format!(
        "{}/{owner}/{repo}/{path}",
        url.host_str().unwrap_or_default().to_ascii_lowercase()
    )
}

pub fn skill_path_tail(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let index = normalized.rfind("/skills/")?;
    Some(normalized[index + 1..].to_string())
}

pub fn collect_materialized_skill_source_urls(
    catalog: &[PluginCatalogEntry],
    records: &[MaterializedPluginSkill],
) -> Vec<String> {
    let materialized = records
        .iter()
        .filter_map(|record| {
            skill_path_tail(&record.file_path).map(|tail| format!("{}\0{tail}", record.plugin_id))
        })
        .collect::<HashSet<_>>();
    catalog
        .iter()
        .flat_map(|plugin| {
            plugin.skills.iter().filter_map(|skill| {
                let source_url = skill.source_url.as_ref()?;
                let tail = skill_path_tail(source_url)?;
                materialized
                    .contains(&format!("{}\0{tail}", plugin.plugin_id))
                    .then(|| source_url.clone())
            })
        })
        .collect()
}

pub fn sweep_legacy_plugin_skill_references(
    sand_root_dir: &Path,
    catalog: &[PluginCatalogEntry],
) -> usize {
    let materialized = materialized_plugin_skills_from_cache(sand_root_dir);
    if materialized.is_empty() {
        return 0;
    }
    let source_urls = collect_materialized_skill_source_urls(catalog, &materialized);
    remove_workflow_live_references(sand_root_dir, &source_urls)
}

pub fn remove_workflow_live_references(sand_root_dir: &Path, source_urls: &[String]) -> usize {
    if source_urls.is_empty() {
        return 0;
    }
    let refs = source_urls
        .iter()
        .map(|url| normalize_plugin_skill_source_url(url))
        .collect::<HashSet<_>>();
    let library = GlobalWorkflowLibrary::new(get_global_workflows_dir(sand_root_dir));
    let mut removed = 0;
    for record in library.list() {
        let Some(source_ref) = record.source_ref.as_deref() else {
            continue;
        };
        if refs.contains(&normalize_plugin_skill_source_url(source_ref))
            && is_live_reference_pointer_body(&record.body)
            && library.remove(&record.id).unwrap_or(false)
        {
            removed += 1;
        }
    }
    removed
}

pub fn materialized_plugin_skills_from_cache(sand_root_dir: &Path) -> Vec<MaterializedPluginSkill> {
    read_plugin_skills_cache(get_plugin_skills_dir(sand_root_dir))
        .map(|cache| {
            cache
                .skills
                .into_iter()
                .map(|skill| MaterializedPluginSkill {
                    plugin_id: skill.plugin_id,
                    file_path: skill.file_path,
                })
                .collect()
        })
        .unwrap_or_default()
}
