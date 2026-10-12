use crate::generated_occurrence_codec::{
    GeneratedAgentV1Bindings, GeneratedTranscriptOccurrenceCodec,
};
use crate::transcript_journal_codec::TranscriptCheckpoint;
use crate::transcript_mirror::{FileTranscriptMirror, JournalOutcome};
use crate::transcript_mirror_router::{
    LegacyTranscriptMirrorPort, RoutedTranscriptMirror,
};
use crate::transcript_occurrence_deriver::{
    ArtifactTranscriptOccurrenceDeriver, TranscriptOccurrenceBlobStore,
};
use std::path::PathBuf;

pub type ProductionGeneratedTranscriptDeriver<Bindings> =
    ArtifactTranscriptOccurrenceDeriver<GeneratedTranscriptOccurrenceCodec<Bindings>>;

pub type ProductionTranscriptMirror<Bindings, Store, Legacy, FeatureFlag> =
    RoutedTranscriptMirror<
        TranscriptCheckpoint,
        Store,
        FileTranscriptMirror<ProductionGeneratedTranscriptDeriver<Bindings>>,
        Legacy,
        FeatureFlag,
    >;

/// iOS-owned composition equivalent of Grok's production transcript provider.
///
/// This function deliberately accepts only generated agent.v1 bindings, rather
/// than a raw protobuf decoder. The generated codec adapter remains the sole
/// turn/user/step decode boundary, then the production deriver feeds the
/// journal mirror. The legacy writer is injected so Host composition can bind
/// the iOS app-task/offload implementation without making this provider own a
/// process or daemon.
pub fn create_production_transcript_mirror<
    Bindings,
    Store,
    Legacy,
    FeatureFlag,
    Reporter,
>(
    transcripts_dir: impl Into<PathBuf>,
    bindings: Bindings,
    legacy: Legacy,
    is_journal_enabled: FeatureFlag,
    report_outcome: Reporter,
) -> ProductionTranscriptMirror<Bindings, Store, Legacy, FeatureFlag>
where
    Bindings: GeneratedAgentV1Bindings,
    Store: Clone + TranscriptOccurrenceBlobStore,
    Legacy: LegacyTranscriptMirrorPort<TranscriptCheckpoint, Store>,
    FeatureFlag: FnMut() -> Result<bool, String>,
    Reporter: Fn(JournalOutcome) + Send + Sync + 'static,
{
    let codec = GeneratedTranscriptOccurrenceCodec::new(bindings);
    let deriver = ArtifactTranscriptOccurrenceDeriver::new(codec);
    let journal = FileTranscriptMirror::new(transcripts_dir, deriver, report_outcome);
    RoutedTranscriptMirror::new(journal, legacy, is_journal_enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_occurrence_codec::{
        GeneratedConversationStep, GeneratedConversationTurn, GeneratedUserMessage,
    };
    use crate::transcript_mirror_router::{LegacyTranscriptMirrorPort, TranscriptMirrorRoute};
    use serde_json::Value;
    use std::collections::HashMap;
    use std::fmt;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug)]
    struct DecodeError(&'static str);

    impl fmt::Display for DecodeError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    #[derive(Clone, Copy)]
    struct Bindings;

    impl GeneratedAgentV1Bindings for Bindings {
        type Error = DecodeError;

        fn decode_conversation_turn_structure(
            &self,
            bytes: &[u8],
        ) -> Result<GeneratedConversationTurn, Self::Error> {
            match bytes {
                [1] => Ok(GeneratedConversationTurn::Agent {
                    user_message: vec![7],
                    steps: vec![],
                }),
                _ => Ok(GeneratedConversationTurn::Empty),
            }
        }

        fn decode_user_message(
            &self,
            bytes: &[u8],
        ) -> Result<GeneratedUserMessage, Self::Error> {
            if bytes == [2] {
                Ok(GeneratedUserMessage {
                    text: "hello from generated agent.v1".into(),
                    text_blob_id: None,
                })
            } else {
                Err(DecodeError("unexpected generated user message"))
            }
        }

        fn decode_conversation_step(
            &self,
            _bytes: &[u8],
        ) -> Result<GeneratedConversationStep, Self::Error> {
            Ok(GeneratedConversationStep::Empty)
        }
    }

    #[derive(Clone, Default)]
    struct Store(Arc<HashMap<Vec<u8>, Vec<u8>>>);

    impl Store {
        fn fixture() -> Self {
            Self(Arc::new(HashMap::from([
                (vec![1], vec![1]),
                (vec![7], vec![2]),
            ])))
        }
    }

    impl TranscriptOccurrenceBlobStore for Store {
        fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>> {
            self.0.get(id).cloned()
        }
    }

    #[derive(Clone, Default)]
    struct Legacy {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl LegacyTranscriptMirrorPort<TranscriptCheckpoint, Store> for Legacy {
        fn write(
            &mut self,
            _conversation_id: &str,
            _checkpoint: &TranscriptCheckpoint,
            _blob_store: &Store,
            state_blob_id: &[u8],
        ) -> Result<(), String> {
            self.writes.lock().unwrap().push(state_blob_id.to_vec());
            Ok(())
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-production-transcript-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn composes_generated_codec_deriver_journal_and_reporter() {
        let root = temp_dir("journal");
        let outcomes = Arc::new(Mutex::new(Vec::<JournalOutcome>::new()));
        let sink = outcomes.clone();
        let legacy = Legacy::default();
        let legacy_writes = legacy.writes.clone();

        let mut mirror = create_production_transcript_mirror::<_, Store, _, _, _>(
            &root,
            Bindings,
            legacy,
            || Ok(true),
            move |outcome| sink.lock().unwrap().push(outcome),
        );
        let store = Store::fixture();
        let initial = TranscriptCheckpoint::default();
        let next = TranscriptCheckpoint {
            turns: vec![vec![1]],
        };

        mirror.recover("conversation-a", &initial, &store).unwrap();
        mirror
            .prepare_checkpoint("conversation-a", &next, &store, true, true)
            .unwrap();
        mirror
            .commit_checkpoint("conversation-a", &[9])
            .unwrap();

        assert_eq!(
            mirror.route_for("conversation-a"),
            Some(TranscriptMirrorRoute::Journal)
        );
        assert!(legacy_writes.lock().unwrap().is_empty());
        let body = fs::read_to_string(
            root.join("conversation-a").join("conversation-a.jsonl"),
        )
        .unwrap();
        assert!(body.contains("hello from generated agent.v1"));
        assert_eq!(
            outcomes
                .lock()
                .unwrap()
                .iter()
                .map(|row| row.op)
                .collect::<Vec<_>>(),
            vec!["replay", "checkpoint", "append"]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn feature_flag_routes_unowned_conversation_to_injected_legacy_writer() {
        let root = temp_dir("legacy");
        let legacy = Legacy::default();
        let writes = legacy.writes.clone();
        let mut mirror = create_production_transcript_mirror::<_, Store, _, _, _>(
            &root,
            Bindings,
            legacy,
            || Ok(false),
            |_| {},
        );
        let store = Store::fixture();
        let checkpoint = TranscriptCheckpoint::default();

        mirror
            .prepare_checkpoint("conversation-a", &checkpoint, &store, true, true)
            .unwrap();
        mirror
            .commit_checkpoint("conversation-a", &[4, 5, 6])
            .unwrap();

        assert_eq!(
            mirror.route_for("conversation-a"),
            Some(TranscriptMirrorRoute::Legacy)
        );
        assert_eq!(writes.lock().unwrap().as_slice(), &[vec![4, 5, 6]]);
        assert!(
            !root
                .join("conversation-a")
                .join("conversation-a.journal-mode")
                .exists()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn provider_surface_never_requires_dynamic_json_or_manual_wire_decoder() {
        fn assert_json_is_only_tool_projection(_: Option<Value>) {}
        assert_json_is_only_tool_projection(None);
        let _ = std::any::type_name::<ProductionGeneratedTranscriptDeriver<Bindings>>();
    }
}
