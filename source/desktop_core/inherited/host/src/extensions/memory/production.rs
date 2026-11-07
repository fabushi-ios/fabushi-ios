use std::path::PathBuf;
use std::sync::Arc;

use crate::extensions::inference::production::ProductionInferenceExtension;
use crate::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry;
use crate::extensions::telemetry::memory_synthesis_telemetry::{
    MemorySynthesisReport,
};
use crate::extensions::telemetry::sand_error_tags::SandErrorValue;
use crate::storage::agent_paths::get_sand_agents_root_dir;

use super::extension::{HostMemoryExtension, start_memory_extension};
use super::memory_service::{
    FileMemoryStore, MemoryService, SynthesisApplyResult, SynthesisChange, SynthesisSnapshot,
};
use super::memory_synthesis_service::{
    MemorySynthesisOptions, MemorySynthesisService, SynthesisCancelSignal,
    SynthesisProposalRequest, SynthesisReport, SynthesisReportOutcome, SynthesisTarget,
    SynthesisVerificationRequest, parse_json_object, synthesis_request_json,
    synthesis_system_prompt, verification_request_json, verification_system_prompt,
};

pub fn start_production_memory_extension() -> HostMemoryExtension {
    start_memory_extension(get_sand_agents_root_dir(None))
}

pub fn memory_synthesis_telemetry_report(report: &SynthesisReport) -> MemorySynthesisReport {
    match report.outcome {
        SynthesisReportOutcome::Committed | SynthesisReportOutcome::NoWork => {
            MemorySynthesisReport::Ok {
                duration_ms: report.duration_ms as f64,
                item_count: report.change_count as i64,
            }
        }
        SynthesisReportOutcome::Dropped => MemorySynthesisReport::Shed {
            cause: SandErrorValue::new("SAND-E0412"),
            item_count: report.evidence_count as i64,
        },
        SynthesisReportOutcome::InvalidOutput => MemorySynthesisReport::Failed {
            cause: SandErrorValue::new("SAND-E0409"),
            duration_ms: report.duration_ms as f64,
            item_count: report.change_count as i64,
        },
        SynthesisReportOutcome::Rejected => MemorySynthesisReport::Failed {
            cause: SandErrorValue::new("SAND-E0410"),
            duration_ms: report.duration_ms as f64,
            item_count: report.change_count as i64,
        },
        SynthesisReportOutcome::Stale => MemorySynthesisReport::Failed {
            cause: SandErrorValue::new("SAND-E0411"),
            duration_ms: report.duration_ms as f64,
            item_count: report.change_count as i64,
        },
        SynthesisReportOutcome::Failed => MemorySynthesisReport::Failed {
            cause: SandErrorValue::new("SAND-E0413"),
            duration_ms: report.duration_ms as f64,
            item_count: report.change_count as i64,
        },
    }
}

struct ProductionSynthesisTarget {
    agent_id: String,
    store: FileMemoryStore,
    service: Arc<MemoryService>,
}

impl SynthesisTarget for ProductionSynthesisTarget {
    fn prepare_synthesis(&self) -> SynthesisSnapshot {
        self.store.prepare_synthesis()
    }

    fn apply_synthesis(
        &self,
        snapshot: &SynthesisSnapshot,
        changes: &[SynthesisChange],
        now_ms: i64,
    ) -> SynthesisApplyResult {
        self.service.apply_synthesis_with_gate(
            &self.agent_id,
            &self.store,
            snapshot,
            changes,
            now_ms,
        )
    }

    fn has_memories(&self) -> bool {
        self.store.has_memories()
    }

    fn is_temporal_review_due(&self, now_ms: i64) -> bool {
        self.store.is_temporal_review_due(now_ms)
    }

    fn mark_temporal_review(&self, now_ms: i64) {
        let _ = self.store.mark_temporal_review(now_ms);
    }
}

pub fn create_production_memory_synthesis(
    service: Arc<MemoryService>,
    inference: Arc<ProductionInferenceExtension>,
    data_dir: PathBuf,
    logs: HostStructuredLogTelemetry,
) -> Arc<MemorySynthesisService> {
    let list_service = Arc::clone(&service);
    let list_targets = Arc::new(move || {
        list_service
            .list_synthesis_targets()
            .into_iter()
            .map(|(agent_id, store)| {
                (
                    agent_id.clone(),
                    Arc::new(ProductionSynthesisTarget {
                        agent_id,
                        store,
                        service: Arc::clone(&list_service),
                    }) as Arc<dyn SynthesisTarget>,
                )
            })
            .collect::<Vec<_>>()
    });

    let target_service = Arc::clone(&service);
    let get_target = Arc::new(move |agent_id: &str| {
        target_service
            .synthesis_target_for_agent(agent_id)
            .map(|store| {
                Arc::new(ProductionSynthesisTarget {
                    agent_id: agent_id.to_string(),
                    store,
                    service: Arc::clone(&target_service),
                }) as Arc<dyn SynthesisTarget>
            })
    });

    let propose_inference = Arc::clone(&inference);
    let propose_data_dir = data_dir.clone();
    let propose = Arc::new(move |request: SynthesisProposalRequest, cancel: SynthesisCancelSignal| {
        let user = synthesis_request_json(&request);
        let output = propose_inference
            .run_summarization_prompt(
                &propose_data_dir,
                synthesis_system_prompt(),
                &user,
                &|| cancel.is_cancelled(),
            )
            .map_err(|error| format!("{error:?}"))?;
        parse_json_object(&output)
            .ok_or_else(|| "memory synthesis inference returned invalid JSON".to_string())
    });

    let verify_inference = Arc::clone(&inference);
    let verify = Arc::new(move |request: SynthesisVerificationRequest, cancel: SynthesisCancelSignal| {
        let user = verification_request_json(&request);
        let output = verify_inference
            .run_summarization_prompt(
                &data_dir,
                verification_system_prompt(),
                &user,
                &|| cancel.is_cancelled(),
            )
            .map_err(|error| format!("{error:?}"))?;
        let verdict = parse_json_object(&output)
            .ok_or_else(|| "memory synthesis verification returned invalid JSON".to_string())?;
        Ok(verdict
            .get("approved")
            .and_then(serde_json::Value::as_bool)
            == Some(true))
    });

    let mut options = MemorySynthesisOptions::new(list_targets, get_target, propose);
    options.verify = Some(verify);
    options.report = Some(Arc::new(move |report| {
        let report = memory_synthesis_telemetry_report(&report);
        let _ = logs.report_memory_synthesis(&report);
    }));
    Arc::new(MemorySynthesisService::new(options))
}
