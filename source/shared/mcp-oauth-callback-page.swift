import Foundation

private func escapeMcpOAuthHTML(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&#39;")
}

private func renderMcpOAuthCallbackPage(
    title: String,
    message: String,
    hint: String
) -> String {
    [
        "<!doctype html>",
        "<html lang=\"en\">",
        "<head>",
        "<meta charset=\"utf-8\">",
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
        "<title>\(escapeMcpOAuthHTML(title))</title>",
        "<style>",
        "html,body{margin:0;min-height:100%;}",
        "body{font-family:ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;background:#0c0c0d;color:#f4f4f5;}",
        ".page{display:flex;min-height:100vh;flex:1;align-items:center;justify-content:center;}",
        ".content{text-align:center;}",
        ".message{font-size:1.125rem;line-height:1.75rem;margin:0;font-weight:400;}",
        ".hint{margin:0.5rem 0 0;font-size:1rem;line-height:1.5rem;color:#a1a1aa;}",
        "</style>",
        "</head>",
        "<body>",
        "<main class=\"page\">",
        "<div class=\"content\">",
        "<p class=\"message\">\(escapeMcpOAuthHTML(message))</p>",
        "<p class=\"hint\">\(escapeMcpOAuthHTML(hint))</p>",
        "</div>",
        "</main>",
        "</body>",
        "</html>",
    ].joined()
}

func renderMcpOAuthSuccessPage(serverName: String? = nil) -> String {
    let name = serverName?.trimmingCharacters(in: .whitespacesAndNewlines)
    let title = (name?.isEmpty == false) ? "\(name!) connected" : "Authentication complete"
    return renderMcpOAuthCallbackPage(
        title: title,
        message: "Authorization complete!",
        hint: "You can close this tab."
    )
}

func renderMcpOAuthErrorPage(serverName: String? = nil) -> String {
    let name = serverName?.trimmingCharacters(in: .whitespacesAndNewlines)
    let title = (name?.isEmpty == false) ? "\(name!) — Authentication failed" : "Authentication failed"
    return renderMcpOAuthCallbackPage(
        title: title,
        message: "OAuth callback failed.",
        hint: "Close this tab and try connecting again."
    )
}
