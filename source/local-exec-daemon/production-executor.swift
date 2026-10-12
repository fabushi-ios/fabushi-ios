import Foundation

protocol IOSRemoteExecutionProviding: Sendable {
    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload
}

enum IOSRemoteExecutionProviderError: LocalizedError, Equatable, Sendable {
    case unavailable

    var errorDescription: String? {
        "Remote execution provider is not configured for this shipping build."
    }
}

struct IOSUnavailableRemoteExecutionProvider: IOSRemoteExecutionProviding {
    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        throw IOSRemoteExecutionProviderError.unavailable
    }
}

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


struct IOSRemoteRunnerExecutionIdentity: Equatable, Sendable {
    let accountScope: String
    let accountRevision: UInt64
    let hostGeneration: UInt64
}

enum IOSRemoteRunnerFenceError: LocalizedError, Equatable, Sendable {
    case unavailable
    case stale

    var errorDescription: String? {
        switch self {
        case .unavailable:
            return "Remote Runner is unavailable without an active account/Host generation."
        case .stale:
            return "Remote Runner result was discarded after the account or Host generation changed."
        }
    }
}

/// Coordinator-owned transport for the shipping local-exec replacement.
/// Authority stays on MahayanaCoordinator; this object only snapshots and
/// rechecks the settled account/Host generation around one remote dispatch.
struct IOSGenerationFencedRemoteRunnerTransport: @unchecked Sendable, RemoteRunnerTransport {
    typealias IdentityProvider = @MainActor () -> IOSRemoteRunnerExecutionIdentity?
    typealias Dispatch = @MainActor (String, CoordinatorPayload) async throws -> CoordinatorPayload

    private let currentIdentity: IdentityProvider
    private let dispatchImpl: Dispatch

    init(
        currentIdentity: @escaping IdentityProvider,
        dispatch: @escaping Dispatch
    ) {
        self.currentIdentity = currentIdentity
        dispatchImpl = dispatch
    }

    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        guard let token = await currentIdentity() else {
            throw IOSRemoteRunnerFenceError.unavailable
        }
        try Task.checkCancellation()
        let result = try await dispatchImpl(method, params)
        try Task.checkCancellation()
        guard await currentIdentity() == token else {
            throw IOSRemoteRunnerFenceError.stale
        }
        return result
    }
}
