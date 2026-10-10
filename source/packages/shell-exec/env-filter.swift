import Foundation

/// iOS adaptation of Grok's shell environment scrubbing.
///
/// iOS never spawns a local shell process. These helpers preserve the recovered
/// environment-sanitization contract at the production Remote Runner boundary
/// so desktop-only Electron and local socket variables are not forwarded.
enum ShellExecEnvironmentFilter {
    static let socketEnvironmentVariablesToScrub: Set<String> = [
        "SSH_AUTH_SOCK",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
        "WAYLAND_DISPLAY",
    ]

    static func filterElectronEnvironment(
        _ environment: [String: CoordinatorPayload]
    ) -> [String: CoordinatorPayload] {
        var filtered = environment
        filtered.removeValue(forKey: "ELECTRON_RUN_AS_NODE")
        return filtered
    }

    static func scrubSocketEnvironmentVariables(
        _ environment: [String: CoordinatorPayload]
    ) -> [String: CoordinatorPayload] {
        var filtered = environment
        for key in socketEnvironmentVariablesToScrub {
            filtered.removeValue(forKey: key)
        }
        return filtered
    }

    static func sanitizeRemoteRunnerParams(
        _ params: CoordinatorPayload
    ) -> CoordinatorPayload {
        guard case .object(var object) = params else {
            return params
        }

        for environmentKey in ["env", "environment"] {
            guard case .object(let environment)? = object[environmentKey] else {
                continue
            }
            object[environmentKey] = .object(
                scrubSocketEnvironmentVariables(
                    filterElectronEnvironment(environment)
                )
            )
        }
        return .object(object)
    }
}
