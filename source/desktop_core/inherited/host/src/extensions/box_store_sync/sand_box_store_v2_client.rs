use std::collections::HashMap;
use std::sync::Arc;

use prost::{Message, Oneof};

use crate::cursor_backend::{
    CursorBackendError, resolve_sand_ghost_mode_header, send_cursor_unary,
};

use super::sand_box_store_files::{
    SAND_BOX_STORE_RPC_TIMEOUT_MS, SandBoxStoreClientError, SandBoxStoreClientErrorCode,
    SandBoxStoreCompletedPart, SandBoxStoreListPage, SandBoxStoreMultipartCompletion,
    SandBoxStoreMultipartCompletionOutcome, SandBoxStoreMultipartInstruction,
    SandBoxStoreMultipartPartRequest, SandBoxStoreMultipartPresignedPart,
    SandBoxStoreObjectStat, SandBoxStoreReadInstruction, SandBoxStoreServiceClient,
    SandBoxStoreWriteFile, SandBoxStoreWriteInstruction,
};

const PRESIGN_WRITES_PATH: &str =
    "/aiserver.v1.GrokBotService/PresignSandBoxStoreWrites";
const COMPLETE_MULTIPART_WRITES_PATH: &str =
    "/aiserver.v1.GrokBotService/CompleteSandBoxStoreMultipartWrites";
const ABORT_MULTIPART_WRITES_PATH: &str =
    "/aiserver.v1.GrokBotService/AbortSandBoxStoreMultipartWrites";
const PRESIGN_READS_PATH: &str =
    "/aiserver.v1.GrokBotService/PresignSandBoxStoreReads";
const STAT_OBJECT_PATH: &str =
    "/aiserver.v1.GrokBotService/StatSandBoxStoreObject";
const LIST_OBJECTS_PATH: &str =
    "/aiserver.v1.GrokBotService/ListSandBoxStoreObjects";

type UnaryTransport = Arc<
    dyn Fn(&str, &[u8]) -> Result<Vec<u8>, SandBoxStoreClientError> + Send + Sync + 'static,
>;

#[derive(Clone)]
pub struct SandBoxStoreV2ClientDependencies {
    pub backend_url: String,
    pub get_access_token: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub get_machine_id: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
}

#[derive(Clone)]
pub struct SandBoxStoreV2Client {
    transport: UnaryTransport,
}

impl SandBoxStoreV2Client {
    pub fn new(deps: SandBoxStoreV2ClientDependencies) -> Self {
        let transport: UnaryTransport = Arc::new(move |path, body| {
            let access_token = (deps.get_access_token)().map_err(transport_error)?;
            let machine_id = (deps.get_machine_id)().map_err(transport_error)?;
            let ghost_mode =
                resolve_sand_ghost_mode_header(&deps.backend_url, &access_token, &machine_id);
            send_cursor_unary(
                &deps.backend_url,
                &access_token,
                &machine_id,
                path,
                body,
                SAND_BOX_STORE_RPC_TIMEOUT_MS,
                ghost_mode,
            )
            .map_err(map_cursor_error)
        });
        Self { transport }
    }

    #[cfg(test)]
    fn with_transport(transport: UnaryTransport) -> Self {
        Self { transport }
    }

    fn rpc<Req, Res>(&self, path: &str, request: Req) -> Result<Res, SandBoxStoreClientError>
    where
        Req: Message,
        Res: Message + Default,
    {
        let mut body = Vec::new();
        request
            .encode(&mut body)
            .map_err(|error| protocol_error(format!("encode {path}: {error}")))?;
        let response = (self.transport)(path, &body)?;
        Res::decode(response.as_slice())
            .map_err(|error| protocol_error(format!("decode {path}: {error}")))
    }
}

impl SandBoxStoreServiceClient for SandBoxStoreV2Client {
    fn presign_reads(
        &self,
        rel_paths: &[String],
    ) -> Result<Vec<SandBoxStoreReadInstruction>, SandBoxStoreClientError> {
        let response: PresignSandBoxStoreReadsResponseProto = self.rpc(
            PRESIGN_READS_PATH,
            PresignSandBoxStoreReadsRequestProto {
                rel_paths: rel_paths.to_vec(),
            },
        )?;
        response
            .instructions
            .into_iter()
            .map(|instruction| {
                Ok(SandBoxStoreReadInstruction {
                    rel_path: instruction.rel_path,
                    url: instruction.url,
                    expires_at_ms: nonnegative_i64(
                        instruction.expires_at_ms,
                        "read instruction expires_at_ms",
                    )?,
                })
            })
            .collect()
    }

    fn stat_object(
        &self,
        rel_path: &str,
    ) -> Result<SandBoxStoreObjectStat, SandBoxStoreClientError> {
        let response: StatSandBoxStoreObjectResponseProto = self.rpc(
            STAT_OBJECT_PATH,
            StatSandBoxStoreObjectRequestProto {
                rel_path: rel_path.to_string(),
            },
        )?;
        Ok(SandBoxStoreObjectStat {
            exists: response.exists,
            etag: response.etag,
        })
    }

    fn presign_writes(
        &self,
        files: &[SandBoxStoreWriteFile],
    ) -> Result<Vec<SandBoxStoreWriteInstruction>, SandBoxStoreClientError> {
        let files = files
            .iter()
            .map(write_file_to_proto)
            .collect::<Result<Vec<_>, _>>()?;
        let response: PresignSandBoxStoreWritesResponseProto = self.rpc(
            PRESIGN_WRITES_PATH,
            PresignSandBoxStoreWritesRequestProto { files },
        )?;
        response
            .instructions
            .into_iter()
            .map(write_instruction_from_proto)
            .collect()
    }

    fn complete_multipart_writes(
        &self,
        completions: &[SandBoxStoreMultipartCompletion],
    ) -> Result<Vec<SandBoxStoreMultipartCompletionOutcome>, SandBoxStoreClientError> {
        let completions = completions
            .iter()
            .map(completion_to_proto)
            .collect::<Result<Vec<_>, _>>()?;
        let completion_count = completions.len();
        let response: CompleteSandBoxStoreMultipartWritesResponseProto = self.rpc(
            COMPLETE_MULTIPART_WRITES_PATH,
            CompleteSandBoxStoreMultipartWritesRequestProto { completions },
        )?;
        indexed_completion_outcomes(response.results, completion_count)
    }

    fn abort_multipart_writes(
        &self,
        contexts: &[Vec<u8>],
    ) -> Result<(), SandBoxStoreClientError> {
        let uploads = contexts
            .iter()
            .map(|context| {
                Ok(SandBoxStoreMultipartWriteAbortProto {
                    context: Some(decode_context(context)?),
                })
            })
            .collect::<Result<Vec<_>, SandBoxStoreClientError>>()?;
        let response: AbortSandBoxStoreMultipartWritesResponseProto = self.rpc(
            ABORT_MULTIPART_WRITES_PATH,
            AbortSandBoxStoreMultipartWritesRequestProto { uploads },
        )?;
        validate_abort_results(response.results, contexts.len())
    }

    fn list_objects(
        &self,
        prefix: &str,
        cursor: &str,
        max_entries: usize,
    ) -> Result<SandBoxStoreListPage, SandBoxStoreClientError> {
        let max_entries = i32::try_from(max_entries)
            .map_err(|_| invalid_argument("list max_entries exceeds int32"))?;
        let response: ListSandBoxStoreObjectsResponseProto = self.rpc(
            LIST_OBJECTS_PATH,
            ListSandBoxStoreObjectsRequestProto {
                prefix: prefix.to_string(),
                cursor: cursor.to_string(),
                max_entries,
            },
        )?;
        Ok(SandBoxStoreListPage {
            entries: response
                .entries
                .into_iter()
                .map(|entry| entry.rel_path)
                .collect(),
            truncated: response.truncated,
            next_cursor: response.next_cursor,
        })
    }
}

fn write_file_to_proto(
    file: &SandBoxStoreWriteFile,
) -> Result<SandBoxStoreWriteFileProto, SandBoxStoreClientError> {
    Ok(SandBoxStoreWriteFileProto {
        rel_path: file.rel_path.clone(),
        sha256: file.sha256.clone(),
        size_bytes: bounded_i64(file.size_bytes, "write file size_bytes")?,
        content_addressed: file.content_addressed,
        if_match_etag: file.if_match_etag.clone(),
        expect_absent: file.expect_absent,
        multipart_parts: file
            .multipart_parts
            .iter()
            .map(multipart_part_to_proto)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn multipart_part_to_proto(
    part: &SandBoxStoreMultipartPartRequest,
) -> Result<SandBoxStoreMultipartPartProto, SandBoxStoreClientError> {
    Ok(SandBoxStoreMultipartPartProto {
        part_number: u32::try_from(part.part_number)
            .map_err(|_| invalid_argument("multipart part_number exceeds uint32"))?,
        size_bytes: bounded_i64(part.size_bytes, "multipart part size_bytes")?,
        sha256: part.sha256.clone(),
    })
}

fn write_instruction_from_proto(
    instruction: SandBoxStoreWriteInstructionProto,
) -> Result<SandBoxStoreWriteInstruction, SandBoxStoreClientError> {
    let multipart = instruction
        .multipart
        .map(|multipart| {
            let context = multipart
                .context
                .map(|context| {
                    let mut bytes = Vec::new();
                    context
                        .encode(&mut bytes)
                        .map_err(|error| protocol_error(format!("encode multipart context: {error}")))?;
                    Ok(bytes)
                })
                .transpose()?;
            let parts = multipart
                .parts
                .into_iter()
                .map(|part| {
                    Ok(SandBoxStoreMultipartPresignedPart {
                        part_number: usize::try_from(part.part_number)
                            .map_err(|_| protocol_error("multipart part_number exceeds usize"))?,
                        offset_bytes: nonnegative_i64(
                            part.offset_bytes,
                            "multipart part offset_bytes",
                        )?,
                        size_bytes: nonnegative_i64(
                            part.size_bytes,
                            "multipart part size_bytes",
                        )?,
                        url: part.url,
                        headers: part.headers,
                    })
                })
                .collect::<Result<Vec<_>, SandBoxStoreClientError>>()?;
            Ok(SandBoxStoreMultipartInstruction { context, parts })
        })
        .transpose()?;
    Ok(SandBoxStoreWriteInstruction {
        rel_path: instruction.rel_path,
        url: instruction.url,
        headers: instruction.headers,
        multipart,
    })
}

fn completion_to_proto(
    completion: &SandBoxStoreMultipartCompletion,
) -> Result<SandBoxStoreMultipartWriteCompletionProto, SandBoxStoreClientError> {
    let parts = completion
        .parts
        .iter()
        .map(|part| {
            Ok(SandBoxStoreMultipartUploadedPartProto {
                part_number: u32::try_from(part.part_number)
                    .map_err(|_| invalid_argument("uploaded part_number exceeds uint32"))?,
                etag: part.etag.clone(),
            })
        })
        .collect::<Result<Vec<_>, SandBoxStoreClientError>>()?;
    Ok(SandBoxStoreMultipartWriteCompletionProto {
        context: Some(decode_context(&completion.context)?),
        parts,
    })
}

fn decode_context(
    bytes: &[u8],
) -> Result<SandBoxStoreMultipartUploadContextProto, SandBoxStoreClientError> {
    SandBoxStoreMultipartUploadContextProto::decode(bytes)
        .map_err(|error| protocol_error(format!("decode multipart context: {error}")))
}

fn indexed_completion_outcomes(
    results: Vec<SandBoxStoreMultipartWriteResultProto>,
    expected: usize,
) -> Result<Vec<SandBoxStoreMultipartCompletionOutcome>, SandBoxStoreClientError> {
    let mut outcomes = vec![SandBoxStoreMultipartCompletionOutcome::Missing; expected];
    let mut seen = vec![false; expected];
    for result in results {
        let index = usize::try_from(result.input_index)
            .map_err(|_| protocol_error("multipart completion input_index exceeds usize"))?;
        if index >= expected {
            return Err(protocol_error(format!(
                "multipart completion input_index {index} is outside {expected} requests"
            )));
        }
        if seen[index] {
            return Err(protocol_error(format!(
                "multipart completion input_index {index} was returned twice"
            )));
        }
        seen[index] = true;
        outcomes[index] = match result.outcome {
            Some(sand_box_store_multipart_write_result_proto::Outcome::Success(_)) => {
                SandBoxStoreMultipartCompletionOutcome::Success
            }
            Some(sand_box_store_multipart_write_result_proto::Outcome::Failure(failure)) => {
                SandBoxStoreMultipartCompletionOutcome::Failure { code: failure.code }
            }
            None => SandBoxStoreMultipartCompletionOutcome::Missing,
        };
    }
    Ok(outcomes)
}

fn validate_abort_results(
    results: Vec<SandBoxStoreMultipartAbortResultProto>,
    expected: usize,
) -> Result<(), SandBoxStoreClientError> {
    let mut seen = vec![false; expected];
    for result in results {
        let index = usize::try_from(result.input_index)
            .map_err(|_| protocol_error("multipart abort input_index exceeds usize"))?;
        if index >= expected {
            return Err(protocol_error(format!(
                "multipart abort input_index {index} is outside {expected} requests"
            )));
        }
        if seen[index] {
            return Err(protocol_error(format!(
                "multipart abort input_index {index} was returned twice"
            )));
        }
        seen[index] = true;
        match result.outcome {
            Some(sand_box_store_multipart_abort_result_proto::Outcome::Success(_)) => {}
            Some(sand_box_store_multipart_abort_result_proto::Outcome::Failure(failure)) => {
                return Err(other_error(format!(
                    "multipart abort failed for input {index} with code {}",
                    failure.code
                )));
            }
            None => {
                return Err(protocol_error(format!(
                    "multipart abort returned no outcome for input {index}"
                )));
            }
        }
    }
    if seen.iter().any(|seen| !seen) {
        return Err(protocol_error(format!(
            "multipart abort returned {} results for {expected} requests",
            seen.iter().filter(|seen| **seen).count()
        )));
    }
    Ok(())
}

fn bounded_i64(value: u64, label: &str) -> Result<i64, SandBoxStoreClientError> {
    i64::try_from(value).map_err(|_| invalid_argument(format!("{label} exceeds int64")))
}

fn nonnegative_i64(value: i64, label: &str) -> Result<u64, SandBoxStoreClientError> {
    u64::try_from(value).map_err(|_| protocol_error(format!("{label} was negative")))
}

fn invalid_argument(message: impl Into<String>) -> SandBoxStoreClientError {
    SandBoxStoreClientError {
        code: SandBoxStoreClientErrorCode::InvalidArgument,
        message: message.into(),
    }
}

fn transport_error(message: impl Into<String>) -> SandBoxStoreClientError {
    SandBoxStoreClientError {
        code: SandBoxStoreClientErrorCode::Transport,
        message: message.into(),
    }
}

fn protocol_error(message: impl Into<String>) -> SandBoxStoreClientError {
    SandBoxStoreClientError {
        code: SandBoxStoreClientErrorCode::Protocol,
        message: message.into(),
    }
}

fn other_error(message: impl Into<String>) -> SandBoxStoreClientError {
    SandBoxStoreClientError {
        code: SandBoxStoreClientErrorCode::Other,
        message: message.into(),
    }
}

fn map_cursor_error(error: CursorBackendError) -> SandBoxStoreClientError {
    match error {
        CursorBackendError::HttpStatus { status: 400, body } => {
            invalid_argument(format!("SandBoxStoreV2 invalid argument: {body}"))
        }
        CursorBackendError::InvalidProto(message) => protocol_error(message),
        CursorBackendError::InvalidBackendUrl(message)
        | CursorBackendError::Transport(message)
        | CursorBackendError::Timeout(message)
        | CursorBackendError::Cancelled(message) => transport_error(message),
        CursorBackendError::HttpStatus { status, body } => {
            other_error(format!("SandBoxStoreV2 HTTP {status}: {body}"))
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartPartProto {
    #[prost(uint32, tag = "1")]
    part_number: u32,
    #[prost(int64, tag = "2")]
    size_bytes: i64,
    #[prost(string, tag = "3")]
    sha256: String,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreWriteFileProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    sha256: String,
    #[prost(int64, tag = "3")]
    size_bytes: i64,
    #[prost(bool, tag = "4")]
    content_addressed: bool,
    #[prost(string, tag = "5")]
    if_match_etag: String,
    #[prost(bool, tag = "6")]
    expect_absent: bool,
    #[prost(message, repeated, tag = "7")]
    multipart_parts: Vec<SandBoxStoreMultipartPartProto>,
}

#[derive(Clone, PartialEq, Message)]
struct PresignSandBoxStoreWritesRequestProto {
    #[prost(message, repeated, tag = "1")]
    files: Vec<SandBoxStoreWriteFileProto>,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartUploadPartInstructionProto {
    #[prost(uint32, tag = "1")]
    part_number: u32,
    #[prost(string, tag = "2")]
    url: String,
    #[prost(map = "string, string", tag = "3")]
    headers: HashMap<String, String>,
    #[prost(int64, tag = "4")]
    offset_bytes: i64,
    #[prost(int64, tag = "5")]
    size_bytes: i64,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartUploadContextProto {
    #[prost(string, tag = "1")]
    upload_id: String,
    #[prost(string, tag = "2")]
    rel_path: String,
    #[prost(int64, tag = "3")]
    size_bytes: i64,
    #[prost(string, tag = "4")]
    sha256: String,
    #[prost(uint32, tag = "5")]
    expected_part_count: u32,
    #[prost(string, repeated, tag = "6")]
    part_sha256s: Vec<String>,
    #[prost(oneof = "sand_box_store_multipart_upload_context_proto::Precondition", tags = "7, 8")]
    precondition: Option<sand_box_store_multipart_upload_context_proto::Precondition>,
    #[prost(string, tag = "9")]
    session_id: String,
}

mod sand_box_store_multipart_upload_context_proto {
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Precondition {
        #[prost(string, tag = "7")]
        IfMatchEtag(String),
        #[prost(bool, tag = "8")]
        ExpectAbsent(bool),
    }
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartWriteInstructionProto {
    #[prost(message, optional, tag = "1")]
    context: Option<SandBoxStoreMultipartUploadContextProto>,
    #[prost(message, repeated, tag = "2")]
    parts: Vec<SandBoxStoreMultipartUploadPartInstructionProto>,
    #[prost(int64, tag = "3")]
    part_urls_expires_at_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreWriteInstructionProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    url: String,
    #[prost(map = "string, string", tag = "3")]
    headers: HashMap<String, String>,
    #[prost(int64, tag = "4")]
    expires_at_ms: i64,
    #[prost(message, optional, tag = "5")]
    multipart: Option<SandBoxStoreMultipartWriteInstructionProto>,
}

#[derive(Clone, PartialEq, Message)]
struct PresignSandBoxStoreWritesResponseProto {
    #[prost(message, repeated, tag = "1")]
    instructions: Vec<SandBoxStoreWriteInstructionProto>,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartUploadedPartProto {
    #[prost(uint32, tag = "1")]
    part_number: u32,
    #[prost(string, tag = "2")]
    etag: String,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartWriteCompletionProto {
    #[prost(message, optional, tag = "1")]
    context: Option<SandBoxStoreMultipartUploadContextProto>,
    #[prost(message, repeated, tag = "2")]
    parts: Vec<SandBoxStoreMultipartUploadedPartProto>,
}

#[derive(Clone, PartialEq, Message)]
struct CompleteSandBoxStoreMultipartWritesRequestProto {
    #[prost(message, repeated, tag = "1")]
    completions: Vec<SandBoxStoreMultipartWriteCompletionProto>,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartWriteSuccessProto {
    #[prost(string, tag = "1")]
    etag: String,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartOperationFailureProto {
    #[prost(int32, tag = "1")]
    code: i32,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartWriteResultProto {
    #[prost(uint32, tag = "1")]
    input_index: u32,
    #[prost(string, tag = "2")]
    rel_path: String,
    #[prost(oneof = "sand_box_store_multipart_write_result_proto::Outcome", tags = "3, 4")]
    outcome: Option<sand_box_store_multipart_write_result_proto::Outcome>,
}

mod sand_box_store_multipart_write_result_proto {
    use prost::Oneof;

    use super::{
        SandBoxStoreMultipartOperationFailureProto, SandBoxStoreMultipartWriteSuccessProto,
    };

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Outcome {
        #[prost(message, tag = "3")]
        Success(SandBoxStoreMultipartWriteSuccessProto),
        #[prost(message, tag = "4")]
        Failure(SandBoxStoreMultipartOperationFailureProto),
    }
}

#[derive(Clone, PartialEq, Message)]
struct CompleteSandBoxStoreMultipartWritesResponseProto {
    #[prost(message, repeated, tag = "1")]
    results: Vec<SandBoxStoreMultipartWriteResultProto>,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartWriteAbortProto {
    #[prost(message, optional, tag = "1")]
    context: Option<SandBoxStoreMultipartUploadContextProto>,
}

#[derive(Clone, PartialEq, Message)]
struct AbortSandBoxStoreMultipartWritesRequestProto {
    #[prost(message, repeated, tag = "1")]
    uploads: Vec<SandBoxStoreMultipartWriteAbortProto>,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartAbortSuccessProto {
    #[prost(bool, tag = "1")]
    already_finished: bool,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreMultipartAbortResultProto {
    #[prost(uint32, tag = "1")]
    input_index: u32,
    #[prost(string, tag = "2")]
    rel_path: String,
    #[prost(oneof = "sand_box_store_multipart_abort_result_proto::Outcome", tags = "3, 4")]
    outcome: Option<sand_box_store_multipart_abort_result_proto::Outcome>,
}

mod sand_box_store_multipart_abort_result_proto {
    use prost::Oneof;

    use super::{
        SandBoxStoreMultipartAbortSuccessProto, SandBoxStoreMultipartOperationFailureProto,
    };

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Outcome {
        #[prost(message, tag = "3")]
        Success(SandBoxStoreMultipartAbortSuccessProto),
        #[prost(message, tag = "4")]
        Failure(SandBoxStoreMultipartOperationFailureProto),
    }
}

#[derive(Clone, PartialEq, Message)]
struct AbortSandBoxStoreMultipartWritesResponseProto {
    #[prost(message, repeated, tag = "1")]
    results: Vec<SandBoxStoreMultipartAbortResultProto>,
}

#[derive(Clone, PartialEq, Message)]
struct PresignSandBoxStoreReadsRequestProto {
    #[prost(string, repeated, tag = "1")]
    rel_paths: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreReadInstructionProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    url: String,
    #[prost(int64, tag = "3")]
    expires_at_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct PresignSandBoxStoreReadsResponseProto {
    #[prost(message, repeated, tag = "1")]
    instructions: Vec<SandBoxStoreReadInstructionProto>,
}

#[derive(Clone, PartialEq, Message)]
struct StatSandBoxStoreObjectRequestProto {
    #[prost(string, tag = "1")]
    rel_path: String,
}

#[derive(Clone, PartialEq, Message)]
struct StatSandBoxStoreObjectResponseProto {
    #[prost(bool, tag = "1")]
    exists: bool,
    #[prost(string, tag = "2")]
    etag: String,
    #[prost(int64, tag = "3")]
    size_bytes: i64,
    #[prost(int64, tag = "4")]
    last_modified_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct ListSandBoxStoreObjectsRequestProto {
    #[prost(string, tag = "1")]
    prefix: String,
    #[prost(string, tag = "2")]
    cursor: String,
    #[prost(int32, tag = "3")]
    max_entries: i32,
}

#[derive(Clone, PartialEq, Message)]
struct SandBoxStoreObjectEntryProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    etag: String,
    #[prost(int64, tag = "3")]
    size_bytes: i64,
    #[prost(int64, tag = "4")]
    last_modified_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct ListSandBoxStoreObjectsResponseProto {
    #[prost(message, repeated, tag = "1")]
    entries: Vec<SandBoxStoreObjectEntryProto>,
    #[prost(string, tag = "2")]
    next_cursor: String,
    #[prost(bool, tag = "3")]
    truncated: bool,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn context() -> SandBoxStoreMultipartUploadContextProto {
        SandBoxStoreMultipartUploadContextProto {
            upload_id: "upload-1".into(),
            rel_path: "blobs/abc".into(),
            size_bytes: 7,
            sha256: "abc".into(),
            expected_part_count: 1,
            part_sha256s: vec!["part-sha".into()],
            precondition: Some(
                sand_box_store_multipart_upload_context_proto::Precondition::ExpectAbsent(true),
            ),
            session_id: "session-1".into(),
        }
    }

    #[test]
    fn read_stat_and_list_use_recovered_grok_rpc_contracts() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&calls);
        let client = SandBoxStoreV2Client::with_transport(Arc::new(move |path, body| {
            captured.lock().expect("calls").push(path.to_string());
            match path {
                PRESIGN_READS_PATH => {
                    let request =
                        PresignSandBoxStoreReadsRequestProto::decode(body).expect("decode reads");
                    assert_eq!(request.rel_paths, vec!["workspace/a.txt"]);
                    Ok(PresignSandBoxStoreReadsResponseProto {
                        instructions: vec![SandBoxStoreReadInstructionProto {
                            rel_path: "workspace/a.txt".into(),
                            url: "https://example.test/read".into(),
                            expires_at_ms: 1234,
                        }],
                    }
                    .encode_to_vec())
                }
                STAT_OBJECT_PATH => {
                    let request =
                        StatSandBoxStoreObjectRequestProto::decode(body).expect("decode stat");
                    assert_eq!(request.rel_path, "workspace/a.txt");
                    Ok(StatSandBoxStoreObjectResponseProto {
                        exists: true,
                        etag: "etag-a".into(),
                        size_bytes: 1,
                        last_modified_ms: 2,
                    }
                    .encode_to_vec())
                }
                LIST_OBJECTS_PATH => {
                    let request =
                        ListSandBoxStoreObjectsRequestProto::decode(body).expect("decode list");
                    assert_eq!(request.prefix, "workspace/");
                    assert_eq!(request.cursor, "cursor-a");
                    assert_eq!(request.max_entries, 500);
                    Ok(ListSandBoxStoreObjectsResponseProto {
                        entries: vec![SandBoxStoreObjectEntryProto {
                            rel_path: "workspace/a.txt".into(),
                            etag: "etag-a".into(),
                            size_bytes: 1,
                            last_modified_ms: 2,
                        }],
                        next_cursor: "cursor-b".into(),
                        truncated: true,
                    }
                    .encode_to_vec())
                }
                other => panic!("unexpected path {other}"),
            }
        }));

        let reads = client
            .presign_reads(&["workspace/a.txt".into()])
            .expect("presign reads");
        assert_eq!(reads[0].expires_at_ms, 1234);
        let stat = client.stat_object("workspace/a.txt").expect("stat");
        assert!(stat.exists);
        assert_eq!(stat.etag, "etag-a");
        let page = client
            .list_objects("workspace/", "cursor-a", 500)
            .expect("list");
        assert_eq!(page.entries, vec!["workspace/a.txt"]);
        assert!(page.truncated);
        assert_eq!(page.next_cursor, "cursor-b");
        assert_eq!(
            calls.lock().expect("calls").as_slice(),
            [PRESIGN_READS_PATH, STAT_OBJECT_PATH, LIST_OBJECTS_PATH]
        );
    }

    #[test]
    fn write_complete_and_abort_round_trip_recovered_multipart_context() {
        let preserved = context().encode_to_vec();
        let expected_context = preserved.clone();
        let client = SandBoxStoreV2Client::with_transport(Arc::new(move |path, body| {
            match path {
                PRESIGN_WRITES_PATH => {
                    let request =
                        PresignSandBoxStoreWritesRequestProto::decode(body).expect("decode writes");
                    assert_eq!(request.files.len(), 1);
                    let file = &request.files[0];
                    assert_eq!(file.rel_path, "blobs/abc");
                    assert!(file.content_addressed);
                    assert_eq!(file.multipart_parts[0].part_number, 1);
                    Ok(PresignSandBoxStoreWritesResponseProto {
                        instructions: vec![SandBoxStoreWriteInstructionProto {
                            rel_path: "blobs/abc".into(),
                            url: String::new(),
                            headers: HashMap::new(),
                            expires_at_ms: 0,
                            multipart: Some(SandBoxStoreMultipartWriteInstructionProto {
                                context: Some(context()),
                                parts: vec![SandBoxStoreMultipartUploadPartInstructionProto {
                                    part_number: 1,
                                    url: "https://example.test/part".into(),
                                    headers: HashMap::from([("x-test".into(), "1".into())]),
                                    offset_bytes: 0,
                                    size_bytes: 7,
                                }],
                                part_urls_expires_at_ms: 999,
                            }),
                        }],
                    }
                    .encode_to_vec())
                }
                COMPLETE_MULTIPART_WRITES_PATH => {
                    let request = CompleteSandBoxStoreMultipartWritesRequestProto::decode(body)
                        .expect("decode complete");
                    let encoded = request.completions[0]
                        .context
                        .as_ref()
                        .expect("completion context")
                        .encode_to_vec();
                    assert_eq!(encoded, expected_context);
                    assert_eq!(request.completions[0].parts[0].etag, "etag-part");
                    Ok(CompleteSandBoxStoreMultipartWritesResponseProto {
                        results: vec![SandBoxStoreMultipartWriteResultProto {
                            input_index: 0,
                            rel_path: "blobs/abc".into(),
                            outcome: Some(
                                sand_box_store_multipart_write_result_proto::Outcome::Success(
                                    SandBoxStoreMultipartWriteSuccessProto {
                                        etag: "etag-object".into(),
                                    },
                                ),
                            ),
                        }],
                    }
                    .encode_to_vec())
                }
                ABORT_MULTIPART_WRITES_PATH => {
                    let request = AbortSandBoxStoreMultipartWritesRequestProto::decode(body)
                        .expect("decode abort");
                    assert_eq!(
                        request.uploads[0]
                            .context
                            .as_ref()
                            .expect("abort context")
                            .encode_to_vec(),
                        expected_context
                    );
                    Ok(AbortSandBoxStoreMultipartWritesResponseProto {
                        results: vec![SandBoxStoreMultipartAbortResultProto {
                            input_index: 0,
                            rel_path: "blobs/abc".into(),
                            outcome: Some(
                                sand_box_store_multipart_abort_result_proto::Outcome::Success(
                                    SandBoxStoreMultipartAbortSuccessProto {
                                        already_finished: false,
                                    },
                                ),
                            ),
                        }],
                    }
                    .encode_to_vec())
                }
                other => panic!("unexpected path {other}"),
            }
        }));

        let writes = client
            .presign_writes(&[SandBoxStoreWriteFile {
                rel_path: "blobs/abc".into(),
                sha256: "abc".into(),
                size_bytes: 7,
                content_addressed: true,
                if_match_etag: String::new(),
                expect_absent: true,
                multipart_parts: vec![SandBoxStoreMultipartPartRequest {
                    part_number: 1,
                    size_bytes: 7,
                    sha256: "part-sha".into(),
                }],
            }])
            .expect("presign writes");
        let multipart = writes[0].multipart.as_ref().expect("multipart");
        assert_eq!(multipart.context.as_ref().expect("context"), &preserved);
        assert_eq!(multipart.parts[0].size_bytes, 7);

        let completion = SandBoxStoreMultipartCompletion {
            context: preserved.clone(),
            parts: vec![SandBoxStoreCompletedPart {
                part_number: 1,
                etag: "etag-part".into(),
            }],
        };
        assert_eq!(
            client
                .complete_multipart_writes(&[completion])
                .expect("complete"),
            vec![SandBoxStoreMultipartCompletionOutcome::Success]
        );
        client
            .abort_multipart_writes(&[preserved])
            .expect("abort");
    }

    #[test]
    fn connect_invalid_argument_maps_to_coalescer_split_signal() {
        let error = map_cursor_error(CursorBackendError::HttpStatus {
            status: 400,
            body: "invalid_argument".into(),
        });
        assert_eq!(error.code, SandBoxStoreClientErrorCode::InvalidArgument);
    }

    #[test]
    fn completion_results_are_reordered_by_input_index() {
        let results = vec![
            SandBoxStoreMultipartWriteResultProto {
                input_index: 1,
                rel_path: "b".into(),
                outcome: Some(
                    sand_box_store_multipart_write_result_proto::Outcome::Failure(
                        SandBoxStoreMultipartOperationFailureProto { code: 5 },
                    ),
                ),
            },
            SandBoxStoreMultipartWriteResultProto {
                input_index: 0,
                rel_path: "a".into(),
                outcome: Some(
                    sand_box_store_multipart_write_result_proto::Outcome::Success(
                        SandBoxStoreMultipartWriteSuccessProto {
                            etag: "ok".into(),
                        },
                    ),
                ),
            },
        ];
        assert_eq!(
            indexed_completion_outcomes(results, 2).expect("outcomes"),
            vec![
                SandBoxStoreMultipartCompletionOutcome::Success,
                SandBoxStoreMultipartCompletionOutcome::Failure { code: 5 },
            ]
        );
    }
}
