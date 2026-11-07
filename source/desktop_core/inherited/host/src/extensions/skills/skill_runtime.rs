use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub allowed_tools: Vec<String>,
    pub user_invocable: bool,
}

pub fn parse_skill_markdown(raw: &str) -> Result<SkillDefinition, String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with("---") {
        return Err("SKILL.md missing leading '---' frontmatter boundary".to_string());
    }

    let rest = &trimmed[3..];
    let end_idx = rest
        .find("\n---")
        .ok_or_else(|| "SKILL.md missing trailing '---' frontmatter boundary".to_string())?;

    let frontmatter = &rest[..end_idx];
    let instructions = rest[end_idx + 4..].trim().to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut allowed_tools = Vec::new();
    let mut user_invocable = true;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').trim_matches('\'');
            match key {
                "name" => name = val.to_string(),
                "description" => description = val.to_string(),
                "user_invocable" => user_invocable = val.parse::<bool>().unwrap_or(true),
                "allowed_tools" => {
                    if !val.is_empty() {
                        for t in val.split(',') {
                            let tool = t.trim();
                            if !tool.is_empty() {
                                allowed_tools.push(tool.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if name.is_empty() {
        return Err("Skill frontmatter missing 'name' property".to_string());
    }

    Ok(SkillDefinition {
        name,
        description,
        instructions,
        allowed_tools,
        user_invocable,
    })
}

#[derive(Clone, Default)]
pub struct SkillRegistry {
    skills: Arc<Mutex<HashMap<String, SkillDefinition>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_skill(&self, skill: SkillDefinition) {
        let mut skills = self.skills.lock().unwrap();
        skills.insert(skill.name.to_ascii_lowercase(), skill);
    }

    pub fn get_skill(&self, name: &str) -> Option<SkillDefinition> {
        let skills = self.skills.lock().unwrap();
        skills.get(&name.to_ascii_lowercase()).cloned()
    }

    pub fn list_skills(&self) -> Vec<SkillDefinition> {
        let skills = self.skills.lock().unwrap();
        skills.values().cloned().collect()
    }

    pub fn format_prompt_injection(&self) -> String {
        let skills = self.list_skills();
        if skills.is_empty() {
            return String::new();
        }

        let mut output = String::from("Available skills:\n");
        for skill in skills {
            output.push_str(&format!("- {}: {}\n", skill.name, skill.description));
        }
        output
    }
}
