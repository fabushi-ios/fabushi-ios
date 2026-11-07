pub fn cloud_agent_transcript_dump_path(bc_id: &str) -> String {
    format!("cloud-agent-transcripts/{bc_id}.jsonl")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentTranscriptDump {
    pub status: String,
    pub line_count: usize,
    pub jsonl: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentDumpFile {
    pub path: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentDumpOutcome {
    pub status: String,
    pub line_count: usize,
    pub file: Option<CloudAgentDumpFile>,
}

pub trait CloudAgentDumpApi {
    fn get_transcript_dump(
        &self,
        bc_id: &str,
    ) -> Result<Option<CloudAgentTranscriptDump>, String>;
}

pub trait CloudAgentBoxFileWriter {
    fn write_box_file(&self, path: &str, data: &[u8]) -> Result<(), String>;
}

pub fn dump_cloud_agent_transcript(
    api: &dyn CloudAgentDumpApi,
    writer: &dyn CloudAgentBoxFileWriter,
    bc_id: &str,
) -> Result<Option<CloudAgentDumpOutcome>, String> {
    let Some(dump) = api.get_transcript_dump(bc_id)? else {
        return Ok(None);
    };
    if dump.line_count == 0 {
        return Ok(Some(CloudAgentDumpOutcome {
            status: dump.status,
            line_count: 0,
            file: None,
        }));
    }
    let path = cloud_agent_transcript_dump_path(bc_id);
    let data = dump.jsonl.as_bytes();
    writer.write_box_file(&path, data)?;
    Ok(Some(CloudAgentDumpOutcome {
        status: dump.status,
        line_count: dump.line_count,
        file: Some(CloudAgentDumpFile {
            path,
            size_bytes: data.len(),
        }),
    }))
}

pub fn format_cloud_agent_dump_completion_line(
    file: &CloudAgentDumpFile,
    line_count: usize,
) -> String {
    format!(
        "Full transcript dumped to {} ({} bytes, {} message lines). Read it with your shell tools; `tail -n 1 {}` gives the final assistant report.",
        file.path, file.size_bytes, line_count, file.path
    )
}

pub trait CloudAgentWatchResult: Clone {
    fn text(&self) -> &str;
    fn with_text(&self, text: String) -> Self;
}

pub fn augment_watch_result_with_transcript_dump<T: CloudAgentWatchResult>(
    api: &dyn CloudAgentDumpApi,
    writer: &dyn CloudAgentBoxFileWriter,
    bc_id: &str,
    result: &T,
) -> T {
    let Ok(Some(outcome)) = dump_cloud_agent_transcript(api, writer, bc_id) else {
        return result.clone();
    };
    let Some(file) = outcome.file else {
        return result.clone();
    };
    result.with_text(format!(
        "{}\n\n{}",
        result.text(),
        format_cloud_agent_dump_completion_line(&file, outcome.line_count)
    ))
}
