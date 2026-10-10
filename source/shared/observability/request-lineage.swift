import Foundation

struct SandRequestLineage: Equatable, Sendable {
    let parentRequestId: String
    let rootParentRequestId: String
    var parentAgentToolCallId: String? = nil
}

func sanitizeHeaderValue(_ value: String) -> String {
    value.replacingOccurrences(of: "\r", with: "").replacingOccurrences(of: "\n", with: "")
}

func buildSandRequestLineageHeaders(_ lineage: SandRequestLineage?) -> [String: String] {
    guard let lineage else { return [:] }
    var headers = [
        "x-parent-request-id": sanitizeHeaderValue(lineage.parentRequestId),
        "x-root-parent-request-id": sanitizeHeaderValue(lineage.rootParentRequestId),
    ]
    if let parentAgentToolCallId = lineage.parentAgentToolCallId {
        headers["x-parent-agent-tool-call-id"] = sanitizeHeaderValue(parentAgentToolCallId)
    }
    return headers
}
