use crate::package_utils_workspace_paths::{
    TRANSCRIPTS_SUBDIR, get_safe_conversation_id,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptPathArgs<'a> {
    pub conversation_id: &'a str,
    pub kind: &'a str,
    pub parent_conversation_id: Option<&'a str>,
    pub ext: &'a str,
}

pub fn get_transcript_relative_path(args: TranscriptPathArgs<'_>) -> String {
    let safe_id = get_safe_conversation_id(args.conversation_id);
    if args.kind == "subagent" {
        if let Some(parent_conversation_id) = args.parent_conversation_id {
            let safe_parent_id = get_safe_conversation_id(parent_conversation_id);
            return format!(
                "{TRANSCRIPTS_SUBDIR}/{safe_parent_id}/subagents/{safe_id}.{}",
                args.ext
            );
        }
    }
    format!("{TRANSCRIPTS_SUBDIR}/{safe_id}/{safe_id}.{}", args.ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_root_and_subagent_paths_with_package_safe_ids() {
        assert_eq!(
            get_transcript_relative_path(TranscriptPathArgs {
                conversation_id: "root/a",
                kind: "root",
                parent_conversation_id: None,
                ext: "jsonl",
            }),
            "agent-transcripts/root_2Fa/root_2Fa.jsonl"
        );
        assert_eq!(
            get_transcript_relative_path(TranscriptPathArgs {
                conversation_id: "child b",
                kind: "subagent",
                parent_conversation_id: Some("root/a"),
                ext: "txt",
            }),
            "agent-transcripts/root_2Fa/subagents/child_20b.txt"
        );
    }

    #[test]
    fn non_subagent_or_missing_parent_uses_root_layout() {
        let path = get_transcript_relative_path(TranscriptPathArgs {
            conversation_id: "c",
            kind: "subagent",
            parent_conversation_id: None,
            ext: "jsonl",
        });
        assert_eq!(path, "agent-transcripts/c/c.jsonl");
    }
}
