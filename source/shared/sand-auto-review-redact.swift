import Foundation

let SAND_AUTO_REVIEW_SECRET_VALUE_PATTERN = #"(?i)^(?:bearer\s+|sk[-_]|gh[pousr]_|xox[baprs]-|AIza)[^\s]*$"#
let SAND_AUTO_REVIEW_SECRET_KEY_PATTERN = #"(?i)(?:auth|credential|key|password|secret|signature|token)"#

private func redactRegex(_ input: String, pattern: String, replacement: String) -> String {
    guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]) else { return input }
    let range = NSRange(input.startIndex..<input.endIndex, in: input)
    return regex.stringByReplacingMatches(in: input, options: [], range: range, withTemplate: replacement)
}

private func redactAutoReviewURLs(_ value: String) -> String {
    guard let regex = try? NSRegularExpression(pattern: #"https?://[^\s\"'`]+"#, options: [.caseInsensitive]) else { return value }
    let ns = value as NSString
    let matches = regex.matches(in: value, range: NSRange(location: 0, length: ns.length)).reversed()
    let mutable = NSMutableString(string: value)
    for match in matches {
        let raw = ns.substring(with: match.range)
        guard var components = URLComponents(string: raw) else { continue }
        components.user = nil
        components.password = nil
        components.query = nil
        components.fragment = nil
        if let sanitized = components.string {
            mutable.replaceCharacters(in: match.range, with: sanitized)
        }
    }
    return mutable as String
}

func redactSandAutoReviewInlineSecrets(_ value: String) -> String {
    var result = redactAutoReviewURLs(value)
    result = redactRegex(
        result,
        pattern: #"((?:--)?(?:api[_-]?key|authorization|credential|password|secret|signature|token)\s*(?:=|:|\s)\s*)(?:\"[^\"]*\"|'[^']*'|Bearer\s+[^\s]+|[^\s]+)"#,
        replacement: "$1…"
    )
    result = redactRegex(result, pattern: #"\bBearer\s+[^\s\"']+"#, replacement: "Bearer …")
    result = redactRegex(
        result,
        pattern: #"\b(?:sk[-_]|gh[pousr]_|xox[baprs]-|AIza)[A-Za-z0-9+/_=-]+"#,
        replacement: "…"
    )
    return result
}
