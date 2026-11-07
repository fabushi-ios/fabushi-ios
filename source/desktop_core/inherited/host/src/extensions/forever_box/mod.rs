pub mod disk_pressure;
pub mod disk_pressure_guard;
pub mod extension;
pub mod forever_box_service;
pub mod host_box;
pub mod runner_tools;

pub use disk_pressure::{
    DiskPressureReminderEpisodes, DiskPressureWatch, DiskPressureWatchDeps,
    start_disk_pressure_watch,
};
pub use disk_pressure_guard::{
    DISK_PRESSURE_HEARTBEAT_MS, DISK_PRESSURE_THRESHOLDS, GIB, DiskPressureGuard,
    DiskPressureGuardOptions, DiskPressureLevel, DiskPressureReport,
    DiskPressureTrigger, DiskVolumeRoot, DiskVolumeSample, DiskVolumeSnapshot,
    classify_disk_pressure, read_disk_volume_snapshots,
};
pub use extension::{
    ForeverBoxExtensionOptions, is_host_bundle_auto_update_enabled,
    is_image_auto_update_enabled, start_forever_box_extension,
    start_forever_box_extension_with_telemetry,
};
pub use forever_box_service::{
    ForeverBoxLifecycle, ForeverBoxService, ForeverBoxServiceError,
};
pub use host_box::{
    BoxStatus, BoxWindowStatus, HostBox, HostBoxStatusListener,
};

pub use runner_tools::{ForeverBoxRemoteResourceLifecycle, ForeverBoxRunnerResourcePort};
