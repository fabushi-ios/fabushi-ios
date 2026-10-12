use std::future::Future;

pub trait McpInstalledServer {
    fn id(&self) -> &str;
    fn server_identifier(&self) -> &str;
}

pub fn resolve_mcp_server_rows_by_identifier_or_legacy_id<'a, T>(
    rows: &'a [T],
    token: &str,
) -> Vec<&'a T>
where
    T: McpInstalledServer,
{
    let token = token.trim();
    if token.is_empty() {
        return Vec::new();
    }

    let exact = rows
        .iter()
        .filter(|row| row.server_identifier() == token)
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }

    rows.iter().filter(|row| row.id() == token).collect()
}

pub fn resolve_mcp_server_row_by_identifier_or_legacy_id<'a, T>(
    rows: &'a [T],
    token: &str,
) -> Option<&'a T>
where
    T: McpInstalledServer,
{
    resolve_mcp_server_rows_by_identifier_or_legacy_id(rows, token)
        .into_iter()
        .next()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpInstalledListing<T> {
    Read { servers: Vec<T> },
    Unreadable,
}

pub async fn read_mcp_installed_listing<T, E, Load, Fut>(
    load: Load,
) -> McpInstalledListing<T>
where
    Load: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<T>, E>>,
{
    match load().await {
        Ok(servers) => McpInstalledListing::Read { servers },
        Err(_) => McpInstalledListing::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Server {
        id: String,
        identifier: String,
    }

    impl McpInstalledServer for Server {
        fn id(&self) -> &str {
            &self.id
        }

        fn server_identifier(&self) -> &str {
            &self.identifier
        }
    }

    fn server(id: &str, identifier: &str) -> Server {
        Server {
            id: id.into(),
            identifier: identifier.into(),
        }
    }

    #[test]
    fn identifier_match_wins_over_legacy_id_and_preserves_duplicates() {
        let rows = vec![
            server("legacy-token", "alpha"),
            server("two", "legacy-token"),
            server("three", "legacy-token"),
        ];
        let resolved = resolve_mcp_server_rows_by_identifier_or_legacy_id(&rows, " legacy-token ");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].id, "two");
        assert_eq!(resolved[1].id, "three");
        assert_eq!(
            resolve_mcp_server_row_by_identifier_or_legacy_id(&rows, "alpha")
                .unwrap()
                .id,
            "legacy-token"
        );
    }

    #[test]
    fn legacy_id_is_used_only_when_identifier_has_no_match() {
        let rows = vec![server("legacy-token", "alpha")];
        assert_eq!(
            resolve_mcp_server_row_by_identifier_or_legacy_id(&rows, "legacy-token")
                .unwrap()
                .identifier,
            "alpha"
        );
        assert!(
            resolve_mcp_server_row_by_identifier_or_legacy_id(&rows, "   ").is_none()
        );
    }

    #[tokio::test]
    async fn installed_listing_distinguishes_read_from_unreadable() {
        let read = read_mcp_installed_listing(|| async {
            Ok::<_, ()>(vec![server("one", "alpha")])
        })
        .await;
        assert!(matches!(read, McpInstalledListing::Read { servers } if servers.len() == 1));

        let unreadable =
            read_mcp_installed_listing::<Server, _, _, _>(|| async { Err::<Vec<Server>, _>("boom") })
                .await;
        assert_eq!(unreadable, McpInstalledListing::Unreadable);
    }
}
