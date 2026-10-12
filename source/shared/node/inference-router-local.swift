import Foundation

/// Grok's desktop implementation reports whether local Codex/Claude CLI
/// processes and their credential files can be used. iOS intentionally does
/// not discover or spawn arbitrary local executables. The same capability
/// decision is represented explicitly so callers can route work to the
/// Coordinator's native provider or Remote Runner instead of simulating a
/// desktop process boundary.
struct LocalInferenceCliStatus: Equatable, Sendable {
    let installed: Bool
    let authenticated: Bool
    let executablePath: String?
    let route: IOSInferenceExecutionRoute
}

enum IOSInferenceExecutionRoute: String, Equatable, Sendable {
    case nativeProvider = "native-provider"
    case remoteRunner = "remote-runner"
}

struct LocalInferenceCliStatuses: Equatable, Sendable {
    let codex: LocalInferenceCliStatus
    let claudeCode: LocalInferenceCliStatus
}

func resolveCodexCliPath() -> String? {
    nil
}

func resolveClaudeCodeCliPath() -> String? {
    nil
}

func getLocalInferenceCliStatus() -> LocalInferenceCliStatuses {
    .init(
        codex: .init(
            installed: false,
            authenticated: false,
            executablePath: nil,
            route: .nativeProvider
        ),
        claudeCode: .init(
            installed: false,
            authenticated: false,
            executablePath: nil,
            route: .remoteRunner
        )
    )
}
