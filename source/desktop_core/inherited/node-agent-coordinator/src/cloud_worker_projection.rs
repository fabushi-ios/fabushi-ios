use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CLOUD_WORKERS_STATUS_METHOD: &str = "cloudWorkers.status";
pub const CLOUD_WORKERS_CAPABILITIES_METHOD: &str = "cloudWorkers.capabilities";
pub const CLOUD_WORKERS_DOCTOR_METHOD: &str = "cloudWorkers.doctor";
pub const CLOUD_WORKERS_STATUS_EVENT_FAMILY: &str = "cloud-workers.status";
const MAX_PROJECTION_TEXT_CHARS: usize = 256;
const MAX_CAPABILITY_PROVIDERS: usize = 64;

fn validate_projection_text(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.chars().count() > MAX_PROJECTION_TEXT_CHARS {
        return Err(format!("{field} exceeds {MAX_PROJECTION_TEXT_CHARS} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloudWorkerCoordinatorCommand {
    Status,
    Capabilities,
    Doctor,
}

impl CloudWorkerCoordinatorCommand {
    pub const fn method(self) -> &'static str {
        match self {
            Self::Status => CLOUD_WORKERS_STATUS_METHOD,
            Self::Capabilities => CLOUD_WORKERS_CAPABILITIES_METHOD,
            Self::Doctor => CLOUD_WORKERS_DOCTOR_METHOD,
        }
    }

    pub fn from_method(method: &str) -> Option<Self> {
        match method {
            CLOUD_WORKERS_STATUS_METHOD => Some(Self::Status),
            CLOUD_WORKERS_CAPABILITIES_METHOD => Some(Self::Capabilities),
            CLOUD_WORKERS_DOCTOR_METHOD => Some(Self::Doctor),
            _ => None,
        }
    }

    pub fn args(self) -> Value {
        match self {
            Self::Status | Self::Capabilities | Self::Doctor => Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloudWorkerCoordinatorStatus {
    Disabled,
    Enabled,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerStatusProjection {
    pub owner: String,
    pub status: CloudWorkerCoordinatorStatus,
    #[serde(rename = "providerReady")]
    pub provider_ready: bool,
    #[serde(rename = "productionMutation")]
    pub production_mutation: bool,
    #[serde(default, rename = "statePersisted")]
    pub state_persisted: bool,
    #[serde(default, rename = "leaseCount")]
    pub lease_count: usize,
    #[serde(default, rename = "warmImageCount")]
    pub warm_image_count: usize,
    #[serde(default, rename = "bootstrapCount")]
    pub bootstrap_count: usize,
    #[serde(default, rename = "artifactCount")]
    pub artifact_count: usize,
    #[serde(default, rename = "migrationReceiptCount")]
    pub migration_receipt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerCapabilityProvider {
    #[serde(rename = "providerId")]
    pub provider_id: String,
    #[serde(rename = "pluginId")]
    pub plugin_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerCapabilitiesProjection {
    pub owner: String,
    #[serde(rename = "metadataOnly")]
    pub metadata_only: bool,
    pub providers: Vec<CloudWorkerCapabilityProvider>,
}

impl CloudWorkerCapabilitiesProjection {
    pub fn validate(&self) -> Result<(), String> {
        if self.owner != "cloud-workers" {
            return Err(format!("owner must be cloud-workers, got {:?}", self.owner));
        }
        if !self.metadata_only {
            return Err("cloud worker capabilities must remain metadata-only".into());
        }
        if self.providers.len() > MAX_CAPABILITY_PROVIDERS {
            return Err(format!(
                "cloud worker capabilities exceed {MAX_CAPABILITY_PROVIDERS} providers"
            ));
        }
        let mut identities = std::collections::HashSet::new();
        for provider in &self.providers {
            validate_projection_text("providerId", &provider.provider_id)?;
            validate_projection_text("pluginId", &provider.plugin_id)?;
            if let Some(label) = &provider.label {
                validate_projection_text("label", label)?;
            }
            if !identities.insert((&provider.provider_id, &provider.plugin_id)) {
                return Err("cloud worker capabilities contain a duplicate provider identity".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerDoctorProjectionStatus {
    InspectionOnly,
    Pending,
    Completed,
    Failed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerDoctorProjection {
    pub status: CloudWorkerDoctorProjectionStatus,
    #[serde(rename = "mutationStarted")]
    pub mutation_started: bool,
    #[serde(default, rename = "diagnosticCode")]
    pub diagnostic_code: Option<String>,
    #[serde(default, rename = "statePersisted")]
    pub state_persisted: bool,
    #[serde(default, rename = "leaseCount")]
    pub lease_count: usize,
    #[serde(default, rename = "bootstrapCount")]
    pub bootstrap_count: usize,
    #[serde(default, rename = "artifactCount")]
    pub artifact_count: usize,
    #[serde(default, rename = "warmImageCount")]
    pub warm_image_count: usize,
}

impl CloudWorkerStatusProjection {
    pub fn validate(&self) -> Result<(), String> {
        if self.owner != "cloud-workers" {
            return Err(format!("owner must be cloud-workers, got {:?}", self.owner));
        }
        Ok(())
    }

    pub fn is_disabled(&self) -> bool {
        self.status == CloudWorkerCoordinatorStatus::Disabled
    }

    pub fn is_unavailable(&self) -> bool {
        self.status == CloudWorkerCoordinatorStatus::Unavailable
    }
}

impl CloudWorkerDoctorProjection {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(diagnostic_code) = &self.diagnostic_code {
            validate_projection_text("diagnosticCode", diagnostic_code)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CloudWorkerCoordinatorEvent {
    StatusChanged {
        projection: CloudWorkerStatusProjection,
    },
}

impl CloudWorkerCoordinatorEvent {
    pub const fn family(&self) -> &'static str {
        CLOUD_WORKERS_STATUS_EVENT_FAMILY
    }

    /// Decode only the registered cloud-worker event family. Unknown families
    /// and malformed payloads stay outside this projection boundary.
    pub fn from_event(family: &str, payload: Value) -> Option<Self> {
        if family != CLOUD_WORKERS_STATUS_EVENT_FAMILY {
            return None;
        }
        let event = serde_json::from_value(payload).ok()?;
        match &event {
            Self::StatusChanged { projection } => projection.validate().ok()?,
        }
        Some(event)
    }

    pub fn payload(&self) -> Value {
        serde_json::to_value(self).expect("cloud-worker event is serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_gateway_method_and_coordinator_command_identical() {
        assert_eq!(
            CloudWorkerCoordinatorCommand::Status.method(),
            "cloudWorkers.status"
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::from_method("cloudWorkers.status"),
            Some(CloudWorkerCoordinatorCommand::Status)
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::Capabilities.method(),
            "cloudWorkers.capabilities"
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::from_method("cloudWorkers.capabilities"),
            Some(CloudWorkerCoordinatorCommand::Capabilities)
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::Doctor.method(),
            "cloudWorkers.doctor"
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::from_method("cloudWorkers.doctor"),
            Some(CloudWorkerCoordinatorCommand::Doctor)
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::Status.args(),
            serde_json::json!({})
        );
        assert_eq!(
            CloudWorkerCoordinatorCommand::Doctor.args(),
            serde_json::json!({})
        );
    }

    #[test]
    fn status_projection_preserves_disabled_boundary() {
        let projection = CloudWorkerStatusProjection {
            owner: "cloud-workers".into(),
            status: CloudWorkerCoordinatorStatus::Disabled,
            provider_ready: false,
            production_mutation: false,
            state_persisted: false,
            lease_count: 0,
            warm_image_count: 0,
            bootstrap_count: 0,
            artifact_count: 0,
            migration_receipt_count: 0,
        };
        assert!(projection.is_disabled());
        let event = CloudWorkerCoordinatorEvent::StatusChanged { projection };
        assert_eq!(event.family(), "cloud-workers.status");
    }

    #[test]
    fn capabilities_projection_remains_metadata_only() {
        let projection = CloudWorkerCapabilitiesProjection {
            owner: "cloud-workers".into(),
            metadata_only: true,
            providers: vec![CloudWorkerCapabilityProvider {
                provider_id: "crabbox".into(),
                plugin_id: "crabbox".into(),
                label: Some("Crabbox Worker Provider".into()),
            }],
        };
        projection.validate().unwrap();
        assert!(serde_json::to_value(projection).unwrap()["metadataOnly"] == true);
        let mut invalid = CloudWorkerCapabilitiesProjection {
            owner: "cloud-workers".into(),
            metadata_only: false,
            providers: Vec::new(),
        };
        assert!(invalid.validate().is_err());
        invalid.metadata_only = true;
        invalid.owner = "other-owner".into();
        assert!(invalid.validate().is_err());
        let duplicate = CloudWorkerCapabilitiesProjection {
            owner: "cloud-workers".into(),
            metadata_only: true,
            providers: vec![
                CloudWorkerCapabilityProvider {
                    provider_id: "crabbox".into(),
                    plugin_id: "crabbox".into(),
                    label: None,
                },
                CloudWorkerCapabilityProvider {
                    provider_id: "crabbox".into(),
                    plugin_id: "crabbox".into(),
                    label: None,
                },
            ],
        };
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn status_projection_preserves_unavailable_as_a_closed_state() {
        let projection: CloudWorkerStatusProjection = serde_json::from_value(serde_json::json!({
            "owner": "cloud-workers",
            "status": "unavailable",
            "providerReady": false,
            "productionMutation": false
        }))
        .unwrap();
        assert!(projection.is_unavailable());
        assert!(!projection.is_disabled());
        assert!(serde_json::from_value::<CloudWorkerStatusProjection>(serde_json::json!({
            "owner": "cloud-workers",
            "status": "future-status",
            "providerReady": false,
            "productionMutation": false
        }))
        .is_err());
    }

    #[test]
    fn event_round_trip_is_scoped_to_the_registered_family() {
        let event = CloudWorkerCoordinatorEvent::StatusChanged {
            projection: CloudWorkerStatusProjection {
                owner: "cloud-workers".into(),
                status: CloudWorkerCoordinatorStatus::Disabled,
                provider_ready: false,
                production_mutation: false,
                state_persisted: false,
                lease_count: 0,
                warm_image_count: 0,
                bootstrap_count: 0,
                artifact_count: 0,
                migration_receipt_count: 0,
            },
        };
        let decoded = CloudWorkerCoordinatorEvent::from_event(event.family(), event.payload());
        assert_eq!(decoded, Some(event));
        assert_eq!(
            CloudWorkerCoordinatorEvent::from_event("other.status", serde_json::json!({})),
            None
        );
    }

    #[test]
    fn rejects_status_projection_with_a_different_owner() {
        let payload = serde_json::json!({
            "kind": "status-changed",
            "projection": {
                "owner": "other-owner",
                "status": "disabled",
                "providerReady": false,
                "productionMutation": false
            }
        });
        assert_eq!(
            CloudWorkerCoordinatorEvent::from_event(
                CLOUD_WORKERS_STATUS_EVENT_FAMILY,
                payload
            ),
            None
        );
    }

    #[test]
    fn rejects_doctor_diagnostic_codes_that_are_unbounded_or_controlled() {
        let controlled = CloudWorkerDoctorProjection {
            status: CloudWorkerDoctorProjectionStatus::Failed,
            mutation_started: false,
            diagnostic_code: Some("bad\ncode".into()),
            state_persisted: false,
            lease_count: 0,
            bootstrap_count: 0,
            artifact_count: 0,
            warm_image_count: 0,
        };
        assert!(controlled.validate().is_err());

        let oversized = CloudWorkerDoctorProjection {
            diagnostic_code: Some("x".repeat(MAX_PROJECTION_TEXT_CHARS + 1)),
            ..controlled
        };
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn preserves_read_only_owner_snapshot_facts_from_host() {
        let projection: CloudWorkerStatusProjection = serde_json::from_value(serde_json::json!({
            "owner": "cloud-workers",
            "status": "enabled",
            "providerReady": false,
            "productionMutation": false,
            "statePersisted": true,
            "leaseCount": 2,
            "warmImageCount": 1,
            "migrationReceiptCount": 3
        }))
        .expect("host status projection should decode");
        assert!(projection.state_persisted);
        assert_eq!(projection.lease_count, 2);
        assert_eq!(projection.warm_image_count, 1);
        assert_eq!(projection.migration_receipt_count, 3);
        assert!(!projection.provider_ready);
        assert!(!projection.production_mutation);
    }
}
