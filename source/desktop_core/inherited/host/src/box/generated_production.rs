use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use url::Url;

use super::box_env::{BoxEnvironmentControlClient, BoxEnvironmentUpdate};
use super::box_file_transfer::{FileTransferAccessor, ShellExecResult, WriteExecResult};
use super::box_mcp::{
    BoxMcpControlClient, BoxMcpLoadRequest, BoxMcpLoadResponse, ConnectErrorCode,
    ConnectErrorCodeSource,
};
use super::box_remote_accessor::{
    BoxEndpoint, BoxPingControlClient, BoxPingErrorMetadata, BoxRemoteExecClient,
    BoxRemoteExecControlMessage, BoxRemoteExecEnvelope, BoxRemoteExecError, BoxRemoteExecManager,
    BoxTransportOptions, ConnectCode, create_box_transport,
};
use super::box_shell_command::{HostShellArgs, ShellCommandParsingResult};
use super::box_windows::{ShellAccessor, ShellExecutionOutcome, ShellExecutionResult};
use super::protected_path_guard::{SandProtectedPathError, assert_path_outside_protected_roots};
use crate::ports::r#box::SandBoxNoMonitorAvailableError;

pub const PING_PATH: &str = "/agent.v1.ControlService/Ping";
pub const UPDATE_ENVIRONMENT_VARIABLES_PATH: &str =
    "/agent.v1.ControlService/UpdateEnvironmentVariables";
pub const LOAD_MCP_SERVERS_PATH: &str = "/agent.v1.ControlService/LoadMcpServers";
pub const EXEC_PATH: &str = "/agent.v1.ExecService/Exec";
pub const CONNECT_PROTOCOL_VERSION: &str = "1";
pub const CONNECT_STREAM_CONTENT_TYPE: &str = "application/connect+proto";
pub const CONNECT_STREAM_END_FLAG: u8 = 0x02;
pub const PRODUCTION_BOX_RPC_TIMEOUT_MS: u64 = 15_000;
/// Exec calls can contain bounded browser navigation stages longer than unary RPC.
pub const PRODUCTION_BOX_EXEC_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug)]
pub enum ProductionBoxTransportError {
    InvalidBaseUrl(String),
    InvalidHeader(String),
    Io(std::io::Error),
    InvalidHttpResponse(String),
    HttpStatus { status: u16, body: String },
}

impl fmt::Display for ProductionBoxTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl(message)
            | Self::InvalidHeader(message)
            | Self::InvalidHttpResponse(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
            Self::HttpStatus { status, body } => {
                write!(formatter, "box ControlService returned HTTP {status}")?;
                if !body.is_empty() {
                    write!(formatter, ": {body}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ProductionBoxTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ProductionBoxTransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn http_status_connect_code(status: u16) -> Option<i64> {
    match status {
        400 => Some(3),
        401 => Some(16),
        403 => Some(7),
        404 => Some(5),
        409 => Some(10),
        429 => Some(8),
        499 => Some(1),
        501 => Some(12),
        503 => Some(14),
        504 => Some(4),
        _ => None,
    }
}

impl BoxPingErrorMetadata for ProductionBoxTransportError {
    fn connect_code(&self) -> Option<ConnectCode> {
        match self {
            Self::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                Some(ConnectCode::Number(4))
            }
            Self::HttpStatus { status, .. } => {
                http_status_connect_code(*status).map(ConnectCode::Number)
            }
            _ => None,
        }
    }

    fn system_errno(&self) -> Option<&str> {
        match self {
            Self::Io(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                Some("ECONNREFUSED")
            }
            _ => None,
        }
    }
}

impl ConnectErrorCodeSource for ProductionBoxTransportError {
    fn connect_error_code(&self) -> Option<ConnectErrorCode> {
        match self {
            Self::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                Some(ConnectErrorCode::Number(4))
            }
            Self::HttpStatus { status, .. } => {
                http_status_connect_code(*status).map(ConnectErrorCode::Number)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionBoxTransport {
    options: BoxTransportOptions,
}

impl ProductionBoxTransport {
    pub fn from_endpoint(endpoint: &BoxEndpoint) -> Self {
        create_box_transport(endpoint, |options| Self { options })
    }

    pub fn options(&self) -> &BoxTransportOptions {
        &self.options
    }

    fn request_headers(&self) -> Result<BTreeMap<String, String>, ProductionBoxTransportError> {
        let mut headers = BTreeMap::new();
        for interceptor in &self.options.interceptors {
            interceptor.apply(&mut headers);
        }
        for (name, value) in &headers {
            validate_header(name, value)?;
        }
        Ok(headers)
    }
}

pub struct ProductionBoxControlClient {
    transport: ProductionBoxTransport,
}

pub fn create_production_box_control_client(
    transport: &ProductionBoxTransport,
) -> ProductionBoxControlClient {
    ProductionBoxControlClient {
        transport: transport.clone(),
    }
}

impl<Ctx> BoxPingControlClient<Ctx> for ProductionBoxControlClient {
    type Error = ProductionBoxTransportError;

    fn ping(&mut self, _ctx: &Ctx, _timeout_ms: u64) -> Result<(), Self::Error> {
        send_connect_unary(&self.transport, PING_PATH, &[])
    }
}

impl<Ctx> BoxEnvironmentControlClient<Ctx> for ProductionBoxControlClient {
    type Error = ProductionBoxTransportError;

    fn update_environment_variables(
        &mut self,
        _ctx: &Ctx,
        request: BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error> {
        let body = encode_update_environment_variables_request(&request);
        send_connect_unary(
            &self.transport,
            UPDATE_ENVIRONMENT_VARIABLES_PATH,
            &body,
        )
    }
}

impl<Ctx> BoxMcpControlClient<Ctx> for ProductionBoxControlClient {
    type Error = ProductionBoxTransportError;

    fn load_mcp_servers(
        &mut self,
        _ctx: &Ctx,
        request: BoxMcpLoadRequest,
    ) -> Result<BoxMcpLoadResponse, Self::Error> {
        let body = encode_load_mcp_servers_request(&request);
        let response = send_connect_unary_response(
            &self.transport,
            LOAD_MCP_SERVERS_PATH,
            &body,
        )?;
        Ok(BoxMcpLoadResponse {
            loaded_server_names: decode_load_mcp_servers_response(&response)?,
        })
    }
}

pub const BOX_GENERATED_PROTOBUF_VERSION: &str = "1.10.1";
pub const BOX_GENERATED_CONNECT_VERSION: &str = "1.6.1";
pub const BOX_GENERATED_CONNECT_NODE_VERSION: &str = "1.6.1";
pub const SHELL_STREAM_FIELD_NUMBER: u32 = 14;
pub const BACKGROUND_SHELL_SPAWN_FIELD_NUMBER: u32 = 16;
pub const SHELL_STREAM_TIMEOUT_BEHAVIOR_BACKGROUND: i32 = 2;
pub const SHELL_BACKGROUND_REASON_TIMEOUT: i32 = 1;
pub const SHELL_BACKGROUND_REASON_USER_REQUEST: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionReadArgs {
    pub path: String,
    pub tool_call_id: String,
    pub offset: Option<i32>,
    pub limit: Option<u32>,
    pub encoding_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionBackgroundShellSpawnArgs {
    pub command: String,
    pub working_directory: String,
    pub tool_call_id: String,
    pub skip_approval: bool,
    pub parsing_result: ShellCommandParsingResult,
}

impl From<HostShellArgs> for ProductionBackgroundShellSpawnArgs {
    fn from(args: HostShellArgs) -> Self {
        Self {
            command: args.command,
            working_directory: args.working_directory,
            tool_call_id: args.tool_call_id,
            skip_approval: args.skip_approval,
            parsing_result: args.parsing_result,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionBackgroundShellSpawnResult {
    Success { shell_id: u32, command: String, working_directory: String, pid: Option<u32> },
    Error { command: String, working_directory: String, error: String },
    Rejected { command: String, working_directory: String, reason: String, is_readonly: bool },
    PermissionDenied { command: String, working_directory: String, error: String, is_readonly: bool },
    SandboxUnsupported { command: String, working_directory: String, sandbox_policy_type: i32, reason: String, is_readonly: bool },
    Other { case: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionShellStreamArgs {
    pub shell_args: HostShellArgs,
    pub timeout_ms: i32,
    pub hard_timeout_ms: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionShellStreamEvent {
    Start,
    Stdout(String),
    Stderr(String),
    Exit {
        code: u32,
        cwd: String,
        aborted: bool,
        local_execution_time_ms: Option<i32>,
    },
    Rejected {
        command: String,
        working_directory: String,
        reason: String,
        is_readonly: bool,
    },
    PermissionDenied {
        command: String,
        working_directory: String,
        error: String,
        is_readonly: bool,
    },
    Backgrounded {
        shell_id: u32,
        command: String,
        working_directory: String,
        pid: Option<u32>,
        ms_to_wait: Option<i32>,
        reason: Option<i32>,
    },
    Other {
        case: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionReadOutput {
    Content(String),
    Data(Vec<u8>),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionReadResult {
    Success {
        path: String,
        output: ProductionReadOutput,
        total_lines: i32,
        file_size: i64,
        truncated: bool,
        output_blob_id: Option<Vec<u8>>,
        range_applied: bool,
    },
    Error { path: String, error: String },
    Rejected { path: String, reason: String },
    FileNotFound { path: String },
    PermissionDenied { path: String },
    InvalidFile { path: String, reason: String },
    Other { case: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionExecRequest {
    Shell { id: u64, args: HostShellArgs },
    Write {
        id: u64,
        path: String,
        file_bytes: Vec<u8>,
        tool_call_id: String,
    },
    Read { id: u64, args: ProductionReadArgs },
    ComputerUse { id: u64, protobuf_args: Vec<u8> },
    RawResource {
        id: u64,
        field_number: u32,
        protobuf_args: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionShellResult {
    Success {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    Failure {
        exit_code: i32,
        signal: String,
        stdout: String,
        stderr: String,
        aborted: bool,
    },
    SpawnError { error: String },
    PermissionDenied { error: String },
    Rejected { reason: String },
    Timeout { timeout_ms: u64 },
    Other { case: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionExecClientMessage {
    Shell(ProductionShellResult),
    Write(WriteExecResult),
    Read(ProductionReadResult),
    ComputerUse(Vec<u8>),
    RawResource {
        field_number: u32,
        protobuf_result: Vec<u8>,
    },
    Other,
}

#[derive(Debug)]
pub enum ProductionBoxExecError {
    Remote(BoxRemoteExecError<ProductionBoxTransportError>),
    ProtectedPath(SandProtectedPathError),
    NoMonitor(SandBoxNoMonitorAvailableError),
    InvalidResult(ProductionBoxTransportError),
    MissingResult(&'static str),
}

impl fmt::Display for ProductionBoxExecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Remote(error) => error.fmt(formatter),
            Self::ProtectedPath(error) => error.fmt(formatter),
            Self::NoMonitor(error) => error.fmt(formatter),
            Self::InvalidResult(error) => error.fmt(formatter),
            Self::MissingResult(kind) => {
                write!(formatter, "box ExecService closed without a {kind} result")
            }
        }
    }
}

impl std::error::Error for ProductionBoxExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Remote(error) => Some(error),
            Self::ProtectedPath(error) => Some(error),
            Self::NoMonitor(error) => Some(error),
            Self::InvalidResult(error) => Some(error),
            Self::MissingResult(_) => None,
        }
    }
}

impl From<BoxRemoteExecError<ProductionBoxTransportError>> for ProductionBoxExecError {
    fn from(error: BoxRemoteExecError<ProductionBoxTransportError>) -> Self {
        Self::Remote(error)
    }
}

impl From<SandProtectedPathError> for ProductionBoxExecError {
    fn from(error: SandProtectedPathError) -> Self {
        Self::ProtectedPath(error)
    }
}

impl From<SandBoxNoMonitorAvailableError> for ProductionBoxExecError {
    fn from(error: SandBoxNoMonitorAvailableError) -> Self {
        Self::NoMonitor(error)
    }
}

impl From<ProductionBoxTransportError> for ProductionBoxExecError {
    fn from(error: ProductionBoxTransportError) -> Self {
        Self::InvalidResult(error)
    }
}

pub struct ProductionBoxExecClient {
    transport: ProductionBoxTransport,
}

pub fn create_production_box_exec_client(
    transport: &ProductionBoxTransport,
) -> ProductionBoxExecClient {
    ProductionBoxExecClient {
        transport: transport.clone(),
    }
}

impl<Ctx> BoxRemoteExecClient<Ctx, ProductionExecRequest, ProductionExecClientMessage>
    for ProductionBoxExecClient
{
    type Error = ProductionBoxTransportError;

    fn exec(
        &mut self,
        _ctx: &Ctx,
        args: ProductionExecRequest,
    ) -> Result<Vec<BoxRemoteExecEnvelope<ProductionExecClientMessage>>, Self::Error> {
        let request = encode_exec_server_message(&args);
        send_connect_server_stream(&self.transport, EXEC_PATH, &request)
    }
}

pub struct ProductionBoxResourceAccessor {
    manager: BoxRemoteExecManager<ProductionBoxExecClient>,
    protected_read_roots: Vec<PathBuf>,
    no_monitor_computer_use: bool,
}

pub fn create_production_box_resource_accessor(
    transport: &ProductionBoxTransport,
) -> ProductionBoxResourceAccessor {
    ProductionBoxResourceAccessor {
        manager: BoxRemoteExecManager::new(create_production_box_exec_client(transport)),
        protected_read_roots: Vec::new(),
        no_monitor_computer_use: false,
    }
}

impl ProductionBoxResourceAccessor {
    pub fn with_file_read_guard(mut self, protected_read_roots: Vec<PathBuf>) -> Self {
        self.protected_read_roots = protected_read_roots;
        self
    }

    pub fn with_no_monitor_computer_use(mut self) -> Self {
        self.no_monitor_computer_use = true;
        self
    }

    pub fn execute_read<Ctx>(
        &mut self,
        ctx: &Ctx,
        args: ProductionReadArgs,
    ) -> Result<ProductionReadResult, ProductionBoxExecError> {
        assert_path_outside_protected_roots(
            &self.protected_read_roots,
            Path::new(&args.path),
            Path::new("/workspace"),
        )?;
        let messages = self.manager.create_exec_instance(ctx, |id| {
            ProductionExecRequest::Read { id, args }
        })?;
        messages
            .into_iter()
            .find_map(|message| match message {
                ProductionExecClientMessage::Read(result) => Some(result),
                _ => None,
            })
            .ok_or(ProductionBoxExecError::MissingResult("read"))
    }

    pub fn execute_computer_use_protobuf<Ctx>(
        &mut self,
        ctx: &Ctx,
        protobuf_args: Vec<u8>,
    ) -> Result<Vec<u8>, ProductionBoxExecError> {
        if self.no_monitor_computer_use {
            return Err(SandBoxNoMonitorAvailableError::default().into());
        }
        let messages = self.manager.create_exec_instance(ctx, |id| {
            ProductionExecRequest::ComputerUse { id, protobuf_args }
        })?;
        messages
            .into_iter()
            .find_map(|message| match message {
                ProductionExecClientMessage::ComputerUse(result) => Some(result),
                _ => None,
            })
            .ok_or(ProductionBoxExecError::MissingResult("computer-use"))
    }

    pub fn execute_raw_resource<Ctx>(
        &mut self,
        ctx: &Ctx,
        field_number: u32,
        protobuf_args: Vec<u8>,
    ) -> Result<Vec<u8>, ProductionBoxExecError> {
        let messages = self.manager.create_exec_instance(ctx, |id| {
            ProductionExecRequest::RawResource {
                id,
                field_number,
                protobuf_args,
            }
        })?;
        messages
            .into_iter()
            .find_map(|message| match message {
                ProductionExecClientMessage::RawResource {
                    field_number: returned_field,
                    protobuf_result,
                } if returned_field == field_number => Some(protobuf_result),
                _ => None,
            })
            .ok_or(ProductionBoxExecError::MissingResult("raw-resource"))
    }

    pub fn execute_background_shell_spawn<Ctx>(
        &mut self,
        ctx: &Ctx,
        args: ProductionBackgroundShellSpawnArgs,
    ) -> Result<ProductionBackgroundShellSpawnResult, ProductionBoxExecError> {
        let protobuf_args = encode_background_shell_spawn_args(&args);
        let protobuf_result =
            self.execute_raw_resource(ctx, BACKGROUND_SHELL_SPAWN_FIELD_NUMBER, protobuf_args)?;
        Ok(decode_background_shell_spawn_result(&protobuf_result)?)
    }

    pub fn execute_shell_stream<Ctx>(
        &mut self,
        ctx: &Ctx,
        args: ProductionShellStreamArgs,
    ) -> Result<Vec<ProductionShellStreamEvent>, ProductionBoxExecError> {
        let protobuf_args = encode_shell_stream_args(&args);
        let messages = self.manager.create_exec_instance(ctx, |id| {
            ProductionExecRequest::RawResource {
                id,
                field_number: SHELL_STREAM_FIELD_NUMBER,
                protobuf_args,
            }
        })?;
        let mut events = Vec::new();
        for message in messages {
            if let ProductionExecClientMessage::RawResource {
                field_number,
                protobuf_result,
            } = message
            {
                if field_number == SHELL_STREAM_FIELD_NUMBER {
                    events.push(decode_shell_stream_event(&protobuf_result)?);
                }
            }
        }
        if events.is_empty() {
            return Err(ProductionBoxExecError::MissingResult("shell-stream"));
        }
        Ok(events)
    }

    pub fn execute_shell_result<Ctx>(
        &mut self,
        ctx: &Ctx,
        args: HostShellArgs,
    ) -> Result<ProductionShellResult, ProductionBoxExecError> {
        let messages = self.manager.create_exec_instance(ctx, |id| {
            ProductionExecRequest::Shell { id, args }
        })?;
        messages
            .into_iter()
            .find_map(|message| match message {
                ProductionExecClientMessage::Shell(result) => Some(result),
                _ => None,
            })
            .ok_or(ProductionBoxExecError::MissingResult("shell"))
    }
}

impl<Ctx> ShellAccessor<Ctx> for ProductionBoxResourceAccessor {
    type Error = ProductionBoxExecError;

    fn execute(
        &mut self,
        ctx: &Ctx,
        args: HostShellArgs,
    ) -> Result<ShellExecutionResult, Self::Error> {
        let result = self.execute_shell_result(ctx, args)?;
        Ok(match result {
            ProductionShellResult::Success {
                exit_code, stderr, ..
            } => ShellExecutionResult {
                result: ShellExecutionOutcome::Success { exit_code, stderr },
            },
            ProductionShellResult::Failure {
                exit_code,
                signal,
                stderr,
                aborted,
                ..
            } => ShellExecutionResult {
                result: ShellExecutionOutcome::Failure {
                    case: format!(
                        "failure(exit={exit_code},signal={signal},aborted={aborted},stderr={stderr})"
                    ),
                },
            },
            ProductionShellResult::SpawnError { error } => ShellExecutionResult {
                result: ShellExecutionOutcome::Failure {
                    case: format!("spawn_error({error})"),
                },
            },
            ProductionShellResult::PermissionDenied { error } => ShellExecutionResult {
                result: ShellExecutionOutcome::Failure {
                    case: format!("permission_denied({error})"),
                },
            },
            ProductionShellResult::Rejected { reason } => ShellExecutionResult {
                result: ShellExecutionOutcome::Failure {
                    case: format!("rejected({reason})"),
                },
            },
            ProductionShellResult::Timeout { timeout_ms } => ShellExecutionResult {
                result: ShellExecutionOutcome::Failure {
                    case: format!("timeout({timeout_ms}ms)"),
                },
            },
            ProductionShellResult::Other { case } => ShellExecutionResult {
                result: ShellExecutionOutcome::Failure { case },
            },
        })
    }
}

impl<Ctx> FileTransferAccessor<Ctx> for ProductionBoxResourceAccessor {
    type Error = ProductionBoxExecError;

    fn execute_shell(
        &mut self,
        ctx: &Ctx,
        args: HostShellArgs,
    ) -> Result<ShellExecResult, Self::Error> {
        let result = self.execute_shell_result(ctx, args)?;
        Ok(match result {
            ProductionShellResult::Success { exit_code, .. } => {
                ShellExecResult::Success { exit_code }
            }
            ProductionShellResult::Failure {
                exit_code,
                signal,
                stderr,
                aborted,
                ..
            } => ShellExecResult::Failure {
                exit_code,
                signal,
                stderr,
                aborted,
            },
            ProductionShellResult::SpawnError { error } => ShellExecResult::SpawnError { error },
            ProductionShellResult::PermissionDenied { error } => {
                ShellExecResult::PermissionDenied { error }
            }
            ProductionShellResult::Rejected { reason } => ShellExecResult::Rejected { reason },
            ProductionShellResult::Timeout { timeout_ms } => {
                ShellExecResult::Timeout { timeout_ms }
            }
            ProductionShellResult::Other { case } => ShellExecResult::Other { case },
        })
    }

    fn execute_write(
        &mut self,
        ctx: &Ctx,
        path: &str,
        file_bytes: &[u8],
        tool_call_id: &str,
    ) -> Result<WriteExecResult, Self::Error> {
        let messages = self.manager.create_exec_instance(ctx, |id| {
            ProductionExecRequest::Write {
                id,
                path: path.to_string(),
                file_bytes: file_bytes.to_vec(),
                tool_call_id: tool_call_id.to_string(),
            }
        })?;
        messages
            .into_iter()
            .find_map(|message| match message {
                ProductionExecClientMessage::Write(result) => Some(result),
                _ => None,
            })
            .ok_or(ProductionBoxExecError::MissingResult("write"))
    }
}

fn encode_key(field_number: u32, wire_type: u8, out: &mut Vec<u8>) {
    encode_varint(((field_number as usize) << 3) | usize::from(wire_type), out);
}

fn encode_uint32_field(field_number: u32, value: u64, out: &mut Vec<u8>) {
    encode_key(field_number, 0, out);
    encode_varint(
        usize::try_from(value.min(u64::from(u32::MAX))).unwrap_or(u32::MAX as usize),
        out,
    );
}

fn encode_bool_field(field_number: u32, value: bool, out: &mut Vec<u8>) {
    if value {
        encode_key(field_number, 0, out);
        out.push(1);
    }
}

fn encode_shell_parsing_result(parsing_result: &ShellCommandParsingResult) -> Vec<u8> {
    let mut body = Vec::new();
    if parsing_result.parsing_failed {
        encode_bool_field(1, true, &mut body);
    }
    for executable in &parsing_result.executable_commands {
        let mut command = Vec::new();
        if !executable.name.is_empty() {
            encode_len_delimited(1, executable.name.as_bytes(), &mut command);
        }
        for argument in &executable.args {
            let mut encoded_argument = Vec::new();
            encode_len_delimited(2, argument.as_bytes(), &mut encoded_argument);
            encode_len_delimited(2, &encoded_argument, &mut command);
        }
        if !executable.full_text.is_empty() {
            encode_len_delimited(3, executable.full_text.as_bytes(), &mut command);
        }
        encode_len_delimited(2, &command, &mut body);
    }
    if parsing_result.has_redirects {
        encode_bool_field(3, true, &mut body);
    }
    if parsing_result.has_command_substitution {
        encode_bool_field(4, true, &mut body);
    }
    body
}

fn encode_shell_args(args: &HostShellArgs) -> Vec<u8> {
    let mut body = Vec::new();
    if !args.command.is_empty() {
        encode_len_delimited(1, args.command.as_bytes(), &mut body);
    }
    if !args.working_directory.is_empty() {
        encode_len_delimited(2, args.working_directory.as_bytes(), &mut body);
    }
    if !args.tool_call_id.is_empty() {
        encode_len_delimited(4, args.tool_call_id.as_bytes(), &mut body);
    }
    let parsing = encode_shell_parsing_result(&args.parsing_result);
    if !parsing.is_empty() {
        encode_len_delimited(8, &parsing, &mut body);
    }
    encode_bool_field(12, args.skip_approval, &mut body);
    body
}

pub fn encode_shell_stream_args(args: &ProductionShellStreamArgs) -> Vec<u8> {
    let mut body = Vec::new();
    let shell = &args.shell_args;
    if !shell.command.is_empty() {
        encode_len_delimited(1, shell.command.as_bytes(), &mut body);
    }
    if !shell.working_directory.is_empty() {
        encode_len_delimited(2, shell.working_directory.as_bytes(), &mut body);
    }
    encode_int32_field(3, args.timeout_ms, &mut body);
    if !shell.tool_call_id.is_empty() {
        encode_len_delimited(4, shell.tool_call_id.as_bytes(), &mut body);
    }
    let parsing = encode_shell_parsing_result(&shell.parsing_result);
    if !parsing.is_empty() {
        encode_len_delimited(8, &parsing, &mut body);
    }
    encode_bool_field(12, shell.skip_approval, &mut body);
    encode_int32_field(
        13,
        SHELL_STREAM_TIMEOUT_BEHAVIOR_BACKGROUND,
        &mut body,
    );
    if let Some(hard_timeout_ms) = args.hard_timeout_ms {
        encode_int32_field(14, hard_timeout_ms, &mut body);
    }
    body
}

pub fn encode_background_shell_spawn_args(args: &ProductionBackgroundShellSpawnArgs) -> Vec<u8> {
    let mut body = Vec::new();
    if !args.command.is_empty() {
        encode_len_delimited(1, args.command.as_bytes(), &mut body);
    }
    if !args.working_directory.is_empty() {
        encode_len_delimited(2, args.working_directory.as_bytes(), &mut body);
    }
    if !args.tool_call_id.is_empty() {
        encode_len_delimited(3, args.tool_call_id.as_bytes(), &mut body);
    }
    let parsing = encode_shell_parsing_result(&args.parsing_result);
    if !parsing.is_empty() {
        encode_len_delimited(4, &parsing, &mut body);
    }
    encode_bool_field(12, args.skip_approval, &mut body);
    body
}

fn encode_write_args(path: &str, file_bytes: &[u8], tool_call_id: &str) -> Vec<u8> {
    let mut body = Vec::new();
    if !path.is_empty() {
        encode_len_delimited(1, path.as_bytes(), &mut body);
    }
    if !tool_call_id.is_empty() {
        encode_len_delimited(3, tool_call_id.as_bytes(), &mut body);
    }
    if !file_bytes.is_empty() {
        encode_len_delimited(5, file_bytes, &mut body);
    }
    body
}

fn encode_int32_field(field_number: u32, value: i32, out: &mut Vec<u8>) {
    encode_key(field_number, 0, out);
    let encoded = value as i64 as u64;
    let encoded = usize::try_from(encoded).unwrap_or(usize::MAX);
    encode_varint(encoded, out);
}

fn encode_read_args(args: &ProductionReadArgs) -> Vec<u8> {
    let mut body = Vec::new();
    if !args.path.is_empty() {
        encode_len_delimited(1, args.path.as_bytes(), &mut body);
    }
    if !args.tool_call_id.is_empty() {
        encode_len_delimited(2, args.tool_call_id.as_bytes(), &mut body);
    }
    if let Some(offset) = args.offset {
        encode_int32_field(4, offset, &mut body);
    }
    if let Some(limit) = args.limit {
        encode_uint32_field(5, u64::from(limit), &mut body);
    }
    if let Some(encoding_hint) = args.encoding_hint.as_deref().filter(|value| !value.is_empty()) {
        encode_len_delimited(6, encoding_hint.as_bytes(), &mut body);
    }
    body
}

pub fn encode_exec_server_message(request: &ProductionExecRequest) -> Vec<u8> {
    let mut body = Vec::new();
    match request {
        ProductionExecRequest::Shell { id, args } => {
            encode_uint32_field(1, *id, &mut body);
            let args = encode_shell_args(args);
            encode_len_delimited(2, &args, &mut body);
        }
        ProductionExecRequest::Write {
            id,
            path,
            file_bytes,
            tool_call_id,
        } => {
            encode_uint32_field(1, *id, &mut body);
            let args = encode_write_args(path, file_bytes, tool_call_id);
            encode_len_delimited(3, &args, &mut body);
        }
        ProductionExecRequest::Read { id, args } => {
            encode_uint32_field(1, *id, &mut body);
            let args = encode_read_args(args);
            encode_len_delimited(7, &args, &mut body);
        }
        ProductionExecRequest::ComputerUse { id, protobuf_args } => {
            encode_uint32_field(1, *id, &mut body);
            encode_len_delimited(22, protobuf_args, &mut body);
        }
        ProductionExecRequest::RawResource {
            id,
            field_number,
            protobuf_args,
        } => {
            encode_uint32_field(1, *id, &mut body);
            encode_len_delimited(*field_number, protobuf_args, &mut body);
        }
    }
    body
}

fn decode_len_delimited<'a>(
    input: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], ProductionBoxTransportError> {
    let length = usize::try_from(decode_varint(input, cursor)?).map_err(|_| {
        ProductionBoxTransportError::InvalidHttpResponse(
            "box ExecService protobuf length overflow".into(),
        )
    })?;
    let end = cursor.saturating_add(length);
    if end > input.len() {
        return Err(ProductionBoxTransportError::InvalidHttpResponse(
            "box ExecService returned truncated protobuf".into(),
        ));
    }
    let value = &input[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn decode_u64_field(
    input: &[u8],
    wanted: u64,
) -> Result<Option<u64>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted && wire == 0 {
            return Ok(Some(decode_varint(input, &mut cursor)?));
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(None)
}

fn decode_i32_field(
    input: &[u8],
    wanted: u64,
) -> Result<Option<i32>, ProductionBoxTransportError> {
    Ok(decode_u64_field(input, wanted)?.map(|value| value as u32 as i32))
}

fn decode_string_field(
    input: &[u8],
    wanted: u64,
) -> Result<Option<String>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted && wire == 2 {
            let value = decode_len_delimited(input, &mut cursor)?;
            let value = std::str::from_utf8(value).map_err(|_| {
                ProductionBoxTransportError::InvalidHttpResponse(
                    "box ExecService returned non-UTF-8 protobuf string".into(),
                )
            })?;
            return Ok(Some(value.to_string()));
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(None)
}

fn decode_bytes_field(
    input: &[u8],
    wanted: u64,
) -> Result<Option<Vec<u8>>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted && wire == 2 {
            return Ok(Some(decode_len_delimited(input, &mut cursor)?.to_vec()));
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(None)
}

fn decode_read_result(
    input: &[u8],
) -> Result<ProductionReadResult, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return match field {
                1 => {
                    let output = if let Some(content) = decode_string_field(nested, 2)? {
                        ProductionReadOutput::Content(content)
                    } else if let Some(data) = decode_bytes_field(nested, 5)? {
                        ProductionReadOutput::Data(data)
                    } else {
                        ProductionReadOutput::None
                    };
                    Ok(ProductionReadResult::Success {
                        path: decode_string_field(nested, 1)?.unwrap_or_default(),
                        output,
                        total_lines: decode_u64_field(nested, 3)?.unwrap_or_default() as u32 as i32,
                        file_size: decode_u64_field(nested, 4)?.unwrap_or_default() as i64,
                        truncated: decode_u64_field(nested, 6)?.unwrap_or_default() != 0,
                        output_blob_id: decode_bytes_field(nested, 7)?,
                        range_applied: decode_u64_field(nested, 8)?.unwrap_or_default() != 0,
                    })
                }
                2 => Ok(ProductionReadResult::Error {
                    path: decode_string_field(nested, 1)?.unwrap_or_default(),
                    error: decode_string_field(nested, 2)?.unwrap_or_default(),
                }),
                3 => Ok(ProductionReadResult::Rejected {
                    path: decode_string_field(nested, 1)?.unwrap_or_default(),
                    reason: decode_string_field(nested, 2)?.unwrap_or_default(),
                }),
                4 => Ok(ProductionReadResult::FileNotFound {
                    path: decode_string_field(nested, 1)?.unwrap_or_default(),
                }),
                5 => Ok(ProductionReadResult::PermissionDenied {
                    path: decode_string_field(nested, 1)?.unwrap_or_default(),
                }),
                6 => Ok(ProductionReadResult::InvalidFile {
                    path: decode_string_field(nested, 1)?.unwrap_or_default(),
                    reason: decode_string_field(nested, 2)?.unwrap_or_default(),
                }),
                _ => Ok(ProductionReadResult::Other {
                    case: format!("read_result_field_{field}"),
                }),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(ProductionReadResult::Other {
        case: "missing_read_result".into(),
    })
}

fn decode_shell_success(
    input: &[u8],
) -> Result<ProductionShellResult, ProductionBoxTransportError> {
    let exit_code = decode_u64_field(input, 3)?.unwrap_or_default() as u32 as i32;
    let stdout = decode_string_field(input, 5)?.unwrap_or_default();
    let stderr = decode_string_field(input, 6)?.unwrap_or_default();
    Ok(ProductionShellResult::Success {
        exit_code,
        stdout,
        stderr,
    })
}

fn decode_shell_failure(
    input: &[u8],
) -> Result<ProductionShellResult, ProductionBoxTransportError> {
    Ok(ProductionShellResult::Failure {
        exit_code: decode_u64_field(input, 3)?.unwrap_or_default() as u32 as i32,
        signal: decode_string_field(input, 4)?.unwrap_or_default(),
        stdout: decode_string_field(input, 5)?.unwrap_or_default(),
        stderr: decode_string_field(input, 6)?.unwrap_or_default(),
        aborted: decode_u64_field(input, 11)?.unwrap_or_default() != 0,
    })
}

fn decode_shell_result(
    input: &[u8],
) -> Result<ProductionShellResult, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return match field {
                1 => decode_shell_success(nested),
                2 => decode_shell_failure(nested),
                3 => Ok(ProductionShellResult::Timeout {
                    timeout_ms: decode_u64_field(nested, 3)?.unwrap_or_default(),
                }),
                4 => Ok(ProductionShellResult::Rejected {
                    reason: decode_string_field(nested, 3)?.unwrap_or_default(),
                }),
                5 => Ok(ProductionShellResult::SpawnError {
                    error: decode_string_field(nested, 3)?.unwrap_or_default(),
                }),
                7 => Ok(ProductionShellResult::PermissionDenied {
                    error: decode_string_field(nested, 3)?.unwrap_or_default(),
                }),
                _ => Ok(ProductionShellResult::Other {
                    case: format!("shell_result_field_{field}"),
                }),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(ProductionShellResult::Other {
        case: "missing_shell_result".into(),
    })
}

pub fn decode_background_shell_spawn_result(
    input: &[u8],
) -> Result<ProductionBackgroundShellSpawnResult, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 {
            let nested = decode_len_delimited(input, &mut cursor)?;
            let command = decode_string_field(nested, 1)?.unwrap_or_default();
            let working_directory = decode_string_field(nested, 2)?.unwrap_or_default();
            return match field {
                1 => Ok(ProductionBackgroundShellSpawnResult::Success {
                    shell_id: decode_u64_field(nested, 1)?.unwrap_or_default().min(u64::from(u32::MAX)) as u32,
                    command: decode_string_field(nested, 2)?.unwrap_or_default(),
                    working_directory: decode_string_field(nested, 3)?.unwrap_or_default(),
                    pid: decode_u64_field(nested, 4)?.map(|value| value.min(u64::from(u32::MAX)) as u32),
                }),
                2 => Ok(ProductionBackgroundShellSpawnResult::Error {
                    command,
                    working_directory,
                    error: decode_string_field(nested, 3)?.unwrap_or_default(),
                }),
                3 => Ok(ProductionBackgroundShellSpawnResult::Rejected {
                    command,
                    working_directory,
                    reason: decode_string_field(nested, 3)?.unwrap_or_default(),
                    is_readonly: decode_u64_field(nested, 4)?.unwrap_or_default() != 0,
                }),
                4 => Ok(ProductionBackgroundShellSpawnResult::PermissionDenied {
                    command,
                    working_directory,
                    error: decode_string_field(nested, 3)?.unwrap_or_default(),
                    is_readonly: decode_u64_field(nested, 4)?.unwrap_or_default() != 0,
                }),
                5 => Ok(ProductionBackgroundShellSpawnResult::SandboxUnsupported {
                    command,
                    working_directory,
                    sandbox_policy_type: decode_u64_field(nested, 3)?.unwrap_or_default() as u32 as i32,
                    reason: decode_string_field(nested, 4)?.unwrap_or_default(),
                    is_readonly: decode_u64_field(nested, 5)?.unwrap_or_default() != 0,
                }),
                _ => Ok(ProductionBackgroundShellSpawnResult::Other {
                    case: format!("background_shell_spawn_result_field_{field}"),
                }),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(ProductionBackgroundShellSpawnResult::Other {
        case: "missing_background_shell_spawn_result".into(),
    })
}

pub fn decode_shell_stream_event(
    input: &[u8],
) -> Result<ProductionShellStreamEvent, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return match field {
                1 => Ok(ProductionShellStreamEvent::Stdout(
                    decode_string_field(nested, 1)?.unwrap_or_default(),
                )),
                2 => Ok(ProductionShellStreamEvent::Stderr(
                    decode_string_field(nested, 1)?.unwrap_or_default(),
                )),
                3 => Ok(ProductionShellStreamEvent::Exit {
                    code: decode_u64_field(nested, 1)?
                        .unwrap_or_default()
                        .min(u64::from(u32::MAX)) as u32,
                    cwd: decode_string_field(nested, 2)?.unwrap_or_default(),
                    aborted: decode_u64_field(nested, 4)?.unwrap_or_default() != 0,
                    local_execution_time_ms: decode_i32_field(nested, 6)?,
                }),
                4 => Ok(ProductionShellStreamEvent::Start),
                5 => Ok(ProductionShellStreamEvent::Rejected {
                    command: decode_string_field(nested, 1)?.unwrap_or_default(),
                    working_directory: decode_string_field(nested, 2)?.unwrap_or_default(),
                    reason: decode_string_field(nested, 3)?.unwrap_or_default(),
                    is_readonly: decode_u64_field(nested, 4)?.unwrap_or_default() != 0,
                }),
                6 => Ok(ProductionShellStreamEvent::PermissionDenied {
                    command: decode_string_field(nested, 1)?.unwrap_or_default(),
                    working_directory: decode_string_field(nested, 2)?.unwrap_or_default(),
                    error: decode_string_field(nested, 3)?.unwrap_or_default(),
                    is_readonly: decode_u64_field(nested, 4)?.unwrap_or_default() != 0,
                }),
                7 => Ok(ProductionShellStreamEvent::Backgrounded {
                    shell_id: decode_u64_field(nested, 1)?
                        .unwrap_or_default()
                        .min(u64::from(u32::MAX)) as u32,
                    command: decode_string_field(nested, 2)?.unwrap_or_default(),
                    working_directory: decode_string_field(nested, 3)?.unwrap_or_default(),
                    pid: decode_u64_field(nested, 4)?
                        .map(|value| value.min(u64::from(u32::MAX)) as u32),
                    ms_to_wait: decode_i32_field(nested, 5)?,
                    reason: decode_i32_field(nested, 6)?,
                }),
                _ => Ok(ProductionShellStreamEvent::Other {
                    case: format!("shell_stream_event_field_{field}"),
                }),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(ProductionShellStreamEvent::Other {
        case: "missing_shell_stream_event".into(),
    })
}

fn decode_write_result(
    input: &[u8],
) -> Result<WriteExecResult, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return match field {
                1 => Ok(WriteExecResult::Success),
                5 => Ok(WriteExecResult::Error {
                    error: decode_string_field(nested, 2)?.unwrap_or_default(),
                }),
                6 => Ok(WriteExecResult::Rejected {
                    reason: decode_string_field(nested, 2)?.unwrap_or_default(),
                }),
                3 => Ok(WriteExecResult::Other {
                    case: "permission_denied".into(),
                }),
                4 => Ok(WriteExecResult::Other {
                    case: "no_space".into(),
                }),
                _ => Ok(WriteExecResult::Other {
                    case: format!("write_result_field_{field}"),
                }),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(WriteExecResult::Other {
        case: "missing_write_result".into(),
    })
}

fn decode_exec_client_message(
    input: &[u8],
) -> Result<ProductionExecClientMessage, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return match field {
                2 => Ok(ProductionExecClientMessage::Shell(decode_shell_result(nested)?)),
                3 => Ok(ProductionExecClientMessage::Write(decode_write_result(nested)?)),
                7 => Ok(ProductionExecClientMessage::Read(decode_read_result(nested)?)),
                22 => Ok(ProductionExecClientMessage::ComputerUse(nested.to_vec())),
                _ => Ok(ProductionExecClientMessage::RawResource {
                    field_number: u32::try_from(field).unwrap_or(u32::MAX),
                    protobuf_result: nested.to_vec(),
                }),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(ProductionExecClientMessage::Other)
}

fn decode_exec_control_message(
    input: &[u8],
) -> Result<BoxRemoteExecControlMessage, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 && (1..=3).contains(&field) {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return match field {
                1 => Ok(BoxRemoteExecControlMessage::StreamClose {
                    id: decode_u64_field(nested, 1)?.unwrap_or_default(),
                }),
                2 => Ok(BoxRemoteExecControlMessage::Throw {
                    id: decode_u64_field(nested, 1)?,
                    error: decode_string_field(nested, 2)?
                        .unwrap_or_else(|| "remote ExecService error".into()),
                    stack_trace: decode_string_field(nested, 3)?,
                    error_code: decode_string_field(nested, 4)?,
                }),
                3 => Ok(BoxRemoteExecControlMessage::Heartbeat {
                    id: decode_u64_field(nested, 1)?.unwrap_or_default(),
                }),
                _ => Ok(BoxRemoteExecControlMessage::None),
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(BoxRemoteExecControlMessage::None)
}

fn decode_exec_stream_element(
    input: &[u8],
) -> Result<BoxRemoteExecEnvelope<ProductionExecClientMessage>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if wire == 2 && (field == 1 || field == 2) {
            let nested = decode_len_delimited(input, &mut cursor)?;
            return if field == 1 {
                Ok(BoxRemoteExecEnvelope::ExecClientMessage(
                    decode_exec_client_message(nested)?,
                ))
            } else {
                Ok(BoxRemoteExecEnvelope::ExecClientControlMessage(
                    decode_exec_control_message(nested)?,
                ))
            };
        }
        skip_protobuf_field(input, &mut cursor, wire)?;
    }
    Ok(BoxRemoteExecEnvelope::None)
}

fn encode_connect_envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut envelope = Vec::with_capacity(payload.len() + 5);
    envelope.push(flags);
    envelope.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    envelope.extend_from_slice(payload);
    envelope
}

fn decode_connect_stream(
    body: &[u8],
) -> Result<Vec<BoxRemoteExecEnvelope<ProductionExecClientMessage>>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    let mut messages = Vec::new();
    while cursor < body.len() {
        if body.len().saturating_sub(cursor) < 5 {
            return Err(ProductionBoxTransportError::InvalidHttpResponse(
                "box ExecService returned a truncated Connect envelope".into(),
            ));
        }
        let flags = body[cursor];
        let length = u32::from_be_bytes([
            body[cursor + 1],
            body[cursor + 2],
            body[cursor + 3],
            body[cursor + 4],
        ]) as usize;
        cursor += 5;
        let end = cursor.saturating_add(length);
        if end > body.len() {
            return Err(ProductionBoxTransportError::InvalidHttpResponse(
                "box ExecService returned a truncated Connect payload".into(),
            ));
        }
        let payload = &body[cursor..end];
        cursor = end;
        if flags & CONNECT_STREAM_END_FLAG != 0 {
            if !payload.is_empty() {
                let end_stream: serde_json::Value =
                    serde_json::from_slice(payload).map_err(|error| {
                        ProductionBoxTransportError::InvalidHttpResponse(format!(
                            "box ExecService returned an invalid Connect end-stream envelope: {error}"
                        ))
                    })?;
                if let Some(error) = end_stream.get("error") {
                    return Err(ProductionBoxTransportError::InvalidHttpResponse(format!(
                        "box ExecService returned Connect error: {error}"
                    )));
                }
            }
            continue;
        }
        if flags != 0 {
            return Err(ProductionBoxTransportError::InvalidHttpResponse(format!(
                "box ExecService returned unsupported Connect envelope flags 0x{flags:02x}"
            )));
        }
        messages.push(decode_exec_stream_element(payload)?);
    }
    Ok(messages)
}

fn decode_chunked_http_body(
    input: &[u8],
) -> Result<Vec<u8>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    let mut body = Vec::new();
    loop {
        if cursor >= input.len() {
            return Err(ProductionBoxTransportError::InvalidHttpResponse(
                "box ExecService returned incomplete chunked HTTP framing".into(),
            ));
        }
        let line_end = input[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or_else(|| {
                ProductionBoxTransportError::InvalidHttpResponse(
                    "box ExecService returned malformed chunked HTTP framing".into(),
                )
            })?;
        let size_text = std::str::from_utf8(&input[cursor..line_end]).map_err(|_| {
            ProductionBoxTransportError::InvalidHttpResponse(
                "box ExecService returned a non-UTF-8 chunk size".into(),
            )
        })?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| {
            ProductionBoxTransportError::InvalidHttpResponse(format!(
                "box ExecService returned invalid chunk size {size_text:?}"
            ))
        })?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        let end = cursor.saturating_add(size);
        if end + 2 > input.len() || &input[end..end + 2] != b"\r\n" {
            return Err(ProductionBoxTransportError::InvalidHttpResponse(
                "box ExecService returned a truncated HTTP chunk".into(),
            ));
        }
        body.extend_from_slice(&input[cursor..end]);
        cursor = end + 2;
    }
    Ok(body)
}

fn send_connect_server_stream(
    transport: &ProductionBoxTransport,
    path: &str,
    protobuf_body: &[u8],
) -> Result<Vec<BoxRemoteExecEnvelope<ProductionExecClientMessage>>, ProductionBoxTransportError> {
    let base = Url::parse(&transport.options.base_url)
        .map_err(|error| ProductionBoxTransportError::InvalidBaseUrl(error.to_string()))?;
    if base.scheme() != "http" {
        return Err(ProductionBoxTransportError::InvalidBaseUrl(format!(
            "box transport requires http:// loopback, got {}",
            transport.options.base_url
        )));
    }
    let host = base.host_str().ok_or_else(|| {
        ProductionBoxTransportError::InvalidBaseUrl("box transport base URL has no host".into())
    })?;
    let port = base.port_or_known_default().ok_or_else(|| {
        ProductionBoxTransportError::InvalidBaseUrl("box transport base URL has no port".into())
    })?;

    let timeout = Duration::from_millis(PRODUCTION_BOX_EXEC_TIMEOUT_MS);
    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let body = encode_connect_envelope(0, protobuf_body);
    let mut request = Vec::new();
    write!(
        request,
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: {CONNECT_STREAM_CONTENT_TYPE}\r\nAccept: {CONNECT_STREAM_CONTENT_TYPE}\r\nConnect-Protocol-Version: {CONNECT_PROTOCOL_VERSION}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .map_err(ProductionBoxTransportError::Io)?;
    for (name, value) in transport.request_headers()? {
        write!(request, "{name}: {value}\r\n").map_err(ProductionBoxTransportError::Io)?;
    }
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(&body);
    stream.write_all(&request)?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            ProductionBoxTransportError::InvalidHttpResponse(
                "box ExecService returned an incomplete HTTP response".into(),
            )
        })?;
    let header_text = std::str::from_utf8(&response[..header_end]).map_err(|_| {
        ProductionBoxTransportError::InvalidHttpResponse(
            "box ExecService returned non-UTF-8 HTTP headers".into(),
        )
    })?;
    let status_line = header_text.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| {
            ProductionBoxTransportError::InvalidHttpResponse(format!(
                "box ExecService returned an invalid status line: {status_line}"
            ))
        })?;

    let raw_body = &response[header_end + 4..];
    let is_chunked = header_text.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    });
    let response_body = if is_chunked {
        decode_chunked_http_body(raw_body)?
    } else {
        raw_body.to_vec()
    };
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response_body)
            .chars()
            .take(512)
            .collect::<String>();
        return Err(ProductionBoxTransportError::HttpStatus {
            status,
            body: body_text,
        });
    }
    decode_connect_stream(&response_body)
}

fn validate_header(name: &str, value: &str) -> Result<(), ProductionBoxTransportError> {
    if name.is_empty()
        || name.bytes().any(|byte| byte <= b' ' || byte == b':' || byte >= 0x7f)
        || value.contains('\r')
        || value.contains('\n')
    {
        return Err(ProductionBoxTransportError::InvalidHeader(format!(
            "invalid box transport header {name:?}"
        )));
    }
    Ok(())
}

fn encode_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn encode_len_delimited(field_number: u32, value: &[u8], out: &mut Vec<u8>) {
    encode_key(field_number, 2, out);
    encode_varint(value.len(), out);
    out.extend_from_slice(value);
}

fn encode_string_map_entry(key: &str, value: &str) -> Vec<u8> {
    let mut entry = Vec::new();
    encode_len_delimited(1, key.as_bytes(), &mut entry);
    encode_len_delimited(2, value.as_bytes(), &mut entry);
    entry
}

pub fn encode_update_environment_variables_request(update: &BoxEnvironmentUpdate) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in &update.env {
        let entry = encode_string_map_entry(name, value);
        encode_len_delimited(1, &entry, &mut body);
    }
    if update.replace {
        body.extend_from_slice(&[0x10, 0x01]);
    }
    body
}

pub fn encode_load_mcp_servers_request(request: &BoxMcpLoadRequest) -> Vec<u8> {
    let mut body = Vec::new();
    if !request.mcp_config_json.is_empty() {
        encode_len_delimited(1, request.mcp_config_json.as_bytes(), &mut body);
    }
    if request.remove_missing {
        body.extend_from_slice(&[0x10, 0x01]);
    }
    body
}

fn decode_varint(input: &[u8], cursor: &mut usize) -> Result<u64, ProductionBoxTransportError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while *cursor < input.len() && shift < 64 {
        let byte = input[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(ProductionBoxTransportError::InvalidHttpResponse(
        "box ControlService returned malformed protobuf".into(),
    ))
}

fn skip_protobuf_field(
    input: &[u8],
    cursor: &mut usize,
    wire_type: u8,
) -> Result<(), ProductionBoxTransportError> {
    match wire_type {
        0 => {
            let _ = decode_varint(input, cursor)?;
        }
        1 => {
            *cursor = cursor.saturating_add(8);
        }
        2 => {
            let length = usize::try_from(decode_varint(input, cursor)?).map_err(|_| {
                ProductionBoxTransportError::InvalidHttpResponse(
                    "box ControlService protobuf length overflow".into(),
                )
            })?;
            *cursor = cursor.saturating_add(length);
        }
        5 => {
            *cursor = cursor.saturating_add(4);
        }
        _ => {
            return Err(ProductionBoxTransportError::InvalidHttpResponse(format!(
                "box ControlService returned unsupported protobuf wire type {wire_type}"
            )));
        }
    }
    if *cursor > input.len() {
        return Err(ProductionBoxTransportError::InvalidHttpResponse(
            "box ControlService returned truncated protobuf".into(),
        ));
    }
    Ok(())
}

pub fn decode_load_mcp_servers_response(
    input: &[u8],
) -> Result<Vec<String>, ProductionBoxTransportError> {
    let mut cursor = 0usize;
    let mut names = Vec::new();
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field_number = key >> 3;
        let wire_type = (key & 0x07) as u8;
        if field_number == 1 && wire_type == 2 {
            let length = usize::try_from(decode_varint(input, &mut cursor)?).map_err(|_| {
                ProductionBoxTransportError::InvalidHttpResponse(
                    "box ControlService protobuf length overflow".into(),
                )
            })?;
            let end = cursor.saturating_add(length);
            if end > input.len() {
                return Err(ProductionBoxTransportError::InvalidHttpResponse(
                    "box ControlService returned truncated MCP server name".into(),
                ));
            }
            let name = std::str::from_utf8(&input[cursor..end]).map_err(|_| {
                ProductionBoxTransportError::InvalidHttpResponse(
                    "box ControlService returned non-UTF-8 MCP server name".into(),
                )
            })?;
            names.push(name.to_string());
            cursor = end;
            continue;
        }
        skip_protobuf_field(input, &mut cursor, wire_type)?;
    }
    Ok(names)
}

fn send_connect_unary(
    transport: &ProductionBoxTransport,
    path: &str,
    body: &[u8],
) -> Result<(), ProductionBoxTransportError> {
    send_connect_unary_response(transport, path, body).map(|_| ())
}

fn send_connect_unary_response(
    transport: &ProductionBoxTransport,
    path: &str,
    body: &[u8],
) -> Result<Vec<u8>, ProductionBoxTransportError> {
    let base = Url::parse(&transport.options.base_url)
        .map_err(|error| ProductionBoxTransportError::InvalidBaseUrl(error.to_string()))?;
    if base.scheme() != "http" {
        return Err(ProductionBoxTransportError::InvalidBaseUrl(format!(
            "box transport requires http:// loopback, got {}",
            transport.options.base_url
        )));
    }
    let host = base
        .host_str()
        .ok_or_else(|| ProductionBoxTransportError::InvalidBaseUrl(
            "box transport base URL has no host".into(),
        ))?;
    let port = base
        .port_or_known_default()
        .ok_or_else(|| ProductionBoxTransportError::InvalidBaseUrl(
            "box transport base URL has no port".into(),
        ))?;

    let timeout = Duration::from_millis(PRODUCTION_BOX_RPC_TIMEOUT_MS);
    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let mut request = Vec::new();
    write!(
        request,
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/proto\r\nConnect-Protocol-Version: {CONNECT_PROTOCOL_VERSION}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .map_err(ProductionBoxTransportError::Io)?;
    for (name, value) in transport.request_headers()? {
        write!(request, "{name}: {value}\r\n").map_err(ProductionBoxTransportError::Io)?;
    }
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(body);
    stream.write_all(&request)?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| ProductionBoxTransportError::InvalidHttpResponse(
            "box ControlService returned an incomplete HTTP response".into(),
        ))?;
    let header_text = std::str::from_utf8(&response[..header_end])
        .map_err(|_| ProductionBoxTransportError::InvalidHttpResponse(
            "box ControlService returned non-UTF-8 HTTP headers".into(),
        ))?;
    let status_line = header_text.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| ProductionBoxTransportError::InvalidHttpResponse(format!(
            "box ControlService returned an invalid status line: {status_line}"
        )))?;
    if (200..300).contains(&status) {
        return Ok(response[header_end + 4..].to_vec());
    }
    let body_text = String::from_utf8_lossy(&response[header_end + 4..]);
    let body_text = body_text.chars().take(512).collect::<String>();
    Err(ProductionBoxTransportError::HttpStatus {
        status,
        body: body_text,
    })
}
