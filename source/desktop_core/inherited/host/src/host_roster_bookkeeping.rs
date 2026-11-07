use std::collections::HashSet;
use std::sync::Arc;

use crate::extensions::attachments::attachments_service::AttachmentsService;
use crate::extensions::box_store_sync::production::ProductionBoxStoreSyncApi;
use crate::extensions::forever_box::ForeverBoxService;
use crate::extensions::source_map::source_map_service::SandSourceMap;
use crate::extensions::transcript::transcript_manager::TranscriptManager;
use crate::host_production_extensions::ProductionHostExtensions;

pub trait AttachmentRosterPort: Send + Sync {
    fn set_fallback_agent_id(&self, agent_id: Option<&str>);
}

pub trait TranscriptRosterPort: Send + Sync {
    fn live_running_agent_ids(&self) -> Vec<String>;
}

pub trait ForeverBoxRosterPort: Send + Sync {
    fn enroll_disk_pressure_reminder(&self, agent_ids: &HashSet<String>);
    fn set_busy(&self, is_busy: bool);
}

pub trait SnapshotBackstopPort: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn schedule_snapshot(&self, agent_id: &str);
}

pub trait BoxStoreSnapshotPort: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn schedule_store_db_snapshot(&self, agent_id: &str);
}

pub trait SourceMapRosterPort: Send + Sync {
    fn get_or_create(&self, agent_id: &str);
}

pub struct HostRosterPorts {
    pub attachments: Arc<dyn AttachmentRosterPort>,
    pub transcript: Arc<dyn TranscriptRosterPort>,
    pub forever_box: Arc<dyn ForeverBoxRosterPort>,
    pub state_backstop: Arc<dyn SnapshotBackstopPort>,
    pub box_store_sync: Arc<dyn BoxStoreSnapshotPort>,
    pub source_map: Arc<dyn SourceMapRosterPort>,
}

pub struct HostRosterBookkeeping {
    ports: HostRosterPorts,
    latest_active_agent_id: Option<String>,
    is_busy: bool,
    running_agent_ids: HashSet<String>,
}

struct ProductionAttachmentsRosterPort(Arc<AttachmentsService>);

impl AttachmentRosterPort for ProductionAttachmentsRosterPort {
    fn set_fallback_agent_id(&self, agent_id: Option<&str>) {
        self.0
            .set_fallback_agent_id(agent_id.map(str::to_string));
    }
}

struct ProductionTranscriptRosterPort(Arc<TranscriptManager>);

impl TranscriptRosterPort for ProductionTranscriptRosterPort {
    fn live_running_agent_ids(&self) -> Vec<String> {
        self.0.live_running_agent_ids()
    }
}

struct ProductionForeverBoxRosterPort(Arc<ForeverBoxService>);

impl ForeverBoxRosterPort for ProductionForeverBoxRosterPort {
    fn enroll_disk_pressure_reminder(&self, agent_ids: &HashSet<String>) {
        if let Some(episodes) = self.0.disk_pressure_reminder_episodes() {
            episodes.enroll(agent_ids.iter().map(String::as_str));
        }
    }

    fn set_busy(&self, is_busy: bool) {
        self.0.set_busy(is_busy);
    }
}

struct ProductionStateBackstopRosterPort(Arc<ProductionHostExtensions>);

impl SnapshotBackstopPort for ProductionStateBackstopRosterPort {
    fn is_enabled(&self) -> bool {
        self.0.state_backstop_is_enabled().unwrap_or(false)
    }

    fn schedule_snapshot(&self, agent_id: &str) {
        let _ = self.0.schedule_state_backstop_snapshot(agent_id);
    }
}

struct ProductionBoxStoreRosterPort(ProductionBoxStoreSyncApi);

impl BoxStoreSnapshotPort for ProductionBoxStoreRosterPort {
    fn is_enabled(&self) -> bool {
        self.0.is_enabled()
    }

    fn schedule_store_db_snapshot(&self, agent_id: &str) {
        let _ = self.0.schedule_store_db_snapshot(agent_id);
    }
}

struct ProductionSourceMapRosterPort(Arc<SandSourceMap>);

impl SourceMapRosterPort for ProductionSourceMapRosterPort {
    fn get_or_create(&self, agent_id: &str) {
        let _ = self.0.get_or_create(agent_id);
    }
}

pub fn production_host_roster_bookkeeping(
    attachments: Arc<AttachmentsService>,
    transcript: Arc<TranscriptManager>,
    forever_box: Arc<ForeverBoxService>,
    state_backstop_owner: Arc<ProductionHostExtensions>,
    box_store_sync: ProductionBoxStoreSyncApi,
    source_map: Arc<SandSourceMap>,
) -> HostRosterBookkeeping {
    HostRosterBookkeeping::new(HostRosterPorts {
        attachments: Arc::new(ProductionAttachmentsRosterPort(attachments)),
        transcript: Arc::new(ProductionTranscriptRosterPort(transcript)),
        forever_box: Arc::new(ProductionForeverBoxRosterPort(forever_box)),
        state_backstop: Arc::new(ProductionStateBackstopRosterPort(state_backstop_owner)),
        box_store_sync: Arc::new(ProductionBoxStoreRosterPort(box_store_sync)),
        source_map: Arc::new(ProductionSourceMapRosterPort(source_map)),
    })
}

impl HostRosterBookkeeping {
    pub fn new(ports: HostRosterPorts) -> Self {
        Self {
            ports,
            latest_active_agent_id: None,
            is_busy: false,
            running_agent_ids: HashSet::new(),
        }
    }

    pub fn latest_active_agent_id(&self) -> Option<&str> {
        self.latest_active_agent_id.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        self.is_busy
    }

    pub fn running_agent_ids(&self) -> HashSet<String> {
        self.running_agent_ids.clone()
    }

    pub fn apply_config(&mut self, cfg: &serde_json::Value) {
        let active = crate::agents::agent_roster::try_resolve_default_agent_id(cfg);
        self.apply(active.as_deref());
    }

    pub fn apply(&mut self, active_agent_id: Option<&str>) {
        let normalized = active_agent_id
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        self.latest_active_agent_id = normalized.clone();
        self.ports.attachments.set_fallback_agent_id(normalized.as_deref());

        let previously_running = self.running_agent_ids.clone();
        self.running_agent_ids = self
            .ports
            .transcript
            .live_running_agent_ids()
            .into_iter()
            .collect();
        self.is_busy = !self.running_agent_ids.is_empty();

        let started = self
            .running_agent_ids
            .difference(&previously_running)
            .cloned()
            .collect::<HashSet<_>>();
        if !started.is_empty() {
            self.ports
                .forever_box
                .enroll_disk_pressure_reminder(&started);
        }
        self.ports.forever_box.set_busy(self.is_busy);

        if self.ports.state_backstop.is_enabled()
            || self.ports.box_store_sync.is_enabled()
        {
            for agent_id in previously_running.difference(&self.running_agent_ids) {
                self.ports.state_backstop.schedule_snapshot(agent_id);
                self.ports
                    .box_store_sync
                    .schedule_store_db_snapshot(agent_id);
            }
        }

        if let Some(agent_id) = normalized.as_deref() {
            self.ports.source_map.get_or_create(agent_id);
        }
    }
}
