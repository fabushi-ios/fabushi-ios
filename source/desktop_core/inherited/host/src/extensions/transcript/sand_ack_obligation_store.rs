use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::durable_file_policy::SAND_ACK_OBLIGATIONS_FILE_NAME;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckObligation {
    pub agent_id: String,
    pub created_at_ms: f64,
    pub last_send_at_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_interrupt_at_ms: Option<f64>,
    pub coalesced_count: f64,
    pub redrive_attempts: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordSendOutcome {
    pub obligation: AckObligation,
    pub created: bool,
}

pub fn finite_number(value: Option<&Value>, fallback: f64) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

pub fn coerce_obligation(value: &Value) -> Option<AckObligation> {
    let entry = value.as_object()?;
    let agent_id = entry
        .get("agentId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?
        .to_string();
    let created_at_ms = finite_number(entry.get("createdAtMs"), 0.0);
    Some(AckObligation {
        agent_id,
        created_at_ms,
        last_send_at_ms: finite_number(entry.get("lastSendAtMs"), created_at_ms),
        last_interrupt_at_ms: entry
            .get("lastInterruptAtMs")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite()),
        coalesced_count: finite_number(entry.get("coalescedCount"), 1.0).max(1.0),
        redrive_attempts: finite_number(entry.get("redriveAttempts"), 0.0).max(0.0),
    })
}

pub fn parse_ack_obligations_file(raw: Option<&str>) -> Vec<AckObligation> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    value
        .get("pending")
        .and_then(Value::as_array)
        .map(|pending| pending.iter().filter_map(coerce_obligation).collect())
        .unwrap_or_default()
}

pub struct SandAckObligationStore {
    file_path: PathBuf,
    cache: Mutex<Option<Vec<AckObligation>>>,
}

impl SandAckObligationStore {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            file_path: root_dir.as_ref().join(SAND_ACK_OBLIGATIONS_FILE_NAME),
            cache: Mutex::new(None),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn get(&self, agent_id: &str) -> Option<AckObligation> {
        self.list()
            .into_iter()
            .find(|entry| entry.agent_id == agent_id)
    }

    pub fn list(&self) -> Vec<AckObligation> {
        if let Ok(cache) = self.cache.lock() {
            if let Some(pending) = cache.as_ref() {
                return pending.clone();
            }
        }
        let raw = fs::read_to_string(&self.file_path).ok();
        let parsed = parse_ack_obligations_file(raw.as_deref());
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(parsed.clone());
        }
        parsed
    }

    pub fn record_send(&self, agent_id: &str, at_ms: f64) -> io::Result<RecordSendOutcome> {
        let existing = self.get(agent_id);
        let obligation = match existing.as_ref() {
            Some(existing) => AckObligation {
                last_send_at_ms: at_ms,
                coalesced_count: existing.coalesced_count + 1.0,
                ..existing.clone()
            },
            None => AckObligation {
                agent_id: agent_id.to_string(),
                created_at_ms: at_ms,
                last_send_at_ms: at_ms,
                last_interrupt_at_ms: None,
                coalesced_count: 1.0,
                redrive_attempts: 0.0,
            },
        };
        self.upsert(obligation.clone())?;
        Ok(RecordSendOutcome {
            obligation,
            created: existing.is_none(),
        })
    }

    pub fn record_interrupt(&self, agent_id: &str, at_ms: f64) -> io::Result<bool> {
        let Some(mut existing) = self.get(agent_id) else {
            return Ok(false);
        };
        existing.last_interrupt_at_ms = Some(at_ms);
        self.upsert(existing)?;
        Ok(true)
    }

    pub fn record_redrive_attempt(&self, agent_id: &str) -> io::Result<Option<AckObligation>> {
        let Some(mut existing) = self.get(agent_id) else {
            return Ok(None);
        };
        existing.redrive_attempts += 1.0;
        self.upsert(existing.clone())?;
        Ok(Some(existing))
    }

    pub fn clear(&self, agent_id: &str) -> io::Result<bool> {
        let current = self.list();
        let remaining = current
            .iter()
            .filter(|entry| entry.agent_id != agent_id)
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == current.len() {
            return Ok(false);
        }
        self.write(&remaining)?;
        Ok(true)
    }

    pub fn upsert(&self, obligation: AckObligation) -> io::Result<()> {
        let mut pending = self
            .list()
            .into_iter()
            .filter(|entry| entry.agent_id != obligation.agent_id)
            .collect::<Vec<_>>();
        pending.push(obligation);
        self.write(&pending)
    }

    pub fn restore(&self, previous: Option<AckObligation>, agent_id: &str) -> io::Result<()> {
        match previous {
            Some(previous) => self.upsert(previous),
            None => {
                let _ = self.clear(agent_id)?;
                Ok(())
            }
        }
    }

    fn write(&self, pending: &[AckObligation]) -> io::Result<()> {
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let part = PathBuf::from(format!("{}.part", self.file_path.display()));
        fs::write(
            &part,
            serde_json::to_vec(&json!({
                "version": 1,
                "pending": pending,
            }))
            .map_err(io::Error::other)?,
        )?;
        fs::rename(part, &self.file_path)?;
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(pending.to_vec());
        }
        Ok(())
    }
}
