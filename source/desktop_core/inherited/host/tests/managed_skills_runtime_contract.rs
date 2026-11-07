use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mahayana_host_runtime::extensions::managed_setup::cursor_skills_marketplace::{
    decode_managed_skills, decode_marketplace_plugins, skill_catalog_from_plugins,
};
use mahayana_host_runtime::extensions::managed_setup::managed_skills_cache::{
    read_managed_skills_cache,
};
use mahayana_host_runtime::extensions::managed_setup::managed_skills_service::{
    ManagedSkillsRefreshTrigger, SandManagedSkillsService,
};
use mahayana_host_runtime::extensions::managed_setup::sand_managed_skills::FetchedManagedSkill;
use uuid::Uuid;

fn scratch(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fabushi-{name}-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        if value < 0x80 {
            out.push(value as u8);
            return out;
        }
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
}

fn field_varint(field: u64, value: u64) -> Vec<u8> {
    let mut out = varint(field << 3);
    out.extend(varint(value));
    out
}

fn field_bytes(field: u64, value: &[u8]) -> Vec<u8> {
    let mut out = varint((field << 3) | 2);
    out.extend(varint(value.len() as u64));
    out.extend(value);
    out
}

fn field_string(field: u64, value: &str) -> Vec<u8> {
    field_bytes(field, value.as_bytes())
}

#[test]
fn managed_skills_decoder_preserves_optional_enabled_default() {
    let mut enabled = Vec::new();
    enabled.extend(field_string(1, "alpha"));
    enabled.extend(field_string(2, "Alpha desc"));
    enabled.extend(field_string(3, "Alpha body"));
    enabled.extend(field_varint(7, 0));

    let mut default_enabled = Vec::new();
    default_enabled.extend(field_string(1, "beta"));
    default_enabled.extend(field_string(3, "Beta body"));

    let mut response = Vec::new();
    response.extend(field_bytes(1, &enabled));
    response.extend(field_bytes(1, &default_enabled));

    let decoded = decode_managed_skills(&response).expect("decode managed skills");
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].id, "alpha");
    assert!(!decoded[0].enabled);
    assert_eq!(decoded[1].id, "beta");
    assert!(decoded[1].enabled);
}

#[test]
fn marketplace_decoder_builds_deduped_sorted_skill_catalog() {
    let mut publisher = Vec::new();
    publisher.extend(field_string(10, "https://cdn.example/publisher.png"));

    let mut skill_zeta = Vec::new();
    skill_zeta.extend(field_string(1, "Zeta"));
    skill_zeta.extend(field_string(2, "Zeta desc"));
    skill_zeta.extend(field_string(4, "https://example/zeta.git"));

    let mut skill_alpha = Vec::new();
    skill_alpha.extend(field_string(1, "Alpha"));
    skill_alpha.extend(field_string(4, "https://example/alpha.git"));

    let mut first = Vec::new();
    first.extend(field_varint(1, 42));
    first.extend(field_string(2, "publisher-slug"));
    first.extend(field_string(3, "Publisher"));
    first.extend(field_string(10, "https://cdn.example/plugin.png"));
    first.extend(field_bytes(17, &publisher));
    first.extend(field_bytes(28, &skill_zeta));
    first.extend(field_bytes(28, &skill_alpha));

    let mut duplicate_skill = Vec::new();
    duplicate_skill.extend(field_string(1, "alpha"));
    duplicate_skill.extend(field_string(4, "https://example/duplicate.git"));
    let mut second = Vec::new();
    second.extend(field_varint(1, 43));
    second.extend(field_string(2, "second"));
    second.extend(field_bytes(28, &duplicate_skill));

    let mut response = Vec::new();
    response.extend(field_bytes(1, &first));
    response.extend(field_bytes(1, &second));

    let plugins = decode_marketplace_plugins(&response).expect("decode marketplace");
    let catalog = skill_catalog_from_plugins(&plugins);
    assert_eq!(catalog.len(), 2);
    assert_eq!(catalog[0].name, "Alpha");
    assert_eq!(catalog[0].publisher, "Publisher");
    assert_eq!(
        catalog[0].icon_url.as_deref(),
        Some("https://cdn.example/publisher.png")
    );
    assert_eq!(catalog[1].name, "Zeta");
    assert_eq!(catalog[1].id, "plugin:42:Zeta");
}

#[test]
fn managed_skills_service_refreshes_cache_and_ensure_skill_is_on_demand() {
    let root = scratch("managed-skills-service");
    let cache_dir = root.join("managed-skills");
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetches_for_closure = Arc::clone(&fetches);
    let service = SandManagedSkillsService::new(
        cache_dir.clone(),
        Arc::new(move || {
            fetches_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(vec![FetchedManagedSkill {
                id: "alpha".into(),
                description: "Alpha".into(),
                content: "Do alpha work.".into(),
                enabled: true,
            }])
        }),
        None,
    );

    assert!(service.ensure_skill("alpha"));
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert!(service.ensure_skill("alpha"));
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let cache = read_managed_skills_cache(&cache_dir).expect("managed cache");
    assert_eq!(cache.skills.len(), 1);
    assert_eq!(cache.skills[0].id, "alpha");

    service.refresh(ManagedSkillsRefreshTrigger::AuthChange);
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
    service.dispose();
    service.refresh(ManagedSkillsRefreshTrigger::OnDemand);
    assert_eq!(fetches.load(Ordering::SeqCst), 2);

    let _ = fs::remove_dir_all(root);
}
