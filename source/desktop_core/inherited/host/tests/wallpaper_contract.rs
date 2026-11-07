use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::wallpaper::box_wallpaper_commands::{
    BoxWallpaperCommandOptions, BoxWallpaperCommands, CommandRunOptions, DisplayOwner,
    WallpaperLog, as_display_owner, parse_tone_plan,
};
use mahayana_host_runtime::extensions::wallpaper::extension::{
    WALLPAPER_DEPENDENCIES, wallpaper_extension_id,
};
use mahayana_host_runtime::extensions::wallpaper::wallpaper_service::{
    WallpaperSchedulerDependencies, WallpaperToneScheduler, remaining_wallpaper_delay_ms,
};

#[test]
fn wallpaper_tone_plan_matches_frozen_two_token_contract() {
    let plan = parse_tone_plan("night 17\n").expect("valid tone plan");
    assert_eq!(plan.tone, "night");
    assert_eq!(plan.ms_until_next_boundary, 17_000);

    assert!(parse_tone_plan("Night 17").is_none());
    assert!(parse_tone_plan("night -1").is_none());
    assert!(parse_tone_plan("night 1 extra").is_none());
    assert!(parse_tone_plan("").is_none());
}

#[test]
fn display_owner_projection_only_changes_identity_from_root() {
    let owner = DisplayOwner {
        display: ":7".into(),
        uid: 501,
        gid: 20,
    };
    assert_eq!(
        as_display_owner(&owner, Some(0), Some(0)),
        CommandRunOptions {
            uid: Some(501),
            gid: Some(20),
        }
    );
    assert_eq!(
        as_display_owner(&owner, Some(501), Some(20)),
        CommandRunOptions::default()
    );
    assert_eq!(
        as_display_owner(&owner, Some(1000), Some(1000)),
        CommandRunOptions::default()
    );
    assert_eq!(
        as_display_owner(&owner, None, None),
        CommandRunOptions::default()
    );
}

#[test]
fn wallpaper_commands_use_tone_helper_and_paint_live_displays() {
    let calls = Arc::new(Mutex::new(Vec::<(String, Vec<String>, CommandRunOptions)>::new()));
    let captured = Arc::clone(&calls);
    let runner = Arc::new(move |file: &std::path::Path, args: &[String], options: CommandRunOptions| {
        captured.lock().expect("calls").push((
            file.to_string_lossy().into_owned(),
            args.to_vec(),
            options,
        ));
        if file == std::path::Path::new("/test/node") {
            Ok("dusk 9\n".to_string())
        } else {
            Ok(String::new())
        }
    });
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_logs = Arc::clone(&logs);
    let log: WallpaperLog = Arc::new(move |message| {
        captured_logs.lock().expect("logs").push(message.to_string());
    });
    let commands = BoxWallpaperCommands::new(BoxWallpaperCommandOptions {
        settings_path: PathBuf::from("/test/settings.json"),
        log,
        node_path: PathBuf::from("/test/node"),
        tone_script_path: PathBuf::from("/test/tone.mjs"),
        wallpaper_command_path: PathBuf::from("/test/wallpaper"),
        x_socket_dir: PathBuf::from("/test/x11"),
        run_command: runner,
        read_displays: Arc::new(|_| Ok(vec![
            DisplayOwner { display: ":2".into(), uid: 501, gid: 20 },
            DisplayOwner { display: ":7".into(), uid: 502, gid: 21 },
        ])),
        exists: Arc::new(|_| true),
    });

    assert!(commands.is_available());
    let plan = commands.resolve_plan().expect("resolved plan");
    assert_eq!(plan.tone, "dusk");
    assert_eq!(plan.ms_until_next_boundary, 9_000);
    commands.paint();

    let calls = calls.lock().expect("calls");
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].0, "/test/node");
    assert_eq!(
        calls[0].1,
        vec!["/test/tone.mjs".to_string(), "/test/settings.json".to_string()]
    );
    assert_eq!(calls[1].0, "/test/wallpaper");
    assert_eq!(calls[1].1, vec!["paint".to_string(), ":2".to_string()]);
    assert_eq!(calls[2].1, vec!["paint".to_string(), ":7".to_string()]);
}

#[test]
fn scheduler_recomputes_on_explicit_sync_and_subtracts_paint_time() {
    assert_eq!(
        remaining_wallpaper_delay_ms(1_000, Duration::from_millis(275)),
        725
    );
    assert_eq!(
        remaining_wallpaper_delay_ms(100, Duration::from_millis(150)),
        0
    );

    let resolves = Arc::new(AtomicUsize::new(0));
    let resolve_count = Arc::clone(&resolves);
    let paints = Arc::new(AtomicUsize::new(0));
    let paint_count = Arc::clone(&paints);
    let scheduler = WallpaperToneScheduler::new(WallpaperSchedulerDependencies {
        resolve_plan: Arc::new(move || {
            resolve_count.fetch_add(1, Ordering::SeqCst);
            Some(mahayana_host_runtime::extensions::wallpaper::box_wallpaper_commands::WallpaperPlan {
                tone: "day".into(),
                ms_until_next_boundary: 5_000,
            })
        }),
        paint: Arc::new(move || {
            paint_count.fetch_add(1, Ordering::SeqCst);
        }),
        log: Arc::new(|_| {}),
    });
    scheduler.start();

    wait_until(|| paints.load(Ordering::SeqCst) >= 1);
    let before = scheduler.generation();
    scheduler.begin_sync();
    wait_until(|| paints.load(Ordering::SeqCst) >= 2);
    assert!(scheduler.generation() > before);
    assert!(resolves.load(Ordering::SeqCst) >= 2);
    scheduler.stop();
}

#[test]
fn wallpaper_extension_preserves_grok_id_and_settings_dependency() {
    assert_eq!(wallpaper_extension_id(), HostExtensionId::Wallpaper);
    assert_eq!(WALLPAPER_DEPENDENCIES, &[HostExtensionId::Settings]);
}

fn wait_until(predicate: impl Fn() -> bool) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("condition did not become true");
}
