use std::sync::Arc;

use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::settings::settings_service::{
    SettingsService, SettingsSubscription,
};

use super::box_wallpaper_commands::{BoxWallpaperCommands, WallpaperLog};
use super::wallpaper_service::{
    WallpaperPainter, WallpaperPlanResolver, WallpaperSchedulerDependencies,
    WallpaperToneScheduler,
};

pub const WALLPAPER_DEPENDENCIES: &[HostExtensionId] = &[HostExtensionId::Settings];

pub fn wallpaper_extension_id() -> HostExtensionId {
    HostExtensionId::Wallpaper
}

type TimeZoneSubscription =
    SettingsSubscription<dyn Fn(Option<String>) + Send + Sync + 'static>;

pub struct HostWallpaperExtension {
    enabled: bool,
    scheduler: Option<WallpaperToneScheduler>,
    _time_zone_subscription: Option<TimeZoneSubscription>,
}

impl HostWallpaperExtension {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            scheduler: None,
            _time_zone_subscription: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn stop(&mut self) {
        if let Some(scheduler) = self.scheduler.take() {
            scheduler.stop();
        }
        self._time_zone_subscription.take();
        self.enabled = false;
    }
}

impl Drop for HostWallpaperExtension {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_wallpaper_extension(
    settings: Arc<SettingsService>,
    log: WallpaperLog,
) -> HostWallpaperExtension {
    let commands = Arc::new(BoxWallpaperCommands::production(
        settings.get_settings_path().to_path_buf(),
        Arc::clone(&log),
    ));
    if !commands.is_available() {
        return HostWallpaperExtension::disabled();
    }

    let resolve_commands = Arc::clone(&commands);
    let resolve_plan: WallpaperPlanResolver =
        Arc::new(move || resolve_commands.resolve_plan());
    let paint_commands = Arc::clone(&commands);
    let paint: WallpaperPainter = Arc::new(move || paint_commands.paint());
    let scheduler = WallpaperToneScheduler::new(WallpaperSchedulerDependencies {
        resolve_plan,
        paint,
        log,
    });
    let scheduler_for_time_zone = scheduler.clone();
    let subscription = settings.subscribe_to_user_time_zone(Arc::new(move |_| {
        scheduler_for_time_zone.begin_sync();
    }));
    scheduler.start();

    HostWallpaperExtension {
        enabled: true,
        scheduler: Some(scheduler),
        _time_zone_subscription: Some(subscription),
    }
}
