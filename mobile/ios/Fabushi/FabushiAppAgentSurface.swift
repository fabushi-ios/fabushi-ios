import Foundation

/// Native App MCP semantic surface for the iOS shell.
///
/// It exposes the same stable `fabushi.app.*` contract as Web and Electron,
/// but never exposes arbitrary Swift invocation, JavaScript, shell access, or
/// sensitive field values. A native device transport can publish this object
/// without changing the UI contract.
@MainActor
final class FabushiAppAgentSurface {
    static let version = 1
    static let maximumElementCount = 500
    static let truncationAgentId = "fabushi.surface.truncated"
    static let toolNames = [
        "fabushi.app.status",
        "fabushi.app.snapshot",
        "fabushi.app.find",
        "fabushi.app.action",
        "fabushi.app.wait",
        "fabushi.app.assert",
    ]

    struct Element: Equatable {
        let agentId: String
        let role: String
        let name: String
        var visible = true
        var enabled = true
        var sensitive = false
        var valuePresent: Bool? = nil
        var valueLength: Int? = nil
    }

    struct Snapshot: Equatable {
        let version: Int
        let appId: String
        let platform: String
        let screen: String
        let generation: UInt64
        let elements: [Element]
    }

    struct Status: Equatable {
        let version: Int
        let appId: String
        let platform: String
        let available: Bool
        let screen: String
        let generation: UInt64
    }

    struct Assertion: Equatable {
        let passed: Bool
        let screen: String
        let generation: UInt64
        let matches: [Element]
        let failures: [String]
    }

    struct Action {
        let allowed: Set<String>
        let invoke: @MainActor (String?) -> Void
    }

    enum SurfaceError: LocalizedError, Equatable {
        case invalidScreen
        case elementLimit
        case invalidElement
        case duplicateAgentId
        case actionTargetMissing
        case staleGeneration
        case elementNotFound
        case targetHidden
        case targetDisabled
        case sensitiveInputRequiresSecureInput
        case valueTooLarge
        case actionUnavailable
        case unsupportedAction

        var errorDescription: String? {
            switch self {
            case .invalidScreen: "invalid_app_surface_screen"
            case .elementLimit: "app_surface_element_limit"
            case .invalidElement: "invalid_app_surface_element"
            case .duplicateAgentId: "duplicate_app_surface_agent_id"
            case .actionTargetMissing: "app_surface_action_target_missing"
            case .staleGeneration: "stale_app_surface_generation"
            case .elementNotFound: "app_surface_element_not_found"
            case .targetHidden: "app_surface_target_hidden"
            case .targetDisabled: "app_surface_target_disabled"
            case .sensitiveInputRequiresSecureInput: "sensitive_app_surface_input_requires_secure_input"
            case .valueTooLarge: "app_surface_value_too_large"
            case .actionUnavailable: "app_surface_action_unavailable"
            case .unsupportedAction: "unsupported_app_surface_action"
            }
        }
    }

    private let appId: String
    private var generation: UInt64 = 0
    private var screen = "unavailable"
    private var elements: [Element] = []
    private var actions: [String: Action] = [:]

    init(appId: String = "fabushi.ios") {
        self.appId = appId
    }

    @discardableResult
    func publish(
        screen: String,
        elements: [Element],
        actions: [String: Action] = [:]
    ) throws -> Snapshot {
        guard !screen.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              screen.count <= 160
        else { throw SurfaceError.invalidScreen }

        var identifiers = Set<String>()
        for element in elements {
            guard Self.validAgentId(element.agentId),
                  element.agentId != Self.truncationAgentId,
                  element.role.count <= 80,
                  element.name.count <= 240
            else { throw SurfaceError.invalidElement }
            guard identifiers.insert(element.agentId).inserted else { throw SurfaceError.duplicateAgentId }
        }
        guard actions.keys.allSatisfy(identifiers.contains) else { throw SurfaceError.actionTargetMissing }

        let retainedElements: [Element]
        if elements.count > Self.maximumElementCount {
            var bounded = Array(elements.prefix(Self.maximumElementCount - 1))
            bounded.append(.init(
                agentId: Self.truncationAgentId,
                role: "status",
                name: "Additional semantic elements omitted; refine the current view or navigate deeper.",
                visible: true,
                enabled: false
            ))
            retainedElements = bounded
        } else {
            retainedElements = elements
        }
        let retainedIds = Set(retainedElements.map(\.agentId))
        let retainedActions = actions.filter { retainedIds.contains($0.key) }

        generation = generation == UInt64.max ? 1 : generation + 1
        self.screen = screen
        self.elements = retainedElements
        self.actions = retainedActions
        return snapshot()
    }

    func clear() {
        generation = generation == UInt64.max ? 1 : generation + 1
        screen = "unavailable"
        elements = []
        actions = [:]
    }

    func status() -> Status {
        Status(
            version: Self.version,
            appId: appId,
            platform: "ios",
            available: screen != "unavailable",
            screen: screen,
            generation: generation
        )
    }

    func snapshot() -> Snapshot {
        Snapshot(
            version: Self.version,
            appId: appId,
            platform: "ios",
            screen: screen,
            generation: generation,
            elements: elements
        )
    }

    func find(
        agentId: String? = nil,
        role: String? = nil,
        name: String? = nil,
        limit: Int = 25
    ) -> [Element] {
        elements.lazy
            .filter { element in
                guard let agentId, !agentId.isEmpty else { return true }
                return element.agentId == agentId
            }
            .filter { element in
                guard let role, !role.isEmpty else { return true }
                return element.role.caseInsensitiveCompare(role) == .orderedSame
            }
            .filter { element in
                guard let name, !name.isEmpty else { return true }
                return element.name.localizedCaseInsensitiveContains(name)
            }
            .prefix(max(1, min(100, limit)))
            .map { $0 }
    }

    @discardableResult
    func perform(
        expectedGeneration: UInt64,
        agentId: String,
        action: String,
        value: String? = nil
    ) throws -> Snapshot {
        guard expectedGeneration == generation else { throw SurfaceError.staleGeneration }
        guard let element = elements.first(where: { $0.agentId == agentId }) else { throw SurfaceError.elementNotFound }
        guard element.visible else { throw SurfaceError.targetHidden }
        guard element.enabled else { throw SurfaceError.targetDisabled }
        guard !(element.sensitive && value != nil) else { throw SurfaceError.sensitiveInputRequiresSecureInput }
        guard value?.count ?? 0 <= 20_000 else { throw SurfaceError.valueTooLarge }
        guard let binding = actions[agentId] else { throw SurfaceError.actionUnavailable }
        guard binding.allowed.contains(action) else { throw SurfaceError.unsupportedAction }
        binding.invoke(value)
        generation = generation == UInt64.max ? 1 : generation + 1
        return snapshot()
    }

    func assertState(
        screen expectedScreen: String? = nil,
        agentId: String? = nil,
        role: String? = nil,
        name: String? = nil,
        state: String = "present"
    ) -> Assertion {
        let matches = find(agentId: agentId, role: role, name: name, limit: 100)
        var failures: [String] = []
        if let expectedScreen, !expectedScreen.isEmpty, screen != expectedScreen {
            failures.append("screen expected \(expectedScreen), actual \(screen)")
        }
        let statePassed: Bool
        switch state {
        case "absent": statePassed = matches.isEmpty
        case "enabled": statePassed = matches.contains(where: \.enabled)
        case "disabled": statePassed = matches.contains(where: { !$0.enabled })
        case "visible": statePassed = matches.contains(where: \.visible)
        case "hidden": statePassed = matches.contains(where: { !$0.visible })
        default: statePassed = agentId == nil && role == nil && name == nil ? true : !matches.isEmpty
        }
        if !statePassed { failures.append("element state \(state) was not satisfied") }
        return Assertion(
            passed: failures.isEmpty,
            screen: screen,
            generation: generation,
            matches: matches,
            failures: failures
        )
    }

    func waitFor(
        screen: String? = nil,
        agentId: String? = nil,
        role: String? = nil,
        name: String? = nil,
        state: String = "present",
        timeoutMilliseconds: UInt64 = 10_000
    ) async -> Assertion {
        let bounded = max(100, min(30_000, timeoutMilliseconds))
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .milliseconds(Int64(bounded)))
        while true {
            let result = assertState(screen: screen, agentId: agentId, role: role, name: name, state: state)
            if result.passed || clock.now >= deadline { return result }
            do { try await Task.sleep(for: .milliseconds(100)) }
            catch { return result }
        }
    }

    private static func validAgentId(_ value: String) -> Bool {
        guard !value.isEmpty, value.count <= 200 else { return false }
        return value.range(of: #"^[A-Za-z0-9._:/@-]+$"#, options: .regularExpression) != nil
    }
}
