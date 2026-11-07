use std::fs;
use std::path::{Path, PathBuf};

use crate::automations::automation::{AUTOMATION_UI_LIMIT, AutomationRecord, AutomationSpec};
use crate::automations::automation_store::FileAutomationStore;
use crate::extensions::memory::memory_service::{get_project_dir, project_dir_exists};
use crate::storage::folder_id::is_safe_folder_id;
use crate::workflows::workflow_library::WorkflowSpec;
use crate::workflows::workflow_store::{
    FileWorkflowStore, WorkflowImportBatch, WorkflowImportResult, WorkflowImportSkipped,
    WorkflowRecord,
};

use super::agent_session::{now_ms, SandAgentSessionStore};

#[derive(Debug, Clone, PartialEq)]
pub struct AgentAutomationEntry {
    pub agent_id: String,
    pub automation: AutomationRecord,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowImportResponse {
    pub workflows: Vec<WorkflowRecord>,
    pub result: WorkflowImportBatch,
}

pub(crate) fn surface_automations(store: &FileAutomationStore) -> Vec<AutomationRecord> {
    store
        .list()
        .into_iter()
        .take(AUTOMATION_UI_LIMIT)
        .collect()
}

pub(crate) fn surface_workflows(store: &FileWorkflowStore) -> Vec<WorkflowRecord> {
    const WORKFLOW_UI_LIMIT: usize = 100;
    let mut managed = Vec::new();
    let mut user = Vec::new();
    for workflow in store.list_all() {
        if matches!(workflow.source.as_str(), "managed" | "plugin") {
            managed.push(workflow);
        } else {
            user.push(workflow);
        }
    }
    managed.extend(user.into_iter().take(WORKFLOW_UI_LIMIT));
    managed
}

impl SandAgentSessionStore {
    pub fn automation_store_for(&self, agent_id: &str) -> Result<FileAutomationStore, String> {
        self.production.open_automation_store(agent_id)
    }

    /// Run an external automation-store mutation under the canonical session
    /// filesystem/deletion boundary and its durable work-admission fence.
    /// Callers must keep all FileAutomationStore writes inside this closure.
    pub fn with_admitted_filesystem_mutation<T, F>(
        &self,
        agent_id: &str,
        operation: F,
    ) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        self.production
            .with_agent_admitted_filesystem_mutation(agent_id, operation)
    }

    pub fn list_agent_automations(&self, agent_id: &str) -> Result<Vec<AutomationRecord>, String> {
        Ok(surface_automations(&self.automation_store_for(agent_id)?))
    }

    pub fn join_agent_project(&self, agent_id: &str, slug: &str) -> Result<bool, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.production
                .memory_service()
                .project_membership_for_agent(agent_id)
                .join(slug)
                .map_err(|error| error.to_string())
        })
    }

    pub fn create_agent_project(
        &self,
        agent_id: &str,
        slug: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<bool, String> {
        if !is_safe_folder_id(slug) {
            return Err(format!("\"{slug}\" is not a valid project slug"));
        }
        if name.trim().is_empty() {
            return Err("a project needs a non-empty name".into());
        }
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let memory = self.production.memory_service();
            let project_root = memory.sand_root().to_path_buf();
            let project_dir = get_project_dir(&project_root, slug);
            let existed = project_dir_exists(&project_root, slug);
            if !existed {
                fs::create_dir_all(&project_dir).map_err(|error| error.to_string())?;
                let body = format!(
                    "---\nname: {name}\ndescription: {}\n---\n",
                    description.unwrap_or_default()
                );
                fs::write(project_dir.join("project.md"), body)
                    .map_err(|error| error.to_string())?;
            }
            let joined = memory
                .project_membership_for_agent(agent_id)
                .join(slug)
                .map_err(|error| error.to_string())?;
            if !joined {
                return Err(format!("could not join project \"{slug}\""));
            }
            Ok(!existed)
        })
    }

    pub fn leave_agent_project(&self, agent_id: &str, slug: &str) -> Result<bool, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            self.production
                .memory_service()
                .project_membership_for_agent(agent_id)
                .leave(slug)
                .map_err(|error| error.to_string())
        })
    }

    pub fn set_agent_automation_enabled(
        &self,
        agent_id: &str,
        automation_id: &str,
        enabled: bool,
    ) -> Result<Vec<AutomationRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.automation_store_for(agent_id)?;
            let _ = store
                .set_enabled(automation_id, enabled)
                .map_err(|error| error.to_string())?;
            Ok(surface_automations(&store))
        })
    }

    pub fn create_agent_automation(
        &self,
        agent_id: &str,
        spec: &AutomationSpec,
    ) -> Result<Vec<AutomationRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.automation_store_for(agent_id)?;
            let _ = store
                .upsert(spec, now_ms())
                .map_err(|error| error.to_string())?;
            Ok(surface_automations(&store))
        })
    }

    pub fn update_agent_automation(
        &self,
        agent_id: &str,
        automation_id: &str,
        spec: &AutomationSpec,
    ) -> Result<Vec<AutomationRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.automation_store_for(agent_id)?;
            let _ = store
                .update(automation_id, spec)
                .map_err(|error| error.to_string())?;
            Ok(surface_automations(&store))
        })
    }

    pub fn remove_agent_automation(
        &self,
        agent_id: &str,
        automation_id: &str,
    ) -> Result<Vec<AutomationRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.automation_store_for(agent_id)?;
            let _ = store
                .remove(automation_id)
                .map_err(|error| error.to_string())?;
            Ok(surface_automations(&store))
        })
    }

    pub fn workflow_store_for(&self, agent_id: &str) -> Result<FileWorkflowStore, String> {
        self.production.open_workflow_store(agent_id)
    }

    pub fn list_agent_workflows(&self, agent_id: &str) -> Result<Vec<WorkflowRecord>, String> {
        Ok(surface_workflows(&self.workflow_store_for(agent_id)?))
    }

    pub fn get_agent_workflow(
        &self,
        agent_id: &str,
        workflow_id: &str,
    ) -> Result<Option<WorkflowRecord>, String> {
        Ok(self.workflow_store_for(agent_id)?.get(workflow_id))
    }

    pub fn create_agent_workflow(
        &self,
        agent_id: &str,
        spec: &WorkflowSpec,
    ) -> Result<Vec<WorkflowRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let _ = store.create(spec)?;
            Ok(surface_workflows(&store))
        })
    }

    pub fn update_agent_workflow(
        &self,
        agent_id: &str,
        workflow_id: &str,
        spec: &WorkflowSpec,
    ) -> Result<Vec<WorkflowRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let _ = store.update(workflow_id, spec)?;
            Ok(surface_workflows(&store))
        })
    }

    pub fn set_agent_workflow_enabled(
        &self,
        agent_id: &str,
        workflow_id: &str,
        enabled: bool,
    ) -> Result<Vec<WorkflowRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let _ = store.set_enabled_for_agent(workflow_id, enabled)?;
            Ok(surface_workflows(&store))
        })
    }

    pub fn remove_agent_workflow(
        &self,
        agent_id: &str,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowRecord>, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let _ = store.remove(workflow_id)?;
            Ok(surface_workflows(&store))
        })
    }

    pub fn import_agent_workflow_markdown(
        &self,
        agent_id: &str,
        markdown: &str,
        fallback_name: Option<&str>,
    ) -> Result<WorkflowImportResponse, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let result = match store.import_markdown(markdown, fallback_name)? {
                Some((id, name)) => WorkflowImportBatch {
                    imported: vec![WorkflowImportResult { id, name }],
                    skipped: Vec::new(),
                },
                None => WorkflowImportBatch {
                    imported: Vec::new(),
                    skipped: vec![WorkflowImportSkipped {
                        source: "pasted skill".into(),
                        reason: "empty or invalid".into(),
                    }],
                },
            };
            Ok(WorkflowImportResponse {
                workflows: surface_workflows(&store),
                result,
            })
        })
    }

    pub fn import_agent_workflow_source(
        &self,
        agent_id: &str,
        source: &str,
        fallback_name: Option<&str>,
    ) -> Result<WorkflowImportResponse, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let result = match store.import_live_source(source, fallback_name)? {
                Some((id, name)) => WorkflowImportBatch {
                    imported: vec![WorkflowImportResult { id, name }],
                    skipped: Vec::new(),
                },
                None => WorkflowImportBatch {
                    imported: Vec::new(),
                    skipped: vec![WorkflowImportSkipped {
                        source: source.to_string(),
                        reason: "could not link".into(),
                    }],
                },
            };
            Ok(WorkflowImportResponse {
                workflows: surface_workflows(&store),
                result,
            })
        })
    }

    pub fn port_agent_local_skills_from(
        &self,
        agent_id: &str,
        home_dir: &Path,
        cwd: &Path,
    ) -> Result<WorkflowImportResponse, String> {
        self.production.with_agent_admitted_filesystem_mutation(agent_id, || {
            let store = self.workflow_store_for(agent_id)?;
            let result = store.port_local_skills(home_dir, cwd)?;
            Ok(WorkflowImportResponse {
                workflows: surface_workflows(&store),
                result,
            })
        })
    }

    pub fn port_agent_local_skills(
        &self,
        agent_id: &str,
    ) -> Result<WorkflowImportResponse, String> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.port_agent_local_skills_from(agent_id, &home, &cwd)
    }

    pub fn list_all_automations_from(
        &self,
        definitions_only: bool,
    ) -> Result<Vec<AgentAutomationEntry>, String> {
        let mut result = Vec::new();
        for agent_id in self.list_agent_ids()? {
            let Ok(store) = self.automation_store_for(&agent_id) else {
                continue;
            };
            let automations = if definitions_only {
                store.list_definitions()
            } else {
                store.list()
            };
            result.extend(automations.into_iter().map(|automation| AgentAutomationEntry {
                agent_id: agent_id.clone(),
                automation,
            }));
        }
        Ok(result)
    }

    pub fn list_all_automations(&self) -> Result<Vec<AgentAutomationEntry>, String> {
        self.list_all_automations_from(false)
    }

    pub fn list_all_automation_definitions(&self) -> Result<Vec<AgentAutomationEntry>, String> {
        self.list_all_automations_from(true)
    }
}
