import Foundation

let KNOWN_CONNECTOR_TAGS: Set<String> = [
    "asana","atlassian","buildkite","confluence","context7","databricks","datadog","deepwiki","dock",
    "figma","filesystem","github","gmail","google","googlecalendar","googledocs","googledrive","googlesheets",
    "googleworkspace","huggingface","jira","linear","memory","notion","playwright","salesforce","sentry",
    "sequentialthinking","slack","stripe","telegram","todoist","zoominfo",
]
let OTHER_CONNECTOR_TAG = "other"
let UNKNOWN_CONNECTOR_TAG = "unknown"

func boundedConnectorTag(_ serverName: String?) -> String {
    guard let serverName else { return UNKNOWN_CONNECTOR_TAG }
    let normalized = serverName.lowercased().filter { $0.isLetter || $0.isNumber }
    guard !normalized.isEmpty else { return UNKNOWN_CONNECTOR_TAG }
    return KNOWN_CONNECTOR_TAGS.contains(normalized) ? normalized : OTHER_CONNECTOR_TAG
}

struct ConnectorAuthReport: Equatable, Sendable {
    let phase: String
    let outcome: String
    var serverName: String? = nil
    var serverId: String? = nil
    var reauth: Bool? = nil
    var error: SandErrorValue? = nil
}

struct ConnectorAuthTelemetryRecord: Equatable, Sendable {
    let level: String
    let metadata: [String: String]
}

func connectorAuthTelemetry(_ report: ConnectorAuthReport, surface: String) -> ConnectorAuthTelemetryRecord {
    var metadata = [
        "phase": report.phase,
        "connector": boundedConnectorTag(report.serverName),
        "outcome": report.outcome,
        "surface": surface,
    ]
    if let serverId = brandedId(report.serverId) { metadata["server_id"] = serverId }
    if let reauth = report.reauth { metadata["reauth"] = String(reauth) }
    if let error = report.error {
        metadata.merge(sandErrorTags(error)) { _, incoming in incoming }
    }
    return .init(
        level: report.outcome == "failed" || report.outcome == "timeout" ? "warn" : "info",
        metadata: metadata
    )
}
