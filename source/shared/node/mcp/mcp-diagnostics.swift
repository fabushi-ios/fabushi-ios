import Foundation

struct McpDiagnostic: Equatable, Sendable {
    let leg: String
    let errorClass: String
}

let MCP_ERROR_RESULT_CLASS = "mcp_error_result"
let EXEC_ERROR_CLASS_CAP = 128
private let MCP_PRE_PIN_BUFFER_CAP = 32
private let MCP_CONNECT_CODE_NAMES: [Int: String] = [
    1:"Canceled",2:"Unknown",3:"InvalidArgument",4:"DeadlineExceeded",5:"NotFound",
    6:"AlreadyExists",7:"PermissionDenied",8:"ResourceExhausted",9:"FailedPrecondition",
    10:"Aborted",11:"OutOfRange",12:"Unimplemented",13:"Internal",14:"Unavailable",
    15:"DataLoss",16:"Unauthenticated",
]

private final class McpDiagnosticsState: @unchecked Sendable {
    let lock = NSLock()
    var reporter: ((McpDiagnostic) -> Void)?
    var buffered: [McpDiagnostic] = []
    var execErrorClassByToolCallId: [String: String] = [:]
    var execErrorOrder: [String] = []
}
private let MCP_DIAGNOSTICS_STATE = McpDiagnosticsState()

func pinMcpDiagnosticsReporter(_ reporter: ((McpDiagnostic) -> Void)?) {
    let backlog: [McpDiagnostic]
    MCP_DIAGNOSTICS_STATE.lock.lock()
    MCP_DIAGNOSTICS_STATE.reporter = reporter
    backlog = MCP_DIAGNOSTICS_STATE.buffered
    MCP_DIAGNOSTICS_STATE.buffered.removeAll()
    MCP_DIAGNOSTICS_STATE.lock.unlock()
    guard let reporter else { return }
    for diagnostic in backlog { reporter(diagnostic) }
}

func mcpErrorClassOf(_ error: Any) -> String {
    if let connect = error as? any ConnectErrorLike {
        let codeName = MCP_CONNECT_CODE_NAMES[connect.connectCode] ?? "undefined"
        return "ConnectError.\(codeName)"
    }
    if let error = error as? Error {
        let name = String(describing: type(of: error))
        return name.isEmpty ? "Error" : name
    }
    return String(describing: type(of: error))
}

private func emitMcpDiagnostic(_ diagnostic: McpDiagnostic) {
    let reporter: ((McpDiagnostic) -> Void)?
    MCP_DIAGNOSTICS_STATE.lock.lock()
    reporter = MCP_DIAGNOSTICS_STATE.reporter
    if reporter == nil, MCP_DIAGNOSTICS_STATE.buffered.count < MCP_PRE_PIN_BUFFER_CAP {
        MCP_DIAGNOSTICS_STATE.buffered.append(diagnostic)
    }
    MCP_DIAGNOSTICS_STATE.lock.unlock()
    reporter?(diagnostic)
}

func reportMcpHostEdgeFailure(_ leg: String, error: Any) {
    emitMcpDiagnostic(.init(leg: leg, errorClass: mcpErrorClassOf(error)))
}

func reportMcpHostEdgeDegraded(_ leg: String, errorClass: String) {
    emitMcpDiagnostic(.init(leg: leg, errorClass: errorClass))
}

func recordMcpExecErrorClass(_ toolCallId: String, error: Any) {
    guard !toolCallId.isEmpty else { return }
    MCP_DIAGNOSTICS_STATE.lock.lock()
    if MCP_DIAGNOSTICS_STATE.execErrorClassByToolCallId[toolCallId] == nil {
        if MCP_DIAGNOSTICS_STATE.execErrorClassByToolCallId.count >= EXEC_ERROR_CLASS_CAP,
           let oldest = MCP_DIAGNOSTICS_STATE.execErrorOrder.first {
            MCP_DIAGNOSTICS_STATE.execErrorOrder.removeFirst()
            MCP_DIAGNOSTICS_STATE.execErrorClassByToolCallId.removeValue(forKey: oldest)
        }
        MCP_DIAGNOSTICS_STATE.execErrorOrder.append(toolCallId)
    }
    MCP_DIAGNOSTICS_STATE.execErrorClassByToolCallId[toolCallId] = mcpErrorClassOf(error)
    MCP_DIAGNOSTICS_STATE.lock.unlock()
}

func takeMcpExecErrorClass(_ toolCallId: String) -> String {
    MCP_DIAGNOSTICS_STATE.lock.lock()
    defer { MCP_DIAGNOSTICS_STATE.lock.unlock() }
    guard let recorded = MCP_DIAGNOSTICS_STATE.execErrorClassByToolCallId.removeValue(forKey: toolCallId) else {
        return MCP_ERROR_RESULT_CLASS
    }
    MCP_DIAGNOSTICS_STATE.execErrorOrder.removeAll { $0 == toolCallId }
    return recorded
}
