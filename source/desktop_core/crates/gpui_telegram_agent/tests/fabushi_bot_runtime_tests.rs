use gpui_telegram_agent::fabushi_bot::{
    FabushiBotHost, FabushiBotPatch, FabushiBotProvisionSpec,
};
use gpui_telegram_agent::hermes::memory::MemoryTarget;
use gpui_telegram_agent::hermes::providers::ProviderConfig;
use gpui_telegram_agent::hermes::skills::CreateSkillRequest;
use gpui_telegram_agent::hermes::toolsets::{FabushiTeammateHandler, HermesToolExecutor};
use gpui_telegram_agent::hermes::{HermesEngine, HermesEngineConfig};
use mahayana_host_runtime::agents::agent_messaging::render_agent_directory_system_prompt;
use mahayana_host_runtime::agents::agent_profile::read_sand_profile_file;
use mahayana_host_runtime::agents::settings_file::read_sand_settings_file;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Default)]
struct RecordingTeammateHandler {
    events: Mutex<Vec<String>>,
}

impl FabushiTeammateHandler for RecordingTeammateHandler {
    fn send_to_agent(
        &self,
        from_agent_id: &str,
        active_chat_id: Option<i64>,
        target_id: &str,
        message: &str,
        priority: bool,
        caller_depth: usize,
    ) -> std::result::Result<Value, String> {
        let mut guard = self.events.lock().unwrap();
        guard.push(format!(
            "send:{from_agent_id}->{target_id}:chat={:?}:prio={priority}:depth={caller_depth}:{message}",
            active_chat_id
        ));
        Ok(json!({
            "delivered": true,
            "from": from_agent_id,
            "to": target_id,
            "priority": priority
        }))
    }

    fn create_agent(
        &self,
        creator_agent_id: &str,
        active_chat_id: Option<i64>,
        name: &str,
        description: &str,
        avatar_shape: Option<&str>,
        avatar_color: Option<&str>,
    ) -> std::result::Result<Value, String> {
        let mut guard = self.events.lock().unwrap();
        guard.push(format!(
            "create:{creator_agent_id}:name={name}:chat={:?}",
            active_chat_id
        ));
        Ok(json!({
            "created": true,
            "name": name,
            "description": description,
            "avatarShape": avatar_shape,
            "avatarColor": avatar_color
        }))
    }

    fn update_agent(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        avatar_shape: Option<&str>,
        avatar_color: Option<&str>,
    ) -> std::result::Result<Value, String> {
        let mut guard = self.events.lock().unwrap();
        guard.push(format!(
            "update:{caller_agent_id}->{target_agent_id}:name={name:?}"
        ));
        Ok(json!({
            "updated": true,
            "targetId": target_agent_id,
            "name": name,
            "description": description,
            "avatarShape": avatar_shape,
            "avatarColor": avatar_color
        }))
    }
}

fn make_host_engine(dir: &TempDir) -> Arc<HermesEngine> {
    let cfg = HermesEngineConfig {
        workspace_root: dir.path().to_path_buf(),
        data_dir: Some(dir.path().join("host_hermes")),
        default_model: "hermes-local-v1".to_string(),
        ..Default::default()
    };
    Arc::new(HermesEngine::new(cfg).expect("init host hermes"))
}

#[test]
fn test_fabushi_bot_profile_settings_and_strict_per_bot_isolation() {
    let root = TempDir::new().expect("temp dir");
    let host_hermes = make_host_engine(&root);
    let fabushi_host = FabushiBotHost::new_with_dir(root.path().join("agents"), host_hermes)
        .expect("init fabushi host");

    let bot_a = fabushi_host
        .provision_or_sync_bot(FabushiBotProvisionSpec {
            bot_user_id: 600_001,
            owner_user_id: 100_001,
            username: "ArchitectAlphaBot".to_string(),
            display_name: "架构师 Alpha".to_string(),
            description: "负责系统架构设计与技术评审".to_string(),
            title: "Principal Architect".to_string(),
            agent_id: Some("architect-alpha".to_string()),
            avatar_shape: Some("hex".to_string()),
            avatar_color: Some("#10B981".to_string()),
            preferred_model: Some("deepseek-flash".to_string()),
            thinking_level: Some("high".to_string()),
            approval_mode: Some("autonomous".to_string()),
            toolset_preset: Some("full".to_string()),
            commands: vec![],
            inline_mode_enabled: true,
        })
        .expect("provision bot A");

    let bot_b = fabushi_host
        .provision_or_sync_bot(FabushiBotProvisionSpec {
            bot_user_id: 600_002,
            owner_user_id: 100_001,
            username: "DesignerBetaBot".to_string(),
            display_name: "设计师 Beta".to_string(),
            description: "负责 UI 视觉规范与交互走查".to_string(),
            title: "Design Lead".to_string(),
            agent_id: Some("designer-beta".to_string()),
            avatar_shape: Some("pebble".to_string()),
            avatar_color: Some("#EC4899".to_string()),
            preferred_model: None,
            thinking_level: Some("low".to_string()),
            approval_mode: Some("prompt".to_string()),
            toolset_preset: Some("safe".to_string()),
            commands: vec![],
            inline_mode_enabled: false,
        })
        .expect("provision bot B");

    // 1. Verify real Fabushi profile.json & settings.json files on disk
    let profile_a = read_sand_profile_file(bot_a.agent_dir.join("profile.json"))
        .expect("read alpha profile.json");
    assert_eq!(profile_a.name, "架构师 Alpha");
    assert_eq!(profile_a.avatar_shape, "hex");
    assert_eq!(profile_a.avatar_color, "#10B981");

    let settings_a = read_sand_settings_file(bot_a.agent_dir.join("settings.json"));
    assert!(settings_a.notify_on_agent_updates);
    assert!(!settings_a.hidden_from_sidebar);

    let profile_b = read_sand_profile_file(bot_b.agent_dir.join("profile.json"))
        .expect("read beta profile.json");
    assert_eq!(profile_b.avatar_shape, "pebble");
    assert_eq!(profile_b.avatar_color, "#EC4899");

    // 2. Verify strict Memory & Skill & SOUL isolation between Bot A and Bot B
    bot_a
        .hermes()
        .memory()
        .add(MemoryTarget::Memory, "BotA_Secret_Architecture_Fact=Rust_GPUI")
        .expect("write bot A memory");
    bot_a
        .hermes()
        .skills()
        .skill_create(CreateSkillRequest {
            name: "alpha-review-checklist".to_string(),
            description: "Architecture security checklist".to_string(),
            body: "# Alpha Checklist\nAlways check ownership boundaries.".to_string(),
            ..Default::default()
        })
        .expect("create bot A skill");

    let mem_a = bot_a
        .hermes()
        .memory()
        .read(MemoryTarget::Memory)
        .expect("read bot A memory");
    let mem_b = bot_b
        .hermes()
        .memory()
        .read(MemoryTarget::Memory)
        .expect("read bot B memory");
    assert!(mem_a.raw.contains("BotA_Secret_Architecture_Fact"));
    assert!(!mem_b.raw.contains("BotA_Secret_Architecture_Fact"));
    assert!(mem_b.entries.is_empty());

    assert!(bot_a
        .hermes()
        .skills()
        .skills_list(None)
        .iter()
        .any(|s| s.frontmatter.name == "alpha-review-checklist"));
    assert!(!bot_b
        .hermes()
        .skills()
        .skills_list(None)
        .iter()
        .any(|s| s.frontmatter.name == "alpha-review-checklist"));

    let soul_a = bot_a.hermes().memory().read_soul().expect("soul A");
    let soul_b = bot_b.hermes().memory().read_soul().expect("soul B");
    assert!(soul_a.persona.contains("系统架构"));
    assert!(soul_b.persona.contains("UI 视觉"));

    // 3. Verify live AgentProfilePromptSnapshot delta announcement on profile mutation
    let (sys1, prefix1) =
        fabushi_host.build_turn_system_prompt(600_001, 700_001, "Base", &[], &[]);
    assert!(prefix1.is_none());
    assert!(sys1.contains("designer-beta"));
    let expected_dir = render_agent_directory_system_prompt(
        &[bot_b.agent_address()],
        &[],
        Some(&fabushi_host.agents_root().to_string_lossy()),
    );
    assert!(sys1.contains(&expected_dir[..60]));

    fabushi_host
        .update_bot(
            600_001,
            FabushiBotPatch {
                display_name: Some("首席架构师 Alpha".to_string()),
                ..Default::default()
            },
        )
        .expect("update bot A name");
    let (_sys2, prefix2) =
        fabushi_host.build_turn_system_prompt(600_001, 700_001, "Base", &[], &[]);
    let delta = prefix2.expect("expected live profile update delta marker");
    assert!(delta.contains("<<SAND_AGENT_PROFILE_UPDATE:v1:"));
    assert!(delta.contains("首席架构师 Alpha"));

    // 4. Verify Execution Guard & summary DTO
    {
        let mut guard = fabushi_host
            .begin_bot_execution(600_001, 700_001, 100_001, "check", None)
            .expect("execution guard");
        assert_eq!(bot_a.execution_state_label(), "running");
        guard.mark_success();
    }
    assert_eq!(bot_a.execution_state_label(), "idle");
    let dto_a = bot_a.to_summary_dto(Some(600_000_001));
    assert_eq!(dto_a.memory_entries_count, 1);
    assert!(dto_a.skills_count >= 1);
    assert_eq!(dto_a.thinking_level, "high");
}

#[test]
fn test_fabushi_provider_sync_propagates_to_all_isolated_bots() {
    let root = TempDir::new().expect("temp dir");
    let host_hermes = make_host_engine(&root);
    let fabushi_host =
        FabushiBotHost::new_ephemeral(Arc::clone(&host_hermes)).expect("ephemeral fabushi host");

    let bot = fabushi_host
        .provision_or_sync_bot(FabushiBotProvisionSpec {
            bot_user_id: 600_010,
            owner_user_id: 100_001,
            username: "CloudSyncBot".to_string(),
            display_name: "Cloud Sync".to_string(),
            description: "Cloud tester".to_string(),
            title: String::new(),
            agent_id: None,
            avatar_shape: None,
            avatar_color: None,
            preferred_model: None,
            thinking_level: None,
            approval_mode: None,
            toolset_preset: None,
            commands: vec![],
            inline_mode_enabled: false,
        })
        .expect("provision bot");

    let custom_cfg = ProviderConfig::openai_compatible(
        "test_cloud",
        "Test Cloud",
        "https://api.example.invalid/v1",
        "sk-test-1234",
        "gpt-4.1-mini",
    );
    host_hermes.providers().register_provider(custom_cfg);
    host_hermes
        .providers()
        .set_active("test_cloud", Some("gpt-4.1-mini"))
        .expect("set host active provider");

    fabushi_host.sync_all_providers();

    let (active_id, active_model, _mode) = bot.hermes().providers().active_info();
    assert_eq!(active_id, "test_cloud");
    assert_eq!(active_model, "gpt-4.1-mini");
}

#[tokio::test]
async fn test_fabushi_teammate_tools_execute_via_hermes_executor() {
    let root = TempDir::new().expect("temp dir");
    let host_hermes = make_host_engine(&root);
    let fabushi_host = FabushiBotHost::new_with_dir(root.path().join("agents"), host_hermes)
        .expect("init fabushi host");
    let recorder = Arc::new(RecordingTeammateHandler::default());
    fabushi_host.set_teammate_handler(recorder.clone());

    let bot_a = fabushi_host
        .provision_or_sync_bot(FabushiBotProvisionSpec {
            bot_user_id: 600_020,
            owner_user_id: 100_001,
            username: "CoordBot".to_string(),
            display_name: "Coordinator".to_string(),
            description: "Coordinates teammates".to_string(),
            title: String::new(),
            agent_id: Some("coord-bot".to_string()),
            avatar_shape: None,
            avatar_color: None,
            preferred_model: None,
            thinking_level: None,
            approval_mode: None,
            toolset_preset: None,
            commands: vec![],
            inline_mode_enabled: false,
        })
        .expect("provision coord bot");

    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = bot_a.hermes().build_tool_context(
        "sess_room_1",
        Some(900_001),
        Some("600020"),
        1,
        None,
        0,
        cancel,
    );

    let send_res = HermesToolExecutor::execute(
        "SendToAgent",
        json!({
            "targetId": "designer-beta",
            "message": "Please inspect button contrast.",
            "priority": true
        }),
        &ctx,
    )
    .await;
    assert!(send_res.success, "SendToAgent error: {}", send_res.output);

    let create_res = HermesToolExecutor::execute(
        "CreateAgent",
        json!({
            "name": "QA Auditor",
            "description": "Verifies regressions",
            "avatarShape": "teardrop",
            "avatarColor": "#0EA5E9"
        }),
        &ctx,
    )
    .await;
    assert!(
        create_res.success,
        "CreateAgent error: {}",
        create_res.output
    );

    let update_res = HermesToolExecutor::execute(
        "UpdateAgent",
        json!({
            "name": "Lead Coordinator"
        }),
        &ctx,
    )
    .await;
    assert!(
        update_res.success,
        "UpdateAgent error: {}",
        update_res.output
    );

    let events = recorder.events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].contains("send:coord-bot->designer-beta:chat=Some(900001):prio=true"));
    assert!(events[1].contains("create:coord-bot:name=QA Auditor"));
    assert!(events[2].contains("update:coord-bot->coord-bot"));
}
