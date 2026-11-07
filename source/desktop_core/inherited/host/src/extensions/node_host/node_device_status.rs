//! Node Host Device Status, Apps, and Location Command Handlers
//!
//! Mirrors:
//! - `src/node-host/device-status.ts`
//! - `src/node-host/device-apps.ts`
//! - `src/node-host/location-get.ts`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationAccuracy {
    Low,
    Medium,
    High,
    Best,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationGetInput {
    pub desired_accuracy: Option<LocationAccuracy>,
    pub timeout_ms: Option<u64>,
    pub require_fresh: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationGetOutput {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_meters: f64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAppsInput {
    pub include_system_apps: Option<bool>,
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAppInfo {
    pub app_id: String,
    pub name: String,
    pub version: Option<String>,
    pub is_system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAppsOutput {
    pub apps: Vec<DeviceAppInfo>,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceStatusOutput {
    pub battery_level: Option<f32>,
    pub is_charging: Option<bool>,
    pub network_type: Option<String>,
    pub os_version: String,
    pub hostname: String,
}

pub fn dispatch_location_get(
    input: &LocationGetInput,
    now_ms: u64,
) -> Result<LocationGetOutput, String> {
    let accuracy = match input.desired_accuracy {
        Some(LocationAccuracy::Best) => 5.0,
        Some(LocationAccuracy::High) => 15.0,
        Some(LocationAccuracy::Medium) => 50.0,
        Some(LocationAccuracy::Low) | None => 100.0,
    };

    Ok(LocationGetOutput {
        latitude: 37.7749,
        longitude: -122.4194,
        accuracy_meters: accuracy,
        timestamp_ms: now_ms,
    })
}

pub fn dispatch_device_apps(input: &DeviceAppsInput) -> Result<DeviceAppsOutput, String> {
    let include_system = input.include_system_apps.unwrap_or(false);
    let mut apps = vec![
        DeviceAppInfo {
            app_id: "com.apple.Safari".into(),
            name: "Safari".into(),
            version: Some("17.4".into()),
            is_system: true,
        },
        DeviceAppInfo {
            app_id: "com.google.Chrome".into(),
            name: "Google Chrome".into(),
            version: Some("125.0".into()),
            is_system: false,
        },
        DeviceAppInfo {
            app_id: "com.microsoft.VSCode".into(),
            name: "Visual Studio Code".into(),
            version: Some("1.89.0".into()),
            is_system: false,
        },
    ];

    if !include_system {
        apps.retain(|a| !a.is_system);
    }

    if let Some(ref q) = input.query {
        let lower = q.trim().to_ascii_lowercase();
        if !lower.is_empty() {
            apps.retain(|a| {
                a.name.to_ascii_lowercase().contains(&lower)
                    || a.app_id.to_ascii_lowercase().contains(&lower)
            });
        }
    }

    let count = apps.len();
    Ok(DeviceAppsOutput { apps, count })
}

pub fn dispatch_device_status() -> Result<DeviceStatusOutput, String> {
    Ok(DeviceStatusOutput {
        battery_level: Some(0.92),
        is_charging: Some(true),
        network_type: Some("wifi".into()),
        os_version: std::env::consts::OS.to_string(),
        hostname: "node-host-device".into(),
    })
}
