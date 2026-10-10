import Foundation

let DEFAULT_MCP_ACCOUNT_KEY = "default"
let MAX_RENDERED_MCP_ACCOUNT_LABEL_LENGTH = 64
let MAX_CONNECTOR_ERROR_LENGTH = 300
let MAX_UNTRUSTED_MARKUP_SCAN_LENGTH = 16_384

func normalizeMcpAccountLabel(_ rawLabel: String) -> String {
    rawLabel.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
}

func provisionalMcpAccountServerIdentifier(_ rowIdentifier: String, accountKey: String) -> String {
    accountKey == DEFAULT_MCP_ACCOUNT_KEY ? rowIdentifier : "\(rowIdentifier)--\(accountKey)"
}

private func isMcpHostileScalar(_ scalar: UnicodeScalar) -> Bool {
    let value = scalar.value
    if value <= 0x1f || value == 0x7f || value == 0x2028 || value == 0x2029 { return true }
    return [0x22,0x27,0x60,0x5c,0x5b,0x5d,0x7b,0x7d,0x28,0x29,0x3c,0x3e].contains(value)
}

func encodeMcpAccountLabelForListing(_ label: String) -> String {
    var escaped = ""
    for scalar in label.unicodeScalars {
        if isMcpHostileScalar(scalar) {
            escaped += String(format: "\\u%04x", scalar.value)
        } else {
            escaped.unicodeScalars.append(scalar)
        }
    }
    return "\"\(escaped)\""
}

func decodeMcpAccountLabelArgument(_ rawArgument: String) -> String {
    let value = rawArgument.trimmingCharacters(in: .whitespacesAndNewlines)
    guard value.count >= 2, value.first == "\"", value.last == "\"",
          let data = value.data(using: .utf8),
          let decoded = try? JSONDecoder().decode(String.self, from: data) else {
        return rawArgument
    }
    return decoded
}

func formatMcpAccountLabelForPrompt(_ rawLabel: String) -> String {
    var sanitized = ""
    for scalar in rawLabel.unicodeScalars {
        let value = scalar.value
        if [0x22,0x27,0x60,0x5c,0x5b,0x5d,0x7b,0x7d,0x28,0x29,0x3c,0x3e].contains(value) {
            continue
        }
        sanitized.unicodeScalars.append(scalar)
    }
    let inert = sanitized.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    return String(inert.prefix(MAX_RENDERED_MCP_ACCOUNT_LABEL_LENGTH))
}

func formatMcpAccountDisplayName(_ name: String, accountKey: String?) -> String {
    guard let accountKey, accountKey != DEFAULT_MCP_ACCOUNT_KEY else { return name }
    return "\(name) (\(formatMcpAccountLabelForPrompt(accountKey)))"
}

func isEffectivePluginInstalled(isEnabled: Bool) -> Bool { isEnabled }

func uninstallClearedInstallRecord(removed: Bool, reason: String? = nil) -> Bool {
    removed || reason == "team-server"
}

private func replacingRegex(_ input: String, pattern: String, with replacement: String) -> String {
    guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive, .dotMatchesLineSeparators]) else {
        return input
    }
    let range = NSRange(input.startIndex..<input.endIndex, in: input)
    return regex.stringByReplacingMatches(in: input, options: [], range: range, withTemplate: replacement)
}

func stripMarkupAndBoundConnectorError(_ raw: String) -> String {
    let boundedScan = String(raw.prefix(MAX_UNTRUSTED_MARKUP_SCAN_LENGTH))
    let noActiveMarkup = replacingRegex(
        boundedScan,
        pattern: #"<(script|style)\b[^<>]*>[\s\S]*?(?:</\1>|$)"#,
        with: " "
    )
    let noTags = replacingRegex(noActiveMarkup, pattern: #"<[^<>]*>"#, with: " ")
    let collapsed = noTags
        .split(whereSeparator: { $0.isWhitespace })
        .joined(separator: " ")
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard collapsed.count > MAX_CONNECTOR_ERROR_LENGTH else { return collapsed }
    return String(collapsed.prefix(MAX_CONNECTOR_ERROR_LENGTH - 1))
        .trimmingCharacters(in: .whitespacesAndNewlines) + "…"
}
