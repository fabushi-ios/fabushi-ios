use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

pub const BOOTSTRAP_FLAG: &str = "--bootstrap=";
pub const COORDINATOR_CONTROL_CHANNEL: &str = "coordinator-control";
pub const COORDINATOR_DATA_CHANNEL: &str = "coordinator-data";
pub const COORDINATOR_MAIN_DATA_CHANNEL: &str = "coordinator-main-data";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorProcessConfig {
    pub app_version: String,
    pub is_packaged: bool,
    pub data_dir: String,
    #[serde(default)]
    pub local_human_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorBootstrap {
    pub process_config: CoordinatorProcessConfig,
}

impl CoordinatorBootstrap {
    pub fn validate(self) -> Result<Self, String> {
        if self.process_config.app_version.trim().is_empty() {
            return Err("bootstrap.processConfig.appVersion must be non-empty".into());
        }
        if self.process_config.data_dir.trim().is_empty() {
            return Err("bootstrap.processConfig.dataDir must be non-empty".into());
        }
        if self
            .process_config
            .local_human_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err("bootstrap.processConfig.localHumanId must be non-empty when provided".into());
        }
        Ok(self)
    }
}

pub fn parse_bootstrap_argument<'a>(
    argv: impl IntoIterator<Item = &'a str>,
) -> Result<CoordinatorBootstrap, String> {
    let argument = argv
        .into_iter()
        .find(|value| value.starts_with(BOOTSTRAP_FLAG))
        .ok_or_else(|| "missing --bootstrap argument".to_string())?;
    let raw = &argument[BOOTSTRAP_FLAG.len()..];
    let parsed = serde_json::from_str::<CoordinatorBootstrap>(raw)
        .map_err(|_| "--bootstrap is not valid JSON".to_string())?;
    parsed.validate()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CarrierChannel {
    Control,
    Data,
    MainData,
}

impl CarrierChannel {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Control => COORDINATOR_CONTROL_CHANNEL,
            Self::Data => COORDINATOR_DATA_CHANNEL,
            Self::MainData => COORDINATOR_MAIN_DATA_CHANNEL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarrierEnvelope {
    pub channel: String,
    pub frame: Value,
}

impl CarrierEnvelope {
    pub fn new(channel: CarrierChannel, frame: Value) -> Self {
        Self {
            channel: channel.wire_name().to_string(),
            frame,
        }
    }

    pub fn classify(&self) -> Option<CarrierChannel> {
        match self.channel.as_str() {
            COORDINATOR_CONTROL_CHANNEL => Some(CarrierChannel::Control),
            COORDINATOR_DATA_CHANNEL => Some(CarrierChannel::Data),
            COORDINATOR_MAIN_DATA_CHANNEL => Some(CarrierChannel::MainData),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CarrierMessage {
    pub channel: CarrierChannel,
    pub frame: Value,
}

#[derive(Debug)]
pub struct Carrier {
    pub bootstrap: CoordinatorBootstrap,
    queued: VecDeque<CarrierMessage>,
    closed: bool,
}

impl Carrier {
    pub fn new(bootstrap: CoordinatorBootstrap) -> Self {
        Self {
            bootstrap,
            queued: VecDeque::new(),
            closed: false,
        }
    }

    pub fn post(&mut self, channel: CarrierChannel, frame: Value) -> Result<(), &'static str> {
        if self.closed {
            return Err("carrier is closed");
        }
        self.queued.push_back(CarrierMessage { channel, frame });
        Ok(())
    }

    pub fn accept_envelope(&mut self, envelope: CarrierEnvelope) -> Result<(), &'static str> {
        let channel = envelope.classify().ok_or("unknown carrier channel")?;
        self.post(channel, envelope.frame)
    }

    pub fn drain(&mut self) -> Vec<CarrierMessage> {
        self.queued.drain(..).collect()
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.queued.clear();
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
