pub const BOX_MCP_UNSUPPORTED_MESSAGE: &str =
    "Fabushi's computer is running an older image without MCP support — update it from Settings → Updates → Update Fabushi's Computer.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectErrorCode {
    Numeric(i32),
    Text(String),
}

pub trait ConnectErrorClassification {
    fn connect_error_code(&self) -> Option<ConnectErrorCode>;
}

pub fn is_unimplemented_connect_error(
    error: &impl ConnectErrorClassification,
) -> bool {
    match error.connect_error_code() {
        Some(ConnectErrorCode::Numeric(12)) => true,
        Some(ConnectErrorCode::Text(value)) => {
            value.eq_ignore_ascii_case("unimplemented")
        }
        _ => false,
    }
}

#[derive(Debug)]
pub enum BoxMcpLoadError<TransportError> {
    Unsupported {
        message: &'static str,
        cause: TransportError,
    },
    Transport(TransportError),
}

pub trait BoxMcpControlClient<Context> {
    type Error: ConnectErrorClassification;

    async fn load_mcp_servers(
        &mut self,
        context: &Context,
        mcp_config_json: &str,
        remove_missing: bool,
    ) -> Result<Vec<String>, Self::Error>;
}

/// Loads MCP configuration only through the supplied Remote Runner transport.
pub async fn load_box_mcp_servers_via_transport<
    Context,
    Transport,
    Client,
    CreateClient,
>(
    context: &Context,
    transport: Transport,
    config_json: &str,
    create_client: CreateClient,
) -> Result<Vec<String>, BoxMcpLoadError<Client::Error>>
where
    Client: BoxMcpControlClient<Context>,
    CreateClient: FnOnce(Transport) -> Client,
{
    let mut client = create_client(transport);
    match client
        .load_mcp_servers(context, config_json, true)
        .await
    {
        Ok(names) => Ok(names),
        Err(error) if is_unimplemented_connect_error(&error) => {
            Err(BoxMcpLoadError::Unsupported {
                message: BOX_MCP_UNSUPPORTED_MESSAGE,
                cause: error,
            })
        }
        Err(error) => Err(BoxMcpLoadError::Transport(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Error(ConnectErrorCode);

    impl ConnectErrorClassification for Error {
        fn connect_error_code(&self) -> Option<ConnectErrorCode> {
            Some(self.0.clone())
        }
    }

    struct Client {
        response: Result<Vec<String>, Error>,
        remove_missing: std::sync::Arc<std::sync::Mutex<Option<bool>>>,
    }

    impl BoxMcpControlClient<()> for Client {
        type Error = Error;

        async fn load_mcp_servers(
            &mut self,
            _context: &(),
            _mcp_config_json: &str,
            remove_missing: bool,
        ) -> Result<Vec<String>, Self::Error> {
            *self.remove_missing.lock().unwrap() = Some(remove_missing);
            self.response.clone()
        }
    }

    #[tokio::test]
    async fn forces_remove_missing_and_returns_loaded_names() {
        let flag = std::sync::Arc::new(std::sync::Mutex::new(None));
        let flag_for_client = flag.clone();
        let loaded = load_box_mcp_servers_via_transport(
            &(),
            "runner",
            "{}",
            |_| Client {
                response: Ok(vec!["github".into()]),
                remove_missing: flag_for_client,
            },
        )
        .await
        .unwrap();
        assert_eq!(loaded, vec!["github"]);
        assert_eq!(*flag.lock().unwrap(), Some(true));
    }

    #[tokio::test]
    async fn translates_connect_unimplemented_to_capability_error() {
        let result = load_box_mcp_servers_via_transport(
            &(),
            "runner",
            "{}",
            |_| Client {
                response: Err(Error(ConnectErrorCode::Numeric(12))),
                remove_missing: Default::default(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(BoxMcpLoadError::Unsupported {
                message: BOX_MCP_UNSUPPORTED_MESSAGE,
                ..
            })
        ));
    }

    #[test]
    fn accepts_string_unimplemented_codes_case_insensitively() {
        assert!(is_unimplemented_connect_error(&Error(
            ConnectErrorCode::Text("UNIMPLEMENTED".into())
        )));
        assert!(!is_unimplemented_connect_error(&Error(
            ConnectErrorCode::Text("unavailable".into())
        )));
    }
}
