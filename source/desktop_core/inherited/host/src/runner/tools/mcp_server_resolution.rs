use std::future::Future;

pub trait McpInstalledServer {
    fn id(&self) -> &str;
    fn server_identifier(&self) -> &str;
}

pub fn resolve_mcp_server_rows_by_identifier_or_legacy_id<'a, T: McpInstalledServer>(
    rows: &'a [T],
    token: &str,
) -> Vec<&'a T> {
    let token = token.trim();
    if token.is_empty() {
        return Vec::new();
    }
    let exact = rows.iter().filter(|row| row.server_identifier() == token).collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    rows.iter().filter(|row| row.id() == token).collect()
}

pub fn resolve_mcp_server_row_by_identifier_or_legacy_id<'a, T: McpInstalledServer>(
    rows: &'a [T],
    token: &str,
) -> Option<&'a T> {
    resolve_mcp_server_rows_by_identifier_or_legacy_id(rows, token).into_iter().next()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpInstalledListing<T> {
    Read { servers: Vec<T> },
    Unreadable,
}

pub async fn read_mcp_installed_listing<T, F, Fut, E>(load: F) -> McpInstalledListing<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<T>, E>>,
{
    match load().await {
        Ok(servers) => McpInstalledListing::Read { servers },
        Err(_) => McpInstalledListing::Unreadable,
    }
}
