use std::fs;use std::sync::{Arc,mpsc,atomic::{AtomicUsize,Ordering}};use std::time::{Duration,SystemTime,UNIX_EPOCH};
use mahayana_host_runtime::workflows::stat_keyed_parse_cache::{StatKeyedParseCache,mtime_tick_could_still_hide_an_edit};
use mahayana_host_runtime::workflows::workflow_library::{GlobalWorkflowLibrary,LEGACY_WORKFLOW_FILENAME,WORKFLOW_FILENAME,WorkflowSpec,WorkflowTrigger,parse_workflow_file,serialize_workflow_file};
use mahayana_host_runtime::workflows::workflow_store::{FileWorkflowStore,agent_has_workflows};
fn root(label:&str)->std::path::PathBuf{let n=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();std::env::temp_dir().join(format!("fabushi-workflow-{label}-{}-{n}",std::process::id()))}
#[test]fn frontmatter_round_trip(){let raw="---\nname: \"Deploy helper\"\ndescription: \"desc\"\nmetadata:\n  source: \"https://example.test/skill.md\"\ntrigger:\n  schedule: \"0 9 * * 1-5\"\n  enabled: false\ncustom: true\n---\n# Body\nDo it.\n";let p=parse_workflow_file(raw).unwrap();assert_eq!(p.spec.name,"Deploy helper");assert_eq!(p.spec.source_ref.as_deref(),Some("https://example.test/skill.md"));assert_eq!(p.spec.trigger.as_ref().unwrap().schedule,"0 9 * * 1-5");assert!(!p.spec.trigger.as_ref().unwrap().is_enabled);let encoded=serialize_workflow_file(&p.spec,&p.data);assert!(encoded.contains("custom: true"));assert!(encoded.contains("# Body"));}
#[test]fn global_library_crud_and_legacy_rename(){let root=root("library");let library=GlobalWorkflowLibrary::new(&root);let spec=WorkflowSpec{name:"Review PR".into(),description:"review".into(),body:"Read the diff".into(),trigger:None,source_ref:None};let first=library.create(&spec).unwrap().unwrap();assert_eq!(first.id,"review-pr");let second=library.create(&spec).unwrap().unwrap();assert_eq!(second.id,"review-pr-2");fs::write(library.folder(&first.id).join("helper.sh"),"echo hi").unwrap();assert_eq!(library.get(&first.id).unwrap().helper_scripts,vec!["helper.sh"]);assert!(library.remove(&second.id).unwrap());let legacy=library.folder("legacy");fs::create_dir_all(&legacy).unwrap();fs::write(legacy.join(LEGACY_WORKFLOW_FILENAME),"---\nname: Legacy\n---\nbody\n").unwrap();library.rename_legacy_recipe_files();assert!(legacy.join(WORKFLOW_FILENAME).is_file());let _=fs::remove_dir_all(root);}
#[test]fn store_projects_automation_and_migrates_legacy(){let root=root("store");let agent=root.join("agents/a");let global=root.join("workflows");let legacy=agent.join("workflows/old");fs::create_dir_all(&legacy).unwrap();fs::write(legacy.join(LEGACY_WORKFLOW_FILENAME),"---\nname: Old\n---\nLegacy body\n").unwrap();assert!(agent_has_workflows(&agent));let store=FileWorkflowStore::new(&agent,&global);assert!(global.join("old/SKILL.md").is_file());let created=store.create(&WorkflowSpec{name:"Daily".into(),description:String::new(),body:"Do daily".into(),trigger:Some(WorkflowTrigger{schedule:"0 9 * * 1-5".into(),is_enabled:true}),source_ref:None}).unwrap().unwrap();assert_eq!(created.source,"automation");assert!(store.list_all().iter().any(|w|w.id==created.id));let _=fs::remove_dir_all(root);}
#[test]fn racy_mtime_boundary(){assert!(mtime_tick_could_still_hide_an_edit(9_000,10_999));assert!(!mtime_tick_could_still_hide_an_edit(9_000,11_000));}


#[test]
fn workflow_store_routes_enablement_mutations_through_agent_owner() {
    let root = root("enablement-owner");
    let agent = root.join("agents/a");
    let global = root.join("global-workflows");
    let store = FileWorkflowStore::new(&agent, &global);
    let created = store
        .create(&WorkflowSpec {
            name: "Enablement proof".into(),
            description: String::new(),
            body: "Do the thing".into(),
            trigger: None,
            source_ref: None,
        })
        .expect("create workflow")
        .expect("workflow record");

    assert!(created.is_enabled_for_agent);
    let disabled = store
        .set_enabled_for_agent(&created.id, false)
        .expect("disable workflow")
        .expect("disabled record");
    assert!(!disabled.is_enabled_for_agent);
    assert!(!store.list().iter().any(|row| row.id == created.id));

    let enablement_path = agent.join("enabled-workflows.json");
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&enablement_path).expect("enablement file"),
    )
    .expect("enablement json");
    assert_eq!(value["disabled"], serde_json::json!([created.id.clone()]));

    let reenabled = store
        .set_enabled_for_agent(&created.id, true)
        .expect("reenable workflow")
        .expect("reenabled record");
    assert!(reenabled.is_enabled_for_agent);
    assert!(store.list().iter().any(|row| row.id == created.id));

    store
        .set_enabled_for_agent(&created.id, false)
        .expect("disable before remove");
    assert!(store.remove(&created.id).expect("remove workflow"));
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&enablement_path).expect("enablement file after remove"),
    )
    .expect("enablement json");
    assert_eq!(value["disabled"], serde_json::json!([]));

    let _ = fs::remove_dir_all(root);
}


#[test]
fn local_skill_discovery_and_port_match_frozen_sources() {
    use mahayana_host_runtime::workflows::workflow_store::discover_local_skill_files;

    let root = root("local-skills");
    let home = root.join("home");
    let cwd = root.join("cwd");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(cwd.join(".cursor/rules")).unwrap();
    fs::write(home.join("CLAUDE.md"), "# Home memory\n").unwrap();
    fs::write(cwd.join("AGENTS.md"), "# Agents memory\n").unwrap();
    fs::write(cwd.join(".cursor/rules/REVIEW.MDC"), "# Review rule\n").unwrap();
    fs::write(cwd.join(".cursor/rules/ignore.txt"), "ignore").unwrap();

    let discovered = discover_local_skill_files(&home, &cwd);
    assert_eq!(discovered.len(), 3);
    assert!(discovered.iter().any(|row| row.fallback_name == "Claude memory"));
    assert!(discovered.iter().any(|row| row.fallback_name == "Agents memory"));
    assert!(discovered.iter().any(|row| row.fallback_name == "REVIEW"));
    assert!(!discovered.iter().any(|row| row.label.ends_with("ignore.txt")));

    let store = FileWorkflowStore::new(root.join("agent"), root.join("global"));
    let result = store.port_local_skills(&home, &cwd).expect("port local skills");
    assert_eq!(result.imported.len(), 3);
    assert!(result.skipped.is_empty());
    assert_eq!(store.list_all().len(), 3);
    assert!(store
        .list_all()
        .iter()
        .all(|workflow| workflow.source_ref.as_deref().is_some()));

    let _ = fs::remove_dir_all(root);
}


#[test]
fn workflow_library_watches_external_and_atomic_edits(){
 let root=root("watch-library");let library=GlobalWorkflowLibrary::new(&root);let(tx,rx)=mpsc::channel();
 library.set_on_change(Some(Arc::new(move||{let _=tx.send(());})));
 let spec=WorkflowSpec{name:"Watched".into(),description:"first".into(),body:"Body one".into(),trigger:None,source_ref:None};
 let record=library.create(&spec).unwrap().unwrap();
 rx.recv_timeout(Duration::from_secs(3)).expect("atomic library notification");
 assert_eq!(library.get(&record.id).unwrap().body,"Body one");
 while rx.try_recv().is_ok(){}
 let path=library.path(&record.id);
 fs::write(&path,"---\nname: Watched\ndescription: external\n---\nBody two\n").unwrap();
 rx.recv_timeout(Duration::from_secs(3)).expect("external library notification");
 let updated=library.get(&record.id).unwrap();
 assert_eq!(updated.description,"external");assert_eq!(updated.body,"Body two");
 while rx.try_recv().is_ok(){}
 fs::write(library.folder(&record.id).join("helper.py"),"print('ok')").unwrap();
 rx.recv_timeout(Duration::from_secs(3)).expect("helper directory notification");
 assert_eq!(library.get(&record.id).unwrap().helper_scripts,vec!["helper.py"]);
 library.set_on_change(None);let _=fs::remove_dir_all(root);
}

#[test]
fn stat_parse_cache_reuses_stable_fingerprints_and_invalidates_changes(){
 let root=root("stat-cache");fs::create_dir_all(&root).unwrap();let path=root.join("value.txt");fs::write(&path,"one").unwrap();
 let parses=Arc::new(AtomicUsize::new(0));let cache=StatKeyedParseCache::with_now_ms(2,Arc::new(||u128::MAX));
 let p=Arc::clone(&parses);let read_path=path.clone();let first=cache.read(std::slice::from_ref(&path),move||{p.fetch_add(1,Ordering::SeqCst);fs::read_to_string(&read_path).unwrap()}).unwrap();
 assert_eq!(first,"one");
 let p=Arc::clone(&parses);let path2=path.clone();let read_path=path2.clone();let second=cache.read(std::slice::from_ref(&path2),move||{p.fetch_add(1,Ordering::SeqCst);fs::read_to_string(&read_path).unwrap()}).unwrap();
 assert_eq!(second,"one");assert_eq!(parses.load(Ordering::SeqCst),1);
 fs::write(&path,"two-two").unwrap();
 let p=Arc::clone(&parses);let path3=path.clone();let read_path=path3.clone();let third=cache.read(std::slice::from_ref(&path3),move||{p.fetch_add(1,Ordering::SeqCst);fs::read_to_string(&read_path).unwrap()}).unwrap();
 assert_eq!(third,"two-two");assert_eq!(parses.load(Ordering::SeqCst),2);
 fs::remove_file(&path).unwrap();
 let p=Arc::clone(&parses);let path4=path.clone();
 assert!(cache.read(std::slice::from_ref(&path4),move||{p.fetch_add(1,Ordering::SeqCst);"missing".to_string()}).is_none());
 assert_eq!(parses.load(Ordering::SeqCst),2);
 let _=fs::remove_dir_all(root);
}


#[test]
fn managed_and_plugin_skills_are_aggregated_with_frozen_precedence() {
    use mahayana_host_runtime::extensions::managed_setup::managed_skills_cache::{
        ManagedSkill, get_managed_skills_dir, write_managed_skills_cache,
    };
    use mahayana_host_runtime::extensions::mcp::plugin_skills_cache::{
        PluginSkillsCacheWriteIndex, get_plugin_skills_dir, write_plugin_skills_cache,
    };

    let root = root("managed-plugin-aggregation");
    let agent = root.join("agents/a");
    let global = root.join("workflows");
    fs::create_dir_all(&agent).unwrap();

    let managed_dir = get_managed_skills_dir(&root);
    write_managed_skills_cache(
        &managed_dir,
        &[
            ManagedSkill {
                id: "managed-one".into(),
                name: "Managed One".into(),
                description: "managed description".into(),
                body: "Managed body".into(),
            },
            ManagedSkill {
                id: "user-wins".into(),
                name: "Managed Shadow".into(),
                description: String::new(),
                body: "Should be shadowed".into(),
            },
        ],
        1234.0,
    )
    .unwrap();

    let plugin_dir = get_plugin_skills_dir(&root);
    let own_skill_dir = root.join("plugins/own");
    let foreign_skill_dir = root.join("plugins/foreign");
    fs::create_dir_all(&own_skill_dir).unwrap();
    fs::create_dir_all(&foreign_skill_dir).unwrap();
    let own_path = own_skill_dir.join("SKILL.md");
    let foreign_path = foreign_skill_dir.join("SKILL.md");
    fs::write(
        &own_path,
        "---\nname: \"Own Plugin\"\ndescription: \"plugin desc\"\ndisable-model-invocation: true\ncustom-key: \"preserve-me\"\n---\nPlugin body\n",
    )
    .unwrap();
    fs::write(own_skill_dir.join("helper.sh"), "echo helper").unwrap();
    fs::write(
        &foreign_path,
        "---\nname: \"Foreign Plugin\"\n---\nForeign body\n",
    )
    .unwrap();

    let skills = vec![
        serde_json::json!({
            "id":"plugin-own-1",
            "pluginId":"plugin-own",
            "pluginName":"Own",
            "name":"Own Plugin",
            "description":"plugin desc",
            "filePath":own_path.to_string_lossy(),
            "pluginVersion":"1",
            "installPath":own_skill_dir.to_string_lossy(),
            "skillRelativePath":"SKILL.md",
            "publisherUserId":42
        }),
        serde_json::json!({
            "id":"plugin-foreign-1",
            "pluginId":"plugin-foreign",
            "pluginName":"Foreign",
            "name":"Foreign Plugin",
            "description":"",
            "filePath":foreign_path.to_string_lossy(),
            "pluginVersion":"1",
            "installPath":foreign_skill_dir.to_string_lossy(),
            "skillRelativePath":"SKILL.md",
            "publisherUserId":7
        }),
        serde_json::json!({
            "id":"managed-one",
            "pluginId":"plugin-shadowed",
            "pluginName":"Shadowed",
            "name":"Plugin Shadow",
            "description":"",
            "filePath":foreign_path.to_string_lossy(),
            "pluginVersion":"1",
            "installPath":foreign_skill_dir.to_string_lossy(),
            "skillRelativePath":"SKILL.md",
            "publisherUserId":42
        }),
    ];
    write_plugin_skills_cache(
        &plugin_dir,
        &PluginSkillsCacheWriteIndex {
            current_user_id: Some(Some(42)),
            skills,
            auth_blocked: None,
        },
        || 2345.0,
    )
    .unwrap();

    let store = FileWorkflowStore::new(&agent, &global);
    let user = store
        .create(&WorkflowSpec {
            name: "User Wins".into(),
            description: String::new(),
            body: "User body".into(),
            trigger: None,
            source_ref: None,
        })
        .unwrap()
        .unwrap();
    assert_eq!(user.id, "user-wins");

    let all = store.list_all();
    assert_eq!(all.iter().filter(|row| row.id == "user-wins").count(), 1);
    assert_eq!(
        all.iter().find(|row| row.id == "user-wins").unwrap().source,
        "workflow"
    );
    assert_eq!(all.iter().filter(|row| row.id == "managed-one").count(), 1);
    assert_eq!(
        all.iter().find(|row| row.id == "managed-one").unwrap().source,
        "managed"
    );

    let own = all.iter().find(|row| row.id == "plugin-own-1").unwrap();
    assert_eq!(own.source, "plugin");
    assert_eq!(own.plugin_id.as_deref(), Some("plugin-own"));
    assert!(own.published_by_current_user);
    assert!(own.disable_model_invocation);
    assert_eq!(own.helper_scripts, vec!["helper.sh"]);

    let foreign = all.iter().find(|row| row.id == "plugin-foreign-1").unwrap();
    assert!(!foreign.published_by_current_user);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn only_current_user_plugin_skill_can_be_edited_and_frontmatter_is_preserved() {
    use mahayana_host_runtime::extensions::mcp::plugin_skills_cache::{
        PluginSkillsCacheWriteIndex, get_plugin_skills_dir, write_plugin_skills_cache,
    };

    let root = root("plugin-edit");
    let agent = root.join("agents/a");
    let global = root.join("workflows");
    let plugin_dir = get_plugin_skills_dir(&root);
    let own_dir = root.join("plugins/own");
    let foreign_dir = root.join("plugins/foreign");
    fs::create_dir_all(&own_dir).unwrap();
    fs::create_dir_all(&foreign_dir).unwrap();
    let own_path = own_dir.join("SKILL.md");
    let foreign_path = foreign_dir.join("SKILL.md");
    fs::write(
        &own_path,
        "---\nname: \"Owned\"\ndisable-model-invocation: true\ncustom-key: \"keep\"\n---\nOld body\n",
    )
    .unwrap();
    fs::write(&foreign_path, "---\nname: \"Foreign\"\n---\nOld foreign\n").unwrap();

    write_plugin_skills_cache(
        &plugin_dir,
        &PluginSkillsCacheWriteIndex {
            current_user_id: Some(Some(99)),
            skills: vec![
                serde_json::json!({
                    "id":"owned-skill-1","pluginId":"owned","pluginName":"Owned",
                    "name":"Owned","description":"","filePath":own_path.to_string_lossy(),
                    "pluginVersion":"1","installPath":own_dir.to_string_lossy(),
                    "skillRelativePath":"SKILL.md","publisherUserId":99
                }),
                serde_json::json!({
                    "id":"foreign-skill-1","pluginId":"foreign","pluginName":"Foreign",
                    "name":"Foreign","description":"","filePath":foreign_path.to_string_lossy(),
                    "pluginVersion":"1","installPath":foreign_dir.to_string_lossy(),
                    "skillRelativePath":"SKILL.md","publisherUserId":5
                }),
            ],
            auth_blocked: None,
        },
        || 1.0,
    )
    .unwrap();

    let store = FileWorkflowStore::new(&agent, &global);
    let updated = store
        .update(
            "owned-skill-1",
            &WorkflowSpec {
                name: "Owned Updated".into(),
                description: "new desc".into(),
                body: "New body".into(),
                trigger: None,
                source_ref: None,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(updated.name, "Owned Updated");
    assert_eq!(updated.body, "New body");
    let raw = fs::read_to_string(&own_path).unwrap();
    assert!(raw.contains("disable-model-invocation: true"));
    assert!(raw.contains("custom-key: \"keep\""));
    assert!(raw.contains("New body"));

    let denied = store
        .update(
            "foreign-skill-1",
            &WorkflowSpec {
                name: "Denied".into(),
                description: String::new(),
                body: "Denied body".into(),
                trigger: None,
                source_ref: None,
            },
        )
        .unwrap();
    assert!(denied.is_none());
    assert!(fs::read_to_string(&foreign_path).unwrap().contains("Old foreign"));
    assert!(!store.remove("owned-skill-1").unwrap());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn workflow_store_watches_managed_and_plugin_skill_caches() {
    use mahayana_host_runtime::extensions::managed_setup::managed_skills_cache::{
        ManagedSkill, get_managed_skills_dir, write_managed_skills_cache,
    };
    use mahayana_host_runtime::extensions::mcp::plugin_skills_cache::{
        PluginSkillsCacheWriteIndex, get_plugin_skills_dir, write_plugin_skills_cache,
    };

    let root = root("skill-watchers");
    let agent = root.join("agents/a");
    let global = root.join("workflows");
    let store = FileWorkflowStore::new(&agent, &global);
    let (tx, rx) = mpsc::channel();
    store.set_on_change(Some(Arc::new(move || {
        let _ = tx.send(());
    })));

    write_managed_skills_cache(
        get_managed_skills_dir(&root),
        &[ManagedSkill {
            id: "managed-watch".into(),
            name: "Managed Watch".into(),
            description: String::new(),
            body: "body".into(),
        }],
        1.0,
    )
    .unwrap();
    rx.recv_timeout(Duration::from_secs(3))
        .expect("managed cache watcher");

    while rx.try_recv().is_ok() {}
    write_plugin_skills_cache(
        get_plugin_skills_dir(&root),
        &PluginSkillsCacheWriteIndex {
            current_user_id: Some(Some(1)),
            skills: Vec::new(),
            auth_blocked: None,
        },
        || 2.0,
    )
    .unwrap();
    rx.recv_timeout(Duration::from_secs(3))
        .expect("plugin cache watcher");

    store.set_on_change(None);
    let _ = fs::remove_dir_all(root);
}
