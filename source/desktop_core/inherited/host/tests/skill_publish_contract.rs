use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

use mahayana_host_runtime::extensions::mcp::plugin_skills_cache::{
    PluginSkillRecord, PluginSkillsCache,
};
use mahayana_host_runtime::extensions::mcp::skill_publish::{
    PublishPluginInput, PublishedSkillResult, SandSkillPublishService, SkillPublishClient,
    SkillPublishPluginSkills, SkillPublishTeam, PUBLISH_SKILL_RPC_TIMEOUT_MS,
    PUBLISH_TARGETS_RPC_TIMEOUT_MS, normalize_marketplace_name, pack_plugin_artifact,
    skills_root_relative_path, stage_skill_dir_for_publish, synthesize_skill_plugin_dir,
};
use mahayana_host_runtime::workflows::workflow_library::WorkflowSpec;
use tar::Archive;
use uuid::Uuid;

fn root(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("skill-publish-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&path).unwrap();
    path
}

struct FakeClient {
    teams: Mutex<Result<Vec<SkillPublishTeam>, String>>,
    published: Mutex<Vec<PublishPluginInput>>,
    unpublished: Mutex<Vec<(String, i32)>>,
}
impl Default for FakeClient {
    fn default() -> Self {
        Self {
            teams: Mutex::new(Ok(Vec::new())),
            published: Mutex::new(Vec::new()),
            unpublished: Mutex::new(Vec::new()),
        }
    }
}
impl FakeClient {
    fn with_teams(teams: Result<Vec<SkillPublishTeam>, String>) -> Self {
        Self { teams: Mutex::new(teams), ..Self::default() }
    }
}
impl SkillPublishClient for FakeClient {
    fn get_teams(&self, timeout_ms: u64) -> Result<Vec<SkillPublishTeam>, String> {
        assert_eq!(timeout_ms, PUBLISH_TARGETS_RPC_TIMEOUT_MS);
        self.teams.lock().unwrap().clone()
    }
    fn publish_plugin(
        &self,
        request: PublishPluginInput,
        timeout_ms: u64,
    ) -> Result<PublishedSkillResult, String> {
        assert_eq!(timeout_ms, PUBLISH_SKILL_RPC_TIMEOUT_MS);
        self.published.lock().unwrap().push(request);
        Ok(PublishedSkillResult { plugin_id: "42".into(), commit_sha: "abc123".into() })
    }
    fn unpublish_plugin(
        &self,
        plugin_id: &str,
        team_id: i32,
        timeout_ms: u64,
    ) -> Result<(), String> {
        assert_eq!(timeout_ms, PUBLISH_SKILL_RPC_TIMEOUT_MS);
        self.unpublished.lock().unwrap().push((plugin_id.into(), team_id));
        Ok(())
    }
}

struct FakePluginSkills {
    index: Mutex<Option<PluginSkillsCache>>,
    syncs: AtomicUsize,
}
impl FakePluginSkills {
    fn new(index: Option<PluginSkillsCache>) -> Self {
        Self { index: Mutex::new(index), syncs: AtomicUsize::new(0) }
    }
}
impl SkillPublishPluginSkills for FakePluginSkills {
    fn current_index(&self) -> Option<PluginSkillsCache> {
        self.index.lock().unwrap().clone()
    }
    fn sync(&self, _trigger: &str) -> Result<Vec<PluginSkillRecord>, String> {
        self.syncs.fetch_add(1, Ordering::SeqCst);
        Ok(self.current_index().map(|value| value.skills).unwrap_or_default())
    }
}

fn skill_record(file_path: &Path, install_path: &Path) -> PluginSkillRecord {
    PluginSkillRecord {
        id: "plugin-skill".into(),
        plugin_id: "42".into(),
        plugin_name: "team-plugin".into(),
        name: "Published Skill".into(),
        description: "does useful work".into(),
        file_path: file_path.display().to_string(),
        plugin_version: "abc123".into(),
        install_path: install_path.display().to_string(),
        skill_relative_path: "skills/foo/SKILL.md".into(),
        publisher_user_id: Some(7),
        marketplace_team_id: Some(9),
    }
}
fn cache(record: PluginSkillRecord) -> PluginSkillsCache {
    PluginSkillsCache {
        fetched_at: 1.0,
        current_user_id: Some(7),
        skills: vec![record],
        auth_blocked: vec![],
    }
}

#[test]
fn helpers_preserve_frozen_staging_and_plugin_shape() {
    let base = root("helpers");
    let source = base.join("my-skill");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: Demo\ndescription: useful\n---\nbody\n").unwrap();
    fs::write(source.join("workflow.md"), "legacy").unwrap();
    fs::write(source.join("runs.json"), "{}").unwrap();
    fs::write(source.join("helper.sh"), "echo ok").unwrap();
    fs::write(source.join("nested").join("runs.json"), "keep nested").unwrap();

    let staged_root = base.join("staged");
    fs::create_dir_all(&staged_root).unwrap();
    let staged = stage_skill_dir_for_publish(&source, &staged_root).unwrap();
    assert!(staged.join("SKILL.md").is_file());
    assert!(staged.join("helper.sh").is_file());
    assert!(!staged.join("workflow.md").exists());
    assert!(!staged.join("runs.json").exists());
    assert!(staged.join("nested").join("runs.json").is_file());

    assert_eq!(normalize_marketplace_name("  My Fancy.Plugin  "), "my-fancy-plugin");
    assert_eq!(skills_root_relative_path("skills/team/foo/SKILL.md").unwrap(), "team/foo");
    assert!(skills_root_relative_path("SKILL.md").is_err());

    let plugin_dir = synthesize_skill_plugin_dir(
        &staged,
        "team/foo",
        &base.join("plugin"),
        "My Fancy.Plugin",
        Some("Fancy Skill"),
    ).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(plugin_dir.join("plugin.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], "my-fancy-plugin");
    assert_eq!(manifest["displayName"], "Fancy Skill");
    assert_eq!(manifest["skills"][0], "skills/team/foo");
    assert!(plugin_dir.join("skills/team/foo/SKILL.md").is_file());

    let bytes = pack_plugin_artifact(&plugin_dir).unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    let decoder = flate2::read::GzDecoder::new(bytes.as_slice());
    let mut names = Archive::new(decoder)
        .entries().unwrap()
        .map(|entry| entry.unwrap().path().unwrap().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    names.sort();
    assert!(names.iter().any(|name| name.ends_with("plugin.json")));
    assert!(names.iter().any(|name| name.ends_with("skills/team/foo/SKILL.md")));

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn publish_removes_library_copy_only_after_matching_install_is_confirmed() {
    let sand = root("publish-confirmed");
    let client = Arc::new(FakeClient::default());
    let landed = sand.join("plugins/team-plugin/skills/foo/SKILL.md");
    fs::create_dir_all(landed.parent().unwrap()).unwrap();
    fs::write(&landed, "landed").unwrap();
    let skills = Arc::new(FakePluginSkills::new(Some(cache(skill_record(
        &landed,
        &sand.join("plugins/team-plugin"),
    )))));
    let service = SandSkillPublishService::new(sand.clone(), client.clone(), skills.clone());
    service.library().write_at("local-skill", &WorkflowSpec {
        name: "Local Skill".into(),
        description: "does useful work".into(),
        body: "body".into(),
        trigger: None,
        source_ref: None,
    }).unwrap().unwrap();
    fs::write(service.library().folder("local-skill").join("helper.sh"), "echo ok").unwrap();

    let result = service.publish("local-skill", 9).unwrap();
    assert_eq!(result.published.plugin_id, "42");
    assert_eq!(result.promoted_workflow_id.as_deref(), Some("plugin-skill"));
    assert!(service.library().get("local-skill").is_none());
    assert!(skills.syncs.load(Ordering::SeqCst) >= 1);

    let requests = client.published.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].team_id, 9);
    assert_eq!(requests[0].display_name, "Local Skill");
    assert!(!requests[0].plugin_tar_gz.is_empty());
    drop(requests);
    fs::remove_dir_all(sand).unwrap();
}

#[test]
fn unconfirmed_publish_keeps_library_copy_and_retries_sync_five_times() {
    let sand = root("publish-unconfirmed");
    let client = Arc::new(FakeClient::default());
    let skills = Arc::new(FakePluginSkills::new(None));
    let service = SandSkillPublishService::new(sand.clone(), client, skills.clone());
    service.library().write_at("local-skill", &WorkflowSpec {
        name: "Local Skill".into(),
        description: "does useful work".into(),
        body: "body".into(),
        trigger: None,
        source_ref: None,
    }).unwrap().unwrap();

    let result = service.publish("local-skill", 9).unwrap();
    assert_eq!(result.promoted_workflow_id, None);
    assert!(service.library().get("local-skill").is_some());
    assert_eq!(skills.syncs.load(Ordering::SeqCst), 5);
    fs::remove_dir_all(sand).unwrap();
}

#[test]
fn list_targets_and_unpublish_restore_follow_reference_fences() {
    let sand = root("unpublish");
    let client = Arc::new(FakeClient::with_teams(Ok(vec![
        SkillPublishTeam { team_id: 9, name: "Direct".into(), is_direct_member: true },
        SkillPublishTeam { team_id: 10, name: "Indirect".into(), is_direct_member: false },
        SkillPublishTeam { team_id: 0, name: "Invalid".into(), is_direct_member: true },
    ])));
    let install = sand.join("plugins/team-plugin");
    let skill_dir = install.join("skills/foo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(install.join("plugin.json"), r#"{"name":"team-plugin"}"#).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: Published Skill\ndescription: useful\n---\nbody\n").unwrap();
    let skills = Arc::new(FakePluginSkills::new(Some(cache(skill_record(
        &skill_dir.join("SKILL.md"),
        &install,
    )))));
    let service = SandSkillPublishService::new(sand.clone(), client.clone(), skills);

    let targets = service.list_targets();
    assert_eq!(targets.teams.len(), 1);
    assert_eq!(targets.teams[0].team_id, 9);
    assert_eq!(targets.unavailable_reason, None);

    let result = service.unpublish("plugin-skill").unwrap();
    assert_eq!(result.restored_workflow_id.as_deref(), Some("foo"));
    assert!(sand.join("workflows/foo/SKILL.md").is_file());
    assert_eq!(client.unpublished.lock().unwrap().as_slice(), &[("42".into(), 9)]);
    fs::remove_dir_all(sand).unwrap();
}
