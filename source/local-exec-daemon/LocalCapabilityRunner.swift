import Foundation

/// iOS adaptation of Grok local-exec-daemon. iOS does not spawn arbitrary
/// shell/process daemons. Local work is an allow-listed in-process capability.
actor LocalCapabilityRunner {
    enum Capability: String, Sendable, CaseIterable, Equatable {
        case openExternalURL
        case backgroundTransfer
        case shareItem
        case clipboardRead
        case clipboardWrite
    }

    enum RunnerError: LocalizedError {
        case unsupported(String)
        case permissionDenied(String)

        var errorDescription: String? {
            switch self {
            case .unsupported(let name): "unsupported_local_capability: \(name)"
            case .permissionDenied(let name): "local_capability_permission_denied: \(name)"
            }
        }
    }

    func supports(_ capability: Capability) -> Bool {
        switch capability {
        case .shareItem:
            // Requires an explicit presentation context owned by the UI layer.
            return false
        case .openExternalURL, .backgroundTransfer, .clipboardRead, .clipboardWrite:
            return true
        }
    }

    func rejectDesktopProcessSemantic(_ name: String) throws -> Never {
        throw RunnerError.unsupported(name)
    }
}
