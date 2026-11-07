use std::fmt;

pub const BOX_MCP_UNSUPPORTED_MESSAGE: &str =
    "Fabushi's computer is running an older image without MCP support — update it from Settings → Updates → Update Fabushi's Computer.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxMcpUnsupportedError;

impl fmt::Display for SandBoxMcpUnsupportedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(BOX_MCP_UNSUPPORTED_MESSAGE)
    }
}

impl std::error::Error for SandBoxMcpUnsupportedError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectErrorCode {
    Number(i64),
    Text(String),
}

pub trait ConnectErrorCodeSource {
    fn connect_error_code(&self) -> Option<ConnectErrorCode>;
}

pub fn is_unimplemented_connect_code(code: Option<&ConnectErrorCode>) -> bool {
    match code {
        Some(ConnectErrorCode::Number(12)) => true,
        Some(ConnectErrorCode::Text(value)) => value.eq_ignore_ascii_case("unimplemented"),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxMcpLoadRequest {
    pub mcp_config_json: String,
    pub remove_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxMcpLoadResponse {
    pub loaded_server_names: Vec<String>,
}

pub trait BoxMcpControlClient<Ctx> {
    type Error: ConnectErrorCodeSource;

    fn load_mcp_servers(
        &mut self,
        ctx: &Ctx,
        request: BoxMcpLoadRequest,
    ) -> Result<BoxMcpLoadResponse, Self::Error>;
}

#[derive(Debug)]
pub enum BoxMcpLoadError<E> {
    Unsupported {
        error: SandBoxMcpUnsupportedError,
        source: E,
    },
    Transport(E),
}

impl<E: fmt::Display> fmt::Display for BoxMcpLoadError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { error, .. } => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for BoxMcpLoadError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsupported { source, .. } | Self::Transport(source) => Some(source),
        }
    }
}

pub fn load_box_mcp_servers_via_transport<Ctx, Transport, Client, Factory>(
    ctx: &Ctx,
    transport: &Transport,
    config_json: &str,
    create_client: Factory,
) -> Result<Vec<String>, BoxMcpLoadError<Client::Error>>
where
    Client: BoxMcpControlClient<Ctx>,
    Factory: FnOnce(&Transport) -> Client,
{
    let mut control = create_client(transport);
    match control.load_mcp_servers(
        ctx,
        BoxMcpLoadRequest {
            mcp_config_json: config_json.to_string(),
            remove_missing: true,
        },
    ) {
        Ok(response) => Ok(response.loaded_server_names),
        Err(source) if is_unimplemented_connect_code(source.connect_error_code().as_ref()) => {
            Err(BoxMcpLoadError::Unsupported {
                error: SandBoxMcpUnsupportedError,
                source,
            })
        }
        Err(source) => Err(BoxMcpLoadError::Transport(source)),
    }
}
