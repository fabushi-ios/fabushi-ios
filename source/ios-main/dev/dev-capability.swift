import Foundation

enum IOSDevPreloadKind: Equatable, Sendable {
    case primary
    case devControls
}

/// Native iOS counterpart of Grok's dev-capability preload selection.
///
/// iOS never swaps an Electron preload. Instead, the capability decides whether
/// the debug-only native developer-control bridge is composed at all.
struct IOSDevCapability: Equatable, Sendable {
    static let environmentKey = "FABUSHI_DEV_CAPABILITY"

    let enabled: Bool

    static func resolve(
        isDebugBuild: Bool,
        environment: [String: String]
    ) -> IOSDevCapability {
        IOSDevCapability(
            enabled: isDebugBuild && environment[environmentKey] == "1"
        )
    }

    static func live(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> IOSDevCapability {
        #if DEBUG
        return resolve(isDebugBuild: true, environment: environment)
        #else
        return resolve(isDebugBuild: false, environment: environment)
        #endif
    }

    var preloadKind: IOSDevPreloadKind {
        enabled ? .devControls : .primary
    }
}
