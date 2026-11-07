use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::experiments::HostExperimentsExtension;
use crate::host_paths::get_sand_root_dir;
use crate::storage::agent_paths::get_sand_agents_root_dir;
use crate::transcript_mutation_events::{
    TranscriptMutation as RawTranscriptMutation,
    TranscriptMutationSubscription as RawTranscriptMutationSubscription,
    subscribe_transcript_mutations,
};

use super::search_index_db::{MessageSearchResult, MediaSearchResult};
use super::search_index_service::{
    SandSearchIndexService, SearchIndexHealth,
    TranscriptMutation as ServiceTranscriptMutation,
};
use super::search_index_writer::IndexEntry;

pub const CONTENT_SEARCH_EXTENSION_ID: &str = "content-search";
pub const GLOBAL_SEARCH_GATE: &str = "sand_global_search";
pub const SEARCH_INDEX_FILENAME: &str = "search-index.db";
pub const SEARCH_INDEX_DISPOSE_TIMEOUT_MS: u64 = 2_000;
pub const AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT: usize = 5;
pub const AGENT_CONTENT_SEARCH_MAX_RESULTS: usize = 50;
pub const CONTENT_SEARCH_DEPENDENCIES: &[&str] = &["experiments", "telemetry"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMutation {
    pub agent_id: String,
    pub entry_id: String,
}

pub trait ContentSearchIndex: Send + Sync + 'static {
    fn start(&self);
    fn dispose(&self);
    fn is_search_ready(&self) -> bool;
    fn search_messages(&self, query: &str, limit: usize) -> Vec<String>;
    fn search_media(&self, query: &str, limit: usize) -> Vec<String>;
    fn apply_mutation(&self, mutation: TranscriptMutation);
}

pub trait ContentSearchIndexFactory: Send + Sync {
    type Index: ContentSearchIndex;
    fn create(&self) -> Arc<Self::Index>;
}

pub trait FeatureGateSubscription: Send {
    fn dispose(&mut self);
}

pub trait FeatureGateProperty: Send + Sync {
    fn get(&self) -> bool;
    fn subscribe(
        &self,
        listener: Arc<dyn Fn(bool) + Send + Sync>,
    ) -> Box<dyn FeatureGateSubscription>;
}

pub trait TranscriptMutationSubscription: Send {
    fn dispose(&mut self);
}

pub trait TranscriptMutationSource: Send + Sync {
    fn subscribe(
        &self,
        listener: Arc<dyn Fn(TranscriptMutation) + Send + Sync>,
    ) -> Box<dyn TranscriptMutationSubscription>;
}

struct RuntimeState {
    mutation_subscription: Option<Box<dyn TranscriptMutationSubscription>>,
}

pub struct ContentSearchExtension<I: ContentSearchIndex> {
    index: Arc<I>,
    check_gate: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    state: Arc<Mutex<RuntimeState>>,
    gate_subscription: Mutex<Option<Box<dyn FeatureGateSubscription>>>,
}

impl<I: ContentSearchIndex> ContentSearchExtension<I> {
    pub fn is_enabled(&self) -> bool {
        (self.check_gate)(GLOBAL_SEARCH_GATE)
    }

    pub fn is_search_ready(&self) -> bool {
        self.index.is_search_ready()
    }

    pub fn max_matches_per_agent(&self) -> usize {
        AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT
    }

    pub fn max_results(&self) -> usize {
        AGENT_CONTENT_SEARCH_MAX_RESULTS
    }

    pub fn search_messages(&self, query: &str, limit: usize) -> Vec<String> {
        self.index.search_messages(query, limit)
    }

    pub fn search_media(&self, query: &str, limit: usize) -> Vec<String> {
        self.index.search_media(query, limit)
    }
}

impl<I: ContentSearchIndex> Drop for ContentSearchExtension<I> {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.gate_subscription.lock() {
            if let Some(mut subscription) = gate.take() {
                subscription.dispose();
            }
        }
        if let Ok(mut state) = self.state.lock() {
            if let Some(mut subscription) = state.mutation_subscription.take() {
                subscription.dispose();
            }
        }
        self.index.dispose();
    }
}

pub fn start_content_search_extension<F>(
    factory: &F,
    gate: Arc<dyn FeatureGateProperty>,
    check_gate: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    mutations: Arc<dyn TranscriptMutationSource>,
) -> ContentSearchExtension<F::Index>
where
    F: ContentSearchIndexFactory,
{
    let index = factory.create();
    let state = Arc::new(Mutex::new(RuntimeState {
        mutation_subscription: None,
    }));

    let apply_gate: Arc<dyn Fn(bool) + Send + Sync> = {
        let index = Arc::clone(&index);
        let state = Arc::clone(&state);
        let mutations = Arc::clone(&mutations);
        Arc::new(move |enabled| {
            let Ok(mut runtime) = state.lock() else {
                return;
            };
            if !enabled {
                if let Some(mut subscription) = runtime.mutation_subscription.take() {
                    subscription.dispose();
                }
                return;
            }
            if runtime.mutation_subscription.is_none() {
                let target = Arc::clone(&index);
                runtime.mutation_subscription = Some(mutations.subscribe(Arc::new(move |mutation| {
                    target.apply_mutation(mutation);
                })));
            }
            index.start();
        })
    };

    let gate_subscription = gate.subscribe(Arc::clone(&apply_gate));
    apply_gate(gate.get());

    ContentSearchExtension {
        index,
        check_gate,
        state,
        gate_subscription: Mutex::new(Some(gate_subscription)),
    }
}


pub type SearchIndexHealthReporter = Arc<dyn Fn(SearchIndexHealth) + Send + Sync>;

pub struct ProductionContentSearchExtension {
    service: Arc<SandSearchIndexService>,
    experiments: Arc<HostExperimentsExtension>,
    mutation_subscription: Arc<Mutex<Option<RawTranscriptMutationSubscription>>>,
    experiment_subscription: Mutex<Option<StopSubscription>>,
}

impl ProductionContentSearchExtension {
    pub fn is_enabled(&self) -> bool {
        self.experiments.check_feature_gate(GLOBAL_SEARCH_GATE)
    }

    pub fn is_search_ready(&self) -> bool {
        self.service.is_search_ready()
    }

    pub fn max_matches_per_agent(&self) -> usize {
        AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT
    }

    pub fn max_results(&self) -> usize {
        AGENT_CONTENT_SEARCH_MAX_RESULTS
    }

    pub fn search_messages(
        &self,
        query: &str,
        limit: usize,
    ) -> Option<Vec<MessageSearchResult>> {
        self.service.search_messages(query, limit)
    }

    pub fn search_media(
        &self,
        query: &str,
        limit: usize,
    ) -> Option<Vec<MediaSearchResult>> {
        self.service.search_media(query, limit)
    }

    pub fn when_idle(&self) -> bool {
        self.service.when_idle()
    }
}

impl Drop for ProductionContentSearchExtension {
    fn drop(&mut self) {
        if let Ok(mut subscription) = self.experiment_subscription.lock() {
            if let Some(stop) = subscription.take() {
                stop();
            }
        }
        if let Ok(mut subscription) = self.mutation_subscription.lock() {
            if let Some(subscription) = subscription.take() {
                subscription.unsubscribe();
            }
        }
        self.service.dispose();
    }
}

pub fn start_production_content_search_extension(
    experiments: Arc<HostExperimentsExtension>,
    report: SearchIndexHealthReporter,
) -> ProductionContentSearchExtension {
    start_production_content_search_extension_with_paths(
        &get_sand_root_dir(),
        &get_sand_agents_root_dir(None),
        experiments,
        report,
    )
}

pub fn start_production_content_search_extension_with_paths(
    sand_root_dir: &Path,
    agents_root_dir: &Path,
    experiments: Arc<HostExperimentsExtension>,
    report: SearchIndexHealthReporter,
) -> ProductionContentSearchExtension {
    let service = Arc::new(SandSearchIndexService::new_with_reporter(
        sand_root_dir.join(SEARCH_INDEX_FILENAME),
        agents_root_dir.to_path_buf(),
        report,
    ));

    let mutation_subscription: Arc<Mutex<Option<RawTranscriptMutationSubscription>>> =
        Arc::new(Mutex::new(None));
    let apply_gate: Arc<dyn Fn() + Send + Sync> = {
        let service = Arc::clone(&service);
        let experiments = Arc::clone(&experiments);
        let mutation_subscription = Arc::clone(&mutation_subscription);
        Arc::new(move || {
            let enabled = experiments.check_feature_gate(GLOBAL_SEARCH_GATE);
            let Ok(mut active_subscription) = mutation_subscription.lock() else {
                return;
            };
            if !enabled {
                if let Some(subscription) = active_subscription.take() {
                    subscription.unsubscribe();
                }
                return;
            }

            if active_subscription.is_none() {
                let service_for_mutation = Arc::clone(&service);
                *active_subscription = Some(subscribe_transcript_mutations(move |mutation| {
                    if let Some(projected) = project_transcript_mutation(mutation) {
                        service_for_mutation.apply_mutation(projected);
                    }
                }));
            }
            drop(active_subscription);
            service.start();
        })
    };

    let experiment_subscription = experiments.subscribe(Arc::clone(&apply_gate));
    apply_gate();

    ProductionContentSearchExtension {
        service,
        experiments,
        mutation_subscription,
        experiment_subscription: Mutex::new(Some(experiment_subscription)),
    }
}

pub fn project_transcript_mutation(
    mutation: &RawTranscriptMutation,
) -> Option<ServiceTranscriptMutation> {
    let kind = mutation.get("kind")?.as_str()?;
    let agent_id = mutation
        .get("agentId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    match kind {
        "entries-upserted" => {
            let agent_id = agent_id?;
            let entries = mutation
                .get("entries")?
                .as_array()?
                .iter()
                .cloned()
                .map(serde_json::from_value::<IndexEntry>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Some(ServiceTranscriptMutation::EntriesUpserted { agent_id, entries })
        }
        "entry-deleted" => Some(ServiceTranscriptMutation::EntryDeleted {
            agent_id: agent_id?,
            entry_id: mutation
                .get("entryId")?
                .as_str()?
                .trim()
                .to_string(),
        }),
        "conversation-cleared" => Some(ServiceTranscriptMutation::ConversationCleared {
            agent_id: agent_id?,
        }),
        "agent-removed" => Some(ServiceTranscriptMutation::AgentRemoved {
            agent_id: agent_id?,
        }),
        "agent-needs-reindex" => Some(ServiceTranscriptMutation::AgentNeedsReindex {
            agent_id: agent_id?,
        }),
        _ => None,
    }
}
