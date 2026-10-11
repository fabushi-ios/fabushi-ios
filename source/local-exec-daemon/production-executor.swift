import Foundation

protocol IOSLocalCapabilityBackend: Sendable {
    func execute(
        capability: LocalCapabilityRunner.Capability,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload
}

/// Production routing for Grok local-exec semantics on iOS.
///
/// Only explicitly allow-listed native capabilities execute on-device.
/// Shell/process/file-daemon semantics are delegated to RemoteRunner and are
/// never emulated with unrestricted native process-spawn APIs on the phone.
actor IOSProductionLocalExecutor {
    private let capabilityRunner: LocalCapabilityRunner
    private let localBackend: any IOSLocalCapabilityBackend
    private let remoteRunner: RemoteRunner

    init(
        capabilityRunner: LocalCapabilityRunner = LocalCapabilityRunner(),
        localBackend: any IOSLocalCapabilityBackend = IOSNativeLocalCapabilityBackend(),
        remoteTransport: any RemoteRunnerTransport
    ) {
        self.capabilityRunner = capabilityRunner
        self.localBackend = localBackend
        remoteRunner = RemoteRunner(transport: remoteTransport)
    }

    func execute(
        method: String,
        params: CoordinatorPayload = .object([:])
    ) async throws -> CoordinatorPayload {
        let normalized = method.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else {
            throw LocalCapabilityRunner.RunnerError.unsupported("empty-method")
        }

        if let capability = Self.localCapability(for: normalized) {
            guard await capabilityRunner.supports(capability) else {
                throw LocalCapabilityRunner.RunnerError.permissionDenied(capability.rawValue)
            }
            return try await localBackend.execute(capability: capability, params: params)
        }

        guard Self.requiresRemoteRunner(normalized) else {
            throw LocalCapabilityRunner.RunnerError.unsupported(normalized)
        }

        let remoteParams = ShellExecEnvironmentFilter.sanitizeRemoteRunnerParams(params)
        return try await remoteRunner.dispatch(
            method: "local-exec.\(normalized)",
            params: remoteParams
        )
    }

    private static func requiresRemoteRunner(_ method: String) -> Bool {
        remoteNamespace(method, root: "shell")
            || remoteNamespace(method, root: "process")
            || remoteNamespace(method, root: "box")
    }

    private static func remoteNamespace(_ method: String, root: String) -> Bool {
        method == root || method.hasPrefix("\(root).")
    }

    private static func localCapability(for method: String) -> LocalCapabilityRunner.Capability? {
        switch method {
        case "openExternalURL": .openExternalURL
        case "backgroundTransfer": .backgroundTransfer
        case "shareItem": .shareItem
        case "clipboardRead": .clipboardRead
        case "clipboardWrite": .clipboardWrite
        default: nil
        }
    }
}


struct IOSRemoteRunnerUnavailableError: LocalizedError, Equatable, Sendable {
    let method: String

    var errorDescription: String? {
        "Remote Runner transport is unavailable for \(method)."
    }
}

/// Shipping fail-closed port used until an authenticated Remote Runner endpoint
/// is actually composed by the Coordinator. This prevents an invented Host or
/// Dashboard RPC from masquerading as production Remote Runner transport.
struct IOSFailClosedRemoteRunnerTransport: RemoteRunnerTransport {
    func dispatch(
        method: String,
        params _: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        throw IOSRemoteRunnerUnavailableError(method: method)
    }
}
