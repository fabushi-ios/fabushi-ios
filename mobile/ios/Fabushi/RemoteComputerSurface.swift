import Foundation
import SwiftUI
import UIKit
import WebKit

private let remoteComputerOriginHost = "fabushi.ombhrum.com"
private let remoteComputerURL = URL(string: "https://fabushi.ombhrum.com/remote-computer")!
private let remoteComputerForeverBoxID = "forever-box"

struct RemoteComputerScope: Equatable, Sendable {
    let accountScopeKey: String
    let agentID: String?
    let agentName: String?

    var scopeKey: String {
        [accountScopeKey, agentID ?? "account"].joined(separator: ":")
    }

    var displayTitle: String {
        guard let agentName, !agentName.isEmpty else { return "我的电脑" }
        return "\(agentName) 的电脑"
    }
}

enum RemoteComputerRebuildKind: String, Equatable, Sendable {
    case update
    case reset
    case recover
    case reconnecting

    var isResetLike: Bool {
        self == .reset || self == .recover
    }
}

enum RemoteComputerRebuildUpdateSource: String, Equatable, Sendable {
    case auto
    case request
    case migration
}

enum RemoteComputerRebuildTeardown: String, Equatable, Sendable {
    case none
    case transport
    case box
}

enum RemoteComputerRebuildResolution: String, Equatable, Sendable {
    case settled
    case failed
    case cancelled
}

struct RemoteComputerRebuildOperationID: Equatable, Sendable {
    let value: String
}

enum RemoteComputerMigrationPhase: String, CaseIterable, Equatable, Sendable {
    case backingUp = "backing-up"
    case creating
    case moving
    case cleaningUp = "cleaning-up"
    case wiping
    case done
    case failed

    var isTerminal: Bool {
        self == .done || self == .failed
    }
}

struct RemoteComputerRebuildState: Equatable, Sendable {
    var kind: RemoteComputerRebuildKind?
    var operationID: RemoteComputerRebuildOperationID?
    var updateSource: RemoteComputerRebuildUpdateSource?
    var lockBoxID: String?
    var isPending: Bool
    var hasRequestAcknowledgement: Bool
    var imageUpdateAvailable: Bool?
    var expectsImageUpgrade: Bool
    var boxPhase: String?
    var observedBoxID: String?
    var lastHealthyBoxID: String?
    var hasLeftHealthy: Bool
    var teardownObserved: RemoteComputerRebuildTeardown
    var reconnectedSinceLeft: Bool
    var readySince: Int64?
    var isConnected: Bool
    var connectedSince: Int64?
    var resetOperationID: RemoteComputerRebuildOperationID?
    var hasTerminalMigration: Bool
    var lastResolution: RemoteComputerRebuildResolution?
    var lastResolutionKind: RemoteComputerRebuildKind?

    static func initial(
        boxPhase: String? = nil,
        imageUpdateAvailable: Bool? = nil
    ) -> Self {
        .init(
            kind: nil,
            operationID: nil,
            updateSource: nil,
            lockBoxID: nil,
            isPending: false,
            hasRequestAcknowledgement: false,
            imageUpdateAvailable: imageUpdateAvailable,
            expectsImageUpgrade: false,
            boxPhase: boxPhase,
            observedBoxID: nil,
            lastHealthyBoxID: nil,
            hasLeftHealthy: false,
            teardownObserved: .none,
            reconnectedSinceLeft: false,
            readySince: nil,
            isConnected: true,
            connectedSince: nil,
            resetOperationID: nil,
            hasTerminalMigration: false,
            lastResolution: nil,
            lastResolutionKind: nil
        )
    }
}

enum RemoteComputerRebuildEvent: Equatable, Sendable {
    case request(
        kind: RemoteComputerRebuildKind,
        operationID: RemoteComputerRebuildOperationID?,
        source: RemoteComputerRebuildUpdateSource?,
        at: Int64
    )
    case pending(Bool, at: Int64)
    case acknowledged(at: Int64)
    case imageUpdate(Bool?, at: Int64)
    case box(boxID: String?, phase: String?, at: Int64)
    case migration(
        operationID: RemoteComputerRebuildOperationID?,
        phase: RemoteComputerMigrationPhase,
        at: Int64
    )
    case connection(isConnected: Bool, at: Int64)
    case error(at: Int64)
    case deactivate(at: Int64)
    case tick(at: Int64)
}

enum RemoteComputerRebuildReducer {
    private static func healthy(_ phase: String?) -> Bool {
        phase == "running" || phase == "local"
    }

    private static func sameOperation(
        _ left: RemoteComputerRebuildOperationID?,
        _ right: RemoteComputerRebuildOperationID?
    ) -> Bool {
        guard let left, let right else { return false }
        return left.value == right.value
    }

    private static func sameLockedBox(_ state: RemoteComputerRebuildState) -> Bool {
        state.lockBoxID == nil
            || state.observedBoxID == nil
            || state.lockBoxID == state.observedBoxID
    }

    private static func clearOperation(
        _ state: RemoteComputerRebuildState,
        resolution: RemoteComputerRebuildResolution?
    ) -> RemoteComputerRebuildState {
        var next = state
        let previousKind = state.kind
        next.kind = nil
        next.operationID = nil
        next.updateSource = nil
        next.lockBoxID = nil
        next.hasLeftHealthy = false
        next.teardownObserved = .none
        next.reconnectedSinceLeft = false
        next.readySince = nil
        next.resetOperationID = nil
        next.hasTerminalMigration = false
        next.hasRequestAcknowledgement = false
        next.expectsImageUpgrade = false
        next.lastResolution = resolution
        next.lastResolutionKind = previousKind
        return next
    }

    private static func begin(
        _ state: RemoteComputerRebuildState,
        kind: RemoteComputerRebuildKind,
        operationID: RemoteComputerRebuildOperationID?,
        source: RemoteComputerRebuildUpdateSource?,
        at: Int64
    ) -> RemoteComputerRebuildState {
        let scopedOperationID = kind.isResetLike ? operationID : nil
        let updateSource = kind == .update ? source : nil

        if state.kind != nil {
            if kind.isResetLike
                && (
                    state.kind?.isResetLike != true
                    || (
                        operationID != nil
                        && !sameOperation(state.resetOperationID, operationID)
                    )
                )
            {
                var next = state
                next.kind = kind
                next.operationID = scopedOperationID
                next.resetOperationID = scopedOperationID
                next.updateSource = nil
                next.lockBoxID = state.observedBoxID
                next.hasLeftHealthy = true
                next.reconnectedSinceLeft = false
                next.readySince = nil
                next.hasTerminalMigration = false
                next.hasRequestAcknowledgement = false
                next.expectsImageUpgrade = false
                next.lastResolution = nil
                next.lastResolutionKind = nil
                return next
            }

            if state.kind == .reconnecting && kind != .reconnecting {
                var next = state
                next.kind = kind
                next.operationID = scopedOperationID
                next.resetOperationID = scopedOperationID
                next.updateSource = updateSource
                next.lockBoxID = state.observedBoxID
                next.expectsImageUpgrade =
                    kind == .update && state.imageUpdateAvailable == true
                return next
            }

            if state.kind == .update
                && kind == .update
                && state.updateSource == .auto
                && source != nil
                && source != .auto
            {
                var next = state
                next.updateSource = source
                return next
            }

            return state
        }

        let leavesHealthy = kind == .reconnecting || kind.isResetLike
        var next = state
        next.kind = kind
        next.operationID = scopedOperationID
        next.resetOperationID = scopedOperationID
        next.updateSource = updateSource
        next.lockBoxID = state.observedBoxID
        next.hasLeftHealthy = leavesHealthy
        next.reconnectedSinceLeft = false
        next.readySince =
            !leavesHealthy && healthy(state.boxPhase) ? at : nil
        next.connectedSince =
            state.isConnected ? state.connectedSince ?? at : nil
        next.hasTerminalMigration = false
        next.hasRequestAcknowledgement = false
        next.expectsImageUpgrade =
            kind == .update && state.imageUpdateAvailable == true
        next.lastResolution = nil
        next.lastResolutionKind = nil
        return next
    }

    static func reduce(
        _ state: RemoteComputerRebuildState,
        _ event: RemoteComputerRebuildEvent
    ) -> RemoteComputerRebuildState {
        switch event {
        case .request(let kind, let operationID, let source, let at):
            return begin(
                state,
                kind: kind,
                operationID: operationID,
                source: source,
                at: at
            )

        case .pending(let isPending, _):
            if !isPending && state.kind == nil {
                var next = clearOperation(state, resolution: nil)
                next.isPending = false
                return next
            }
            var next = state
            next.isPending = isPending
            return next

        case .acknowledged:
            guard state.kind == .update || state.kind?.isResetLike == true else {
                return state
            }
            var next = state
            next.hasRequestAcknowledgement = true
            return next

        case .imageUpdate(let available, _):
            var next = state
            next.imageUpdateAvailable = available
            return next

        case .box(let boxID, let phase, let at):
            if state.observedBoxID == boxID && state.boxPhase == phase {
                return state
            }

            var next = state
            next.observedBoxID = boxID
            next.boxPhase = phase
            if healthy(phase) {
                next.lastHealthyBoxID = boxID
            }

            if next.kind == nil
                && !next.isPending
                && phase == "pulling"
                && boxID != nil
                && next.lastHealthyBoxID == boxID
            {
                next = begin(
                    next,
                    kind: .update,
                    operationID: nil,
                    source: .auto,
                    at: at
                )
            }

            guard next.kind != nil else { return next }
            if next.lockBoxID == nil, let boxID {
                next.lockBoxID = boxID
            }
            guard sameLockedBox(next) else {
                next.readySince = nil
                return next
            }

            if healthy(phase) {
                next.readySince = next.readySince ?? at
            } else {
                next.hasLeftHealthy = true
                next.teardownObserved = .box
                next.readySince = nil
            }
            return next

        case .migration(let operationID, let phase, let at):
            if phase == .failed {
                return state.kind == nil
                    ? state
                    : clearOperation(state, resolution: .failed)
            }

            if phase == .done {
                let eligible =
                    state.kind?.isResetLike == true
                    || (
                        state.kind == .update
                        && state.updateSource == .migration
                    )
                guard eligible, !state.hasTerminalMigration else {
                    return state
                }
                if state.operationID != nil
                    && operationID != nil
                    && !sameOperation(state.operationID, operationID)
                {
                    return state
                }
                var next = state
                next.hasTerminalMigration = true
                next.hasLeftHealthy = true
                next.readySince =
                    state.isConnected
                    && healthy(state.boxPhase)
                    && sameLockedBox(state)
                    ? at
                    : nil
                return next
            }

            if phase == .wiping {
                return begin(
                    state,
                    kind: .reset,
                    operationID: operationID,
                    source: nil,
                    at: at
                )
            }

            return begin(
                state,
                kind: .update,
                operationID: operationID,
                source: .migration,
                at: at
            )

        case .connection(let isConnected, let at):
            if isConnected {
                var next = state
                next.isConnected = true
                next.connectedSince = state.connectedSince ?? at
                next.reconnectedSinceLeft =
                    state.hasLeftHealthy || state.reconnectedSinceLeft
                if state.kind != nil
                    && state.hasLeftHealthy
                    && healthy(state.boxPhase)
                    && sameLockedBox(state)
                {
                    next.readySince = state.readySince ?? at
                }
                return next
            }

            var next = state
            next.isConnected = false
            next.connectedSince = nil
            if state.kind != nil {
                next.hasLeftHealthy = true
                if state.teardownObserved == .none {
                    next.teardownObserved = .transport
                }
                next.readySince = nil
            }
            return next

        case .error:
            return state.kind == nil
                ? state
                : clearOperation(state, resolution: .failed)

        case .deactivate:
            return state.kind == nil
                ? state
                : clearOperation(
                    state,
                    resolution: state.hasTerminalMigration ? .settled : .cancelled
                )

        case .tick:
            return state
        }
    }
}

struct RemoteComputerMigrationEvent: Equatable, Sendable {
    let operationID: RemoteComputerRebuildOperationID?
    let phase: RemoteComputerMigrationPhase
    let detail: String
}

struct RemoteComputerMigrationSnapshot: Equatable, Sendable {
    let operationID: RemoteComputerRebuildOperationID?
    let phase: RemoteComputerMigrationPhase?
    let detail: String
    let phases: [RemoteComputerMigrationPhase]

    static let empty = Self(
        operationID: nil,
        phase: nil,
        detail: "",
        phases: []
    )
}

struct RemoteComputerMigrationAccumulator: Equatable, Sendable {
    private(set) var current: RemoteComputerMigrationEvent?
    private(set) var snapshot: RemoteComputerMigrationSnapshot = .empty
    private var episodeOperationID: RemoteComputerRebuildOperationID?

    mutating func ingest(_ next: RemoteComputerMigrationEvent) -> Bool {
        guard current != next else { return false }

        let keepsAnonymousEpisode =
            episodeOperationID == nil
            && next.operationID == nil
            && current != nil
            && current?.phase.isTerminal != true

        if !keepsAnonymousEpisode
            && !Self.sameOperation(episodeOperationID, next.operationID)
        {
            episodeOperationID = next.operationID
            snapshot = .empty
        }

        var phases = snapshot.phases
        if !next.phase.isTerminal && phases.last != next.phase {
            phases.append(next.phase)
        }

        current = next
        snapshot = .init(
            operationID: episodeOperationID,
            phase: next.phase,
            detail: next.detail,
            phases: phases
        )
        return true
    }

    mutating func reset() {
        current = nil
        episodeOperationID = nil
        snapshot = .empty
    }

    private static func sameOperation(
        _ left: RemoteComputerRebuildOperationID?,
        _ right: RemoteComputerRebuildOperationID?
    ) -> Bool {
        guard let left, let right else { return false }
        return left.value == right.value
    }
}

struct RemoteComputerForeverBoxStatus: Equatable, Sendable {
    let agentID: String
    let state: String
    let pullPercent: Double?
    let vncURL: String?
    let imageUpdateAvailable: Bool?
}

enum RemoteComputerForeverBoxProjection {
    static func phase(
        _ value: RemoteComputerForeverBoxStatus,
        isStarting: Bool = false
    ) -> String {
        if value.pullPercent != nil {
            return "pulling"
        }
        if value.state == "running" {
            return value.vncURL?.isEmpty == false ? "running" : "local"
        }
        if isStarting {
            return "starting"
        }
        if value.state == "hibernated" {
            return "sleeping"
        }
        return "off"
    }
}

enum RemoteComputerReconnectVariant: String, Equatable, Sendable {
    case checking
    case network
    case restarting
}

struct RemoteComputerRebuildPresentation: Equatable, Sendable {
    let title: String
    let subtitle: String?
    let progress: Double?
    let reconnectVariant: RemoteComputerReconnectVariant?
    let accessibilityIdentifier: String

    static func project(
        state: RemoteComputerRebuildState,
        migration: RemoteComputerMigrationSnapshot
    ) -> Self? {
        guard let kind = state.kind else { return nil }

        if kind == .reconnecting {
            if state.boxPhase == "pulling" || state.boxPhase == "starting" {
                return .init(
                    title: "电脑正在重新启动",
                    subtitle: "正在启动我的电脑",
                    progress: nil,
                    reconnectVariant: .restarting,
                    accessibilityIdentifier: "remote-computer-rebuild-reconnecting"
                )
            }
            if !state.isConnected {
                return .init(
                    title: "正在重新连接",
                    subtitle: nil,
                    progress: nil,
                    reconnectVariant: .network,
                    accessibilityIdentifier: "remote-computer-rebuild-reconnecting"
                )
            }
            return .init(
                title: "正在检查连接",
                subtitle: "正在重新连接",
                progress: nil,
                reconnectVariant: .checking,
                accessibilityIdentifier: "remote-computer-rebuild-reconnecting"
            )
        }

        let steps: [String]
        switch kind {
        case .update:
            steps = [
                "正在准备",
                "正在备份数据",
                "正在重新创建电脑",
                "正在启动电脑",
                "正在清理",
                "正在重新连接",
            ]
        case .reset:
            steps = [
                "正在准备",
                "正在清除数据",
                "正在创建电脑",
                "正在启动电脑",
                "正在清理",
                "正在重新连接",
            ]
        case .recover:
            steps = [
                "正在准备",
                "正在恢复电脑",
                "正在启动电脑",
                "正在重新连接",
            ]
        case .reconnecting:
            return nil
        }

        let activeIndex = activeStepIndex(
            kind: kind,
            state: state,
            migration: migration,
            count: steps.count
        )
        let title: String
        switch kind {
        case .update: title = "正在更新我的电脑"
        case .reset: title = "正在重置我的电脑"
        case .recover: title = "正在恢复我的电脑"
        case .reconnecting: title = "正在重新连接"
        }

        return .init(
            title: title,
            subtitle: activeIndex.map { steps[$0] },
            progress: activeIndex.map { Double($0) / Double(max(steps.count, 1)) },
            reconnectVariant: nil,
            accessibilityIdentifier: "remote-computer-rebuild-\(kind.rawValue)"
        )
    }

    private static func activeStepIndex(
        kind: RemoteComputerRebuildKind,
        state: RemoteComputerRebuildState,
        migration: RemoteComputerMigrationSnapshot,
        count: Int
    ) -> Int? {
        var active = 0
        var afterHealthyPhase = false
        let phases = migration.phases
            + (
                migration.phase.map { phase in
                    !phase.isTerminal && migration.phases.last != phase ? [phase] : []
                } ?? []
            )

        for phase in phases {
            let index: Int
            switch kind {
            case .reset:
                switch phase {
                case .wiping: index = 1
                case .creating: index = 2
                case .moving: index = 3
                case .cleaningUp: index = afterHealthyPhase ? 4 : 1
                case .backingUp: index = 0
                case .done, .failed: index = active
                }
            case .recover:
                switch phase {
                case .backingUp, .wiping, .creating: index = 1
                case .moving: index = 2
                case .cleaningUp: index = afterHealthyPhase ? 3 : 1
                case .done, .failed: index = active
                }
            case .update:
                switch phase {
                case .backingUp: index = 1
                case .creating: index = 2
                case .moving: index = 3
                case .cleaningUp: index = afterHealthyPhase ? 4 : 2
                case .wiping: index = 0
                case .done, .failed: index = active
                }
            case .reconnecting:
                index = 0
            }
            active = max(active, index)
            if phase != .cleaningUp {
                afterHealthyPhase = true
            }
        }

        if state.boxPhase == "starting" || state.boxPhase == "pulling" {
            switch kind {
            case .update, .reset:
                active = max(active, 3)
            case .recover:
                active = max(active, 2)
            case .reconnecting:
                break
            }
        }

        if state.hasTerminalMigration && state.isConnected
            && (state.boxPhase == "running" || state.boxPhase == "local")
        {
            active = max(active, count - 1)
        }

        guard active >= 0, active < count else { return nil }
        return active
    }
}

@MainActor
protocol RemoteComputerRebuildSource: AnyObject {
    /// True only when the current iOS production graph has a real lifecycle
    /// service behind the Desktop-derived migration/update/recreate contract.
    /// The Main RPC method table alone is not evidence of a serving route.
    var supportsManagedLifecycle: Bool { get }

    func getMigrationStatus() async throws -> Any
    func update(force: Bool) async throws -> Any
    func recreate() async throws -> Any
}

@MainActor
final class IOSRemoteComputerRebuildSource: RemoteComputerRebuildSource {
    enum SourceError: LocalizedError {
        case bridgeUnavailable

        var errorDescription: String? {
            "remote_computer_bridge_unavailable"
        }
    }

    private let bridge: IOSPreloadBridge?

    /// The current Rust Product/Host graph does not yet serve the Desktop
    /// Cursor-box lifecycle methods. Keep the UI fail-closed until that owner is
    /// ported; otherwise every visible update/reset action deterministically
    /// falls through to "unknown method".
    let supportsManagedLifecycle = false

    init(bridge: IOSPreloadBridge?) {
        self.bridge = bridge
    }

    func getMigrationStatus() async throws -> Any {
        guard let bridge else { throw SourceError.bridgeUnavailable }
        return try await bridge.request(method: "getBoxMigrationStatus").value
    }

    func update(force: Bool) async throws -> Any {
        guard let bridge else { throw SourceError.bridgeUnavailable }
        return try await bridge.request(
            method: "updateComputer",
            params: [
                "id": remoteComputerForeverBoxID,
                "force": force,
            ]
        ).value
    }

    func recreate() async throws -> Any {
        guard let bridge else { throw SourceError.bridgeUnavailable }
        return try await bridge.request(method: "forceRecreateComputer").value
    }

}

@MainActor
final class RemoteComputerRebuildOwner: ObservableObject {
    @Published private(set) var state = RemoteComputerRebuildState.initial()
    @Published private(set) var migrationSnapshot = RemoteComputerMigrationSnapshot.empty
    @Published private(set) var isHydrating = false
    @Published private(set) var requestError: String?
    @Published private(set) var reloadRevision = 0

    private let source: any RemoteComputerRebuildSource
    private let now: () -> Int64
    private var migrationAccumulator = RemoteComputerMigrationAccumulator()
    private var connected = false
    private var disposed = false
    private var hydrationGeneration = 0
    private var requestGeneration = 0
    private var hasObservedHealthyWebSession = false
    private var webSessionHealthy = false

    init(
        source: any RemoteComputerRebuildSource,
        now: @escaping () -> Int64 = {
            Int64((Date().timeIntervalSince1970 * 1_000).rounded())
        }
    ) {
        self.source = source
        self.now = now
    }

    var managedLifecycleAvailable: Bool {
        source.supportsManagedLifecycle
    }

    func connect() async {
        guard !disposed else { return }
        connected = true
        if source.supportsManagedLifecycle {
            await hydrateMigration()
        }
    }

    func noteReconnect() async {
        guard !disposed, connected else { return }
        if source.supportsManagedLifecycle {
            await hydrateMigration()
        }
        if webSessionHealthy {
            ingest(.box(
                boxID: remoteComputerForeverBoxID,
                phase: "running",
                at: now()
            ))
            ingest(.connection(isConnected: true, at: now()))
        }
    }

    func requestUpdate(force: Bool = false) async {
        await performRequest(kind: .update, force: force)
    }

    func requestReset() async {
        await performRequest(kind: .reset, force: false)
    }

    func requestRecover() async {
        await performRequest(kind: .recover, force: false)
    }

    func requestReconnect() async {
        guard !disposed else { return }
        requestGeneration &+= 1
        requestError = nil
        reloadRevision &+= 1
        ingest(.request(
            kind: .reconnecting,
            operationID: nil,
            source: nil,
            at: now()
        ))
        // On iOS, the existing restricted WKWebView is the transport owner.
        // Changing reloadRevision performs the real reconnect; its navigation
        // callbacks drive connection/box teardown and recovery below.
    }

    func noteNavigationStarted() {
        guard !disposed else { return }
        let at = now()
        if hasObservedHealthyWebSession {
            if state.kind == nil {
                ingest(.request(
                    kind: .reconnecting,
                    operationID: nil,
                    source: nil,
                    at: at
                ))
            }
            ingest(.connection(isConnected: false, at: at))
        }
        webSessionHealthy = false
        ingest(.box(
            boxID: remoteComputerForeverBoxID,
            phase: "starting",
            at: at
        ))
    }

    func noteNavigationFinished() {
        guard !disposed else { return }
        let at = now()
        webSessionHealthy = true
        hasObservedHealthyWebSession = true
        ingest(.box(
            boxID: remoteComputerForeverBoxID,
            phase: "running",
            at: at
        ))
        ingest(.connection(isConnected: true, at: at))

        if state.hasTerminalMigration || state.kind == .reconnecting {
            ingest(.deactivate(at: at))
        }
    }

    func noteNavigationFailed() {
        guard !disposed else { return }
        webSessionHealthy = false
        let at = now()
        ingest(.connection(isConnected: false, at: at))
        ingest(.box(
            boxID: remoteComputerForeverBoxID,
            phase: "off",
            at: at
        ))
    }

    func dispose() {
        guard !disposed else { return }
        hydrationGeneration &+= 1
        requestGeneration &+= 1
        connected = false
        isHydrating = false
        if state.kind != nil {
            ingest(.deactivate(at: now()))
        }
        disposed = true
    }

    private func performRequest(
        kind: RemoteComputerRebuildKind,
        force: Bool
    ) async {
        guard !disposed, !state.isPending else { return }
        guard kind == .reconnecting || source.supportsManagedLifecycle else {
            requestError = "当前 iOS 版本尚未接入远程电脑的更新/重建服务。"
            return
        }

        hydrationGeneration &+= 1
        requestGeneration &+= 1
        let attempt = requestGeneration
        requestError = nil
        let at = now()
        ingest(.request(
            kind: kind,
            operationID: nil,
            source: kind == .update ? .request : nil,
            at: at
        ))
        ingest(.pending(true, at: at))

        if kind == .reconnecting {
            reloadRevision &+= 1
        }

        do {
            let response: Any
            switch kind {
            case .update:
                response = try await source.update(force: force)
            case .reset, .recover:
                response = try await source.recreate()
            case .reconnecting:
                // requestReconnect is a native WebKit transport operation and
                // never enters this backend path.
                return
            }

            guard !disposed, attempt == requestGeneration else { return }
            ingest(.pending(false, at: now()))
            ingest(.acknowledged(at: now()))

            if kind.isResetLike,
               let operationID = Self.operationID(from: response)
            {
                ingest(.request(
                    kind: kind,
                    operationID: operationID,
                    source: nil,
                    at: now()
                ))
            }

            if let responseStatus = Self.responseStatus(from: response),
               responseStatus.status == "rejected"
            {
                requestError = responseStatus.reason ?? "电脑操作被拒绝。"
                ingest(.error(at: now()))
                return
            }

            await hydrateMigration()
        } catch {
            guard !disposed, attempt == requestGeneration else { return }
            ingest(.pending(false, at: now()))
            requestError = String(error.localizedDescription.prefix(240))
            ingest(.error(at: now()))
        }
    }

    private func hydrateMigration() async {
        guard !disposed, connected else { return }
        hydrationGeneration &+= 1
        let attempt = hydrationGeneration
        isHydrating = true

        do {
            let value = try await source.getMigrationStatus()
            guard !disposed,
                  connected,
                  attempt == hydrationGeneration
            else { return }
            if let event = Self.parseMigrationEvent(value) {
                ingestMigration(event)
            }
        } catch {
            // Desktop's hydration stores deliberately keep their last good
            // snapshot on hydrate failure. The WebKit transport remains the
            // native box/connection authority, so a failed status read does
            // not invent a second error state.
        }

        if !disposed, attempt == hydrationGeneration {
            isHydrating = false
        }
    }

    private func ingestMigration(_ event: RemoteComputerMigrationEvent) {
        guard migrationAccumulator.ingest(event) else { return }
        migrationSnapshot = migrationAccumulator.snapshot
        ingest(.migration(
            operationID: event.operationID,
            phase: event.phase,
            at: now()
        ))

        if event.phase == .done,
           webSessionHealthy,
           state.hasTerminalMigration
        {
            ingest(.box(
                boxID: remoteComputerForeverBoxID,
                phase: "running",
                at: now()
            ))
            ingest(.connection(isConnected: true, at: now()))
            ingest(.deactivate(at: now()))
        }
    }

    private func ingest(_ event: RemoteComputerRebuildEvent) {
        state = RemoteComputerRebuildReducer.reduce(state, event)
    }

    private static func parseMigrationEvent(
        _ value: Any
    ) -> RemoteComputerMigrationEvent? {
        guard let object = value as? [String: Any],
              let rawPhase = object["phase"] as? String,
              let phase = RemoteComputerMigrationPhase(rawValue: rawPhase)
        else {
            return nil
        }

        let rawOperationID = object["operationId"]
        let operationID = operationID(from: rawOperationID)
        if rawOperationID != nil,
           !(rawOperationID is NSNull),
           operationID == nil
        {
            return nil
        }

        return .init(
            operationID: operationID,
            phase: phase,
            detail: object["detail"] as? String ?? ""
        )
    }

    private static func operationID(
        from value: Any?
    ) -> RemoteComputerRebuildOperationID? {
        if let string = value as? String {
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : .init(value: trimmed)
        }
        if let object = value as? [String: Any],
           let string = object["value"] as? String
        {
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : .init(value: trimmed)
        }
        if let object = value as? [String: Any],
           let nested = object["operationId"]
        {
            return operationID(from: nested)
        }
        return nil
    }

    private static func responseStatus(
        from value: Any
    ) -> (status: String, reason: String?)? {
        guard let object = value as? [String: Any],
              let status = object["status"] as? String
        else {
            return nil
        }
        return (status, object["reason"] as? String)
    }
}

struct RemoteComputerHostActivitySnapshot: Equatable, Sendable {
    let agentID: String?
    let runningComputerSubagentIDs: [String]
    let isComputerUseTaskActive: Bool

    static let empty = Self(
        agentID: nil,
        runningComputerSubagentIDs: [],
        isComputerUseTaskActive: false
    )

    var isActive: Bool {
        !runningComputerSubagentIDs.isEmpty || isComputerUseTaskActive
    }
}

struct IOSRemoteComputerHostActivitySource {
    let bridge: IOSPreloadBridge?

    @MainActor
    func load(agentID: String) async throws -> RemoteComputerHostActivitySnapshot {
        guard let bridge else {
            throw NSError(
                domain: "Fabushi.RemoteComputerActivity",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Host bridge is unavailable"]
            )
        }

        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": [
                    "type": "subagent.list",
                    "requestId": requestID("subagents"),
                    "agentId": agentID,
                ],
            ]
        )
        let subagentResult = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 5_120
        ) { event in
            event["type"] as? String == "subagent.listed"
                && event["agentId"] as? String == agentID
        }
        guard let subagentEvent = subagentResult.value as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }

        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": [
                    "type": "asyncTask.list",
                    "requestId": requestID("tasks"),
                    "agentId": agentID,
                ],
            ]
        )
        let taskResult = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 5_120
        ) { event in
            event["type"] as? String == "asyncTask.listed"
                && event["agentId"] as? String == agentID
        }
        guard let taskEvent = taskResult.value as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }

        return Self.project(
            agentID: agentID,
            subagentEvent: subagentEvent,
            taskEvent: taskEvent
        )
    }

    static func project(
        agentID: String,
        subagentEvent: [String: Any],
        taskEvent: [String: Any]
    ) -> RemoteComputerHostActivitySnapshot {
        let subagents = subagentEvent["subagents"] as? [[String: Any]] ?? []
        let runningComputerSubagentIDs = subagents.compactMap { row -> String? in
            guard row["status"] as? String == "running",
                  row["subagentType"] as? String == "computerUse",
                  let id = row["id"] as? String,
                  !id.isEmpty
            else { return nil }
            return id
        }

        let tasks = taskEvent["tasks"] as? [[String: Any]] ?? []
        let hasComputerTask = tasks.contains { row in
            row["status"] as? String == "running"
                && row["subagentType"] as? String == "computerUse"
        }
        return .init(
            agentID: agentID,
            runningComputerSubagentIDs: runningComputerSubagentIDs,
            isComputerUseTaskActive: hasComputerTask
        )
    }

    private func requestID(_ suffix: String) -> String {
        "ios-computer-\(suffix)-\(UUID().uuidString.lowercased())"
    }
}

@MainActor
final class RemoteComputerHostActivityOwner: ObservableObject {
    typealias Loader = @MainActor (String) async throws -> RemoteComputerHostActivitySnapshot

    @Published private(set) var snapshot = RemoteComputerHostActivitySnapshot.empty
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastError: String?

    private let loader: Loader
    private var generation = 0
    private var disposed = false

    init(loader: @escaping Loader) {
        self.loader = loader
    }

    func refresh(agentID: String?) async {
        generation &+= 1
        let requestGeneration = generation
        guard !disposed else { return }

        guard let agentID, !agentID.isEmpty else {
            snapshot = .empty
            isRefreshing = false
            lastError = nil
            return
        }

        isRefreshing = true
        lastError = nil
        do {
            let next = try await loader(agentID)
            guard !disposed, generation == requestGeneration else { return }
            snapshot = next
            isRefreshing = false
        } catch is CancellationError {
            guard !disposed, generation == requestGeneration else { return }
            isRefreshing = false
        } catch {
            guard !disposed, generation == requestGeneration else { return }
            snapshot = .init(
                agentID: agentID,
                runningComputerSubagentIDs: [],
                isComputerUseTaskActive: false
            )
            isRefreshing = false
            lastError = error.localizedDescription
        }
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        generation &+= 1
        isRefreshing = false
    }
}

/// The existing shipping remote-computer surface remains the sole native
/// Computer UI owner. Desktop's empty lazy overlay is intentionally not copied:
/// native iOS renders the rebuild/reconnect state directly above this WebKit
/// session, and the WebKit navigation lifecycle is the platform transport/box
/// signal instead of a second reachability watcher.
struct RemoteComputerSurface: View {
    let scope: RemoteComputerScope?
    let reconnectGeneration: Int
    let onClose: () -> Void

    @StateObject private var rebuildOwner: RemoteComputerRebuildOwner
    @StateObject private var activityOwner: RemoteComputerHostActivityOwner
    @State private var status = "正在连接我的电脑…"
    @State private var errorMessage: String?
    @State private var resetConfirmationPresented = false
    @State private var recoverConfirmationPresented = false

    init(
        bridge: IOSPreloadBridge? = nil,
        scope: RemoteComputerScope? = nil,
        reconnectGeneration: Int = 0,
        onClose: @escaping () -> Void
    ) {
        self.scope = scope
        self.reconnectGeneration = reconnectGeneration
        self.onClose = onClose
        _rebuildOwner = StateObject(
            wrappedValue: RemoteComputerRebuildOwner(
                source: IOSRemoteComputerRebuildSource(bridge: bridge)
            )
        )
        let activitySource = IOSRemoteComputerHostActivitySource(bridge: bridge)
        _activityOwner = StateObject(
            wrappedValue: RemoteComputerHostActivityOwner { agentID in
                try await activitySource.load(agentID: agentID)
            }
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Button("返回", action: onClose)
                    .accessibilityIdentifier("remote-computer-close")

                VStack(alignment: .leading, spacing: 2) {
                    Text(scope?.displayTitle ?? "我的电脑")
                        .font(.headline)
                        .accessibilityIdentifier("remote-computer-scope-title")
                    Text(status)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("remote-computer-status")
                    if activityOwner.snapshot.isActive {
                        Text(activityOwner.snapshot.runningComputerSubagentIDs.isEmpty
                            ? "Computer Use 任务活动中"
                            : "Computer Use 活动中 · \(activityOwner.snapshot.runningComputerSubagentIDs.count) 个子任务")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .accessibilityIdentifier("remote-computer-agent-activity")
                    }
                }

                Spacer()

                if errorMessage == nil && status != "已安全连接" {
                    ProgressView()
                        .controlSize(.small)
                        .accessibilityIdentifier("remote-computer-loading")
                }

                Menu {
                    if rebuildOwner.managedLifecycleAvailable {
                        Button("更新电脑") {
                            Task { await rebuildOwner.requestUpdate() }
                        }
                    }
                    Button("重新连接") {
                        errorMessage = nil
                        status = "正在重新连接…"
                        Task { await rebuildOwner.requestReconnect() }
                    }
                    if rebuildOwner.managedLifecycleAvailable {
                        Button("重置电脑", role: .destructive) {
                            resetConfirmationPresented = true
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .disabled(rebuildOwner.state.isPending || rebuildOwner.state.kind != nil)
                .accessibilityIdentifier("remote-computer-actions")
            }
            .padding(12)

            if let presentation = RemoteComputerRebuildPresentation.project(
                state: rebuildOwner.state,
                migration: rebuildOwner.migrationSnapshot
            ) {
                RemoteComputerRebuildBanner(
                    presentation: presentation,
                    detail: rebuildOwner.migrationSnapshot.detail,
                    isHydrating: rebuildOwner.isHydrating
                )
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            }

            if let requestError = rebuildOwner.requestError {
                Text(requestError)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
                    .accessibilityIdentifier("remote-computer-rebuild-error")
            }

            if let errorMessage {
                VStack(alignment: .leading, spacing: 10) {
                    Text("无法打开远程电脑")
                        .font(.headline)
                    Text(errorMessage)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack {
                        Button("重新连接") {
                            self.errorMessage = nil
                            status = "正在重新连接…"
                            Task { await rebuildOwner.requestReconnect() }
                        }
                        .accessibilityIdentifier("remote-computer-reload")

                        if rebuildOwner.managedLifecycleAvailable {
                            Button("恢复电脑") {
                                recoverConfirmationPresented = true
                            }
                            .accessibilityIdentifier("remote-computer-recover")
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
                .accessibilityIdentifier("remote-computer-error")
            }

            RemoteComputerWebView(
                reloadToken: rebuildOwner.reloadRevision,
                status: $status,
                errorMessage: $errorMessage,
                onNavigationStarted: { rebuildOwner.noteNavigationStarted() },
                onNavigationFinished: { rebuildOwner.noteNavigationFinished() },
                onNavigationFailed: { rebuildOwner.noteNavigationFailed() }
            )
        }
        .background(Color(uiColor: .systemBackground))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("remote-computer-surface")
        .id(scope?.scopeKey ?? "account")
        .task(id: scope?.scopeKey ?? "account") {
            await rebuildOwner.connect()
            await activityOwner.refresh(agentID: scope?.agentID)
        }
        .onChange(of: reconnectGeneration) { _, _ in
            Task {
                await rebuildOwner.noteReconnect()
                await activityOwner.refresh(agentID: scope?.agentID)
            }
        }
        .onDisappear {
            rebuildOwner.dispose()
            activityOwner.dispose()
        }
        .confirmationDialog(
            "重置我的电脑？",
            isPresented: $resetConfirmationPresented,
            titleVisibility: .visible
        ) {
            Button("重置电脑", role: .destructive) {
                Task { await rebuildOwner.requestReset() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("重置会重新创建远程电脑。")
        }
        .confirmationDialog(
            "恢复我的电脑？",
            isPresented: $recoverConfirmationPresented,
            titleVisibility: .visible
        ) {
            Button("恢复电脑", role: .destructive) {
                errorMessage = nil
                status = "正在恢复我的电脑…"
                Task { await rebuildOwner.requestRecover() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("恢复会重新创建当前远程电脑。")
        }
    }
}

private struct RemoteComputerRebuildBanner: View {
    let presentation: RemoteComputerRebuildPresentation
    let detail: String
    let isHydrating: Bool

    var body: some View {
        HStack(spacing: 10) {
            if let progress = presentation.progress {
                ProgressView(value: progress)
                    .frame(width: 28)
            } else {
                ProgressView()
                    .controlSize(.small)
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(presentation.title)
                    .font(.subheadline.weight(.semibold))
                if let subtitle = presentation.subtitle {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if !detail.isEmpty {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }

            Spacer()

            if isHydrating {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .foregroundStyle(.secondary)
                    .accessibilityLabel("正在刷新电脑状态")
            }
        }
        .padding(10)
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 12))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(presentation.accessibilityIdentifier)
    }
}

enum RemoteComputerShellPhase: String, Equatable, Sendable {
    case off
    case starting
    case sleeping
    case local
    case running
    case pulling
}

enum RemoteComputerShellReadState: String, Equatable, Sendable {
    case unknown
    case known
    case error
}

struct RemoteComputerShellHandoff: Equatable, Sendable {
    let requestID: String
    let instruction: String
    let snapshotDataURL: String?
}

struct RemoteComputerShellStatusProjection {
    let phase: RemoteComputerShellPhase
    let readState: RemoteComputerShellReadState
    let isStatusKnown: Bool
    let isStatusUnavailable: Bool
    let pullPercent: Double?
    let vncURL: String?
    let handoff: RemoteComputerShellHandoff?
    let windows: [Any]
}

struct RemoteComputerShellMonitor: Equatable, Sendable {
    let subagentID: String
    let title: String
    let vncURL: String
    let handoff: RemoteComputerShellHandoff?
}

struct RemoteComputerShellStageCopy: Equatable, Sendable {
    let message: String
    let progressPercent: Double?
    let isBusy: Bool
    let hasRetry: Bool
}

enum RemoteComputerShellCursorType: String, Equatable, Sendable {
    case click
    case drag
    case move
    case scroll
}

struct RemoteComputerShellCursor: Equatable, Sendable {
    let x: Double
    let y: Double
    let type: RemoteComputerShellCursorType
    let sequence: Int
    let clickSequence: Int
    let millisecondsSinceMove: Int64?
    let lastMovedAtMilliseconds: Int64?
}

struct RemoteComputerShellCursorPresentation: Equatable, Sendable {
    struct Press: Equatable, Sendable {
        let key: Int
        let delayMilliseconds: Int64
    }

    let isGliding: Bool
    let isVisible: Bool
    let press: Press?
}

struct RemoteComputerShellVNCSession: Equatable, Sendable {
    enum Phase: String, Equatable, Sendable {
        case connect = "rfb_connect"
        case disconnect = "rfb_disconnect"
        case reconnect
    }

    let phase: Phase
    let clean: Bool?
}

struct RemoteComputerShellVNCIdentity: Equatable, Sendable {
    let host: String?
    let display: String?
}

enum RemoteComputerShellModel {
    static let statusTimeoutMilliseconds: Int64 = 15_000
    static let crashLimit = 3
    static let crashWindowMilliseconds: Int64 = 60_000
    static let focusDelayMilliseconds: Int64 = 32
    static let warmPreviewLimit = 3
    static let directMonitorLimit = 3
    static let activeHoldMilliseconds: Int64 = 2_500

    static func projectHandoff(_ value: Any?) -> RemoteComputerShellHandoff? {
        guard let object = value as? [String: Any],
              let requestID = object["requestId"] as? String,
              let instruction = object["instruction"] as? String
        else { return nil }
        return .init(
            requestID: requestID,
            instruction: instruction,
            snapshotDataURL: object["snapshotDataUrl"] as? String
        )
    }

    static func projectStatus(
        _ value: Any?,
        readState: RemoteComputerShellReadState,
        isEnsureStarting: Bool = false
    ) -> RemoteComputerShellStatusProjection {
        let status = value as? [String: Any]
        let state = status?["state"] as? String
        let rawVNC = status?["vncUrl"] as? String
        let vncURL = state == "running" && rawVNC?.isEmpty == false ? rawVNC : nil
        let pull = status?["pull"] as? [String: Any]

        let phase: RemoteComputerShellPhase
        if pull != nil {
            phase = .pulling
        } else if state == "running" {
            phase = vncURL == nil ? .local : .running
        } else if isEnsureStarting {
            phase = .starting
        } else if state == "hibernated" {
            phase = .sleeping
        } else {
            phase = .off
        }

        return .init(
            phase: phase,
            readState: readState,
            isStatusKnown: readState == .known,
            isStatusUnavailable: readState == .error,
            pullPercent: numeric(pull?["percent"]),
            vncURL: vncURL,
            handoff: projectHandoff(status?["handoff"]),
            windows: vncURL == nil ? [] : (status?["windows"] as? [Any] ?? [])
        )
    }

    static func projectMonitors(
        subagents: Any?,
        statusFor: (String) -> Any?
    ) -> [RemoteComputerShellMonitor] {
        guard let rows = subagents as? [Any] else { return [] }
        return rows.compactMap { raw in
            guard let row = raw as? [String: Any],
                  row["status"] as? String == "running",
                  row["subagentType"] as? String == "computerUse",
                  let id = row["subagentId"] as? String,
                  let status = statusFor(id) as? [String: Any],
                  status["state"] as? String == "running",
                  let vncURL = status["vncUrl"] as? String,
                  !vncURL.isEmpty
            else { return nil }
            let title = (row["title"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return .init(
                subagentID: id,
                title: title.isEmpty ? "Subagent" : title,
                vncURL: vncURL,
                handoff: nil
            )
        }
    }

    static func isComputerUseTaskActive(_ value: Any?) -> Bool {
        guard let rows = value as? [Any] else { return false }
        return rows.contains {
            ($0 as? [String: Any])?["subagentType"] as? String == "computerUse"
        }
    }

    static func stageCopy(
        isScreenLoading: Bool,
        isScreenUnavailable: Bool,
        subjectLabel: String,
        emptyMessage: String? = nil,
        isEmptyLoading: Bool,
        pullPercent: Double?
    ) -> RemoteComputerShellStageCopy {
        if isScreenLoading {
            return .init(
                message: "Switching to \(subjectLabel)'s screen…",
                progressPercent: nil,
                isBusy: true,
                hasRetry: false
            )
        }
        if isScreenUnavailable {
            return .init(
                message: "Can't reach \(subjectLabel)'s screen",
                progressPercent: nil,
                isBusy: false,
                hasRetry: true
            )
        }
        if let pullPercent {
            return .init(
                message: "Setting up the computer",
                progressPercent: pullPercent,
                isBusy: true,
                hasRetry: false
            )
        }
        return .init(
            message: emptyMessage ?? "Booting up the computer",
            progressPercent: nil,
            isBusy: isEmptyLoading,
            hasRetry: false
        )
    }

    static func vncDimensions(_ value: String) -> (width: Int, height: Int) {
        guard let url = URL(string: value),
              url.path.hasSuffix("/sand-special-treatment-v1/vnc.html")
        else { return (1280, 800) }
        return (2048, 2048)
    }

    static func retainWarmVNCSources(
        _ previous: [String],
        source: String?,
        maxWarm: Int = warmPreviewLimit
    ) -> [String] {
        guard let source, previous.first != source else { return previous }
        let limit = max(1, maxWarm)
        let next = [source] + previous.filter { $0 != source }
        return Array(next.prefix(limit))
    }

    static func vncViewerURL(_ value: String, interactive: Bool) -> URL? {
        guard var components = URLComponents(string: value) else { return nil }
        var items = components.queryItems ?? []
        func set(_ name: String, _ value: String) {
            items.removeAll { $0.name == name }
            items.append(URLQueryItem(name: name, value: value))
        }
        set("autoconnect", "true")
        set("resize", "scale")
        set("reconnect", "true")
        if interactive {
            set("sandInteractive", "1")
        } else {
            items.removeAll { $0.name == "sandInteractive" }
        }
        components.queryItems = items
        return components.url
    }

    static func vncIdentity(_ value: String) -> RemoteComputerShellVNCIdentity {
        guard let url = URL(string: value),
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        else { return .init(host: nil, display: nil) }

        if let path = components.queryItems?.first(where: { $0.name == "path" })?.value,
           let question = path.firstIndex(of: "?")
        {
            let query = String(path[path.index(after: question)...])
            if let nested = URLComponents(string: "https://local.invalid/?\(query)"),
               let token = nested.queryItems?.first(where: { $0.name == "token" })?.value,
               !token.isEmpty
            {
                return .init(host: url.host, display: token)
            }
        }
        return .init(host: url.host, display: "primary")
    }

    static func parseVNCSession(_ value: Any?) -> RemoteComputerShellVNCSession? {
        guard let string = value as? String,
              let data = string.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let phaseValue = object["phase"] as? String,
              let phase = RemoteComputerShellVNCSession.Phase(rawValue: phaseValue)
        else { return nil }
        return .init(phase: phase, clean: object["clean"] as? Bool)
    }

    static func projectCursor(
        _ value: Any?,
        previous: RemoteComputerShellCursor?,
        nowMilliseconds: Int64
    ) -> RemoteComputerShellCursor? {
        guard let object = value as? [String: Any],
              object["agentId"] is String,
              let typeValue = object["type"] as? String,
              let type = RemoteComputerShellCursorType(rawValue: typeValue),
              let x = numeric(object["x"]), x >= 0,
              let y = numeric(object["y"]), y >= 0
        else { return nil }

        let changed = previous.map { $0.x != x || $0.y != y } ?? false
        let lastMoved = changed
            ? nowMilliseconds
            : previous?.lastMovedAtMilliseconds
        return .init(
            x: x,
            y: y,
            type: type,
            sequence: (previous?.sequence ?? 0) + 1,
            clickSequence: (previous?.clickSequence ?? 0) + (type == .click ? 1 : 0),
            millisecondsSinceMove: lastMoved.map { max(0, nowMilliseconds - $0) },
            lastMovedAtMilliseconds: lastMoved
        )
    }

    static func cursorPresentation(
        _ cursor: RemoteComputerShellCursor?,
        hasFrame: Bool
    ) -> RemoteComputerShellCursorPresentation {
        let visible = cursor != nil && hasFrame
        let press: RemoteComputerShellCursorPresentation.Press?
        if visible, cursor?.type == .click, let cursor {
            press = .init(
                key: cursor.clickSequence,
                delayMilliseconds: cursor.millisecondsSinceMove.map {
                    max(0, 500 - $0)
                } ?? 0
            )
        } else {
            press = nil
        }
        return .init(
            isGliding: (cursor?.sequence ?? 0) > 1,
            isVisible: visible,
            press: press
        )
    }

    static func firstSelectedMonitor(
        _ monitors: [RemoteComputerShellMonitor],
        requested: String?
    ) -> String? {
        if let requested, monitors.contains(where: { $0.subagentID == requested }) {
            return requested
        }
        return monitors.first(where: { $0.handoff != nil })?.subagentID
            ?? monitors.first?.subagentID
    }

    static func stepSelectedMonitor(
        _ monitors: [RemoteComputerShellMonitor],
        current: String?,
        delta: Int
    ) -> String? {
        guard monitors.count >= 2 else { return current }
        let found = monitors.firstIndex(where: { $0.subagentID == current }) ?? 0
        let normalized = delta < 0 ? -1 : 1
        let index = (found + normalized + monitors.count) % monitors.count
        return monitors[index].subagentID
    }

    static func handoffStatusLabel(
        _ status: String
    ) -> (label: String, muted: Bool) {
        switch status {
        case "waiting": return ("Action needed", false)
        case "handed_back": return ("Done", false)
        case "replied": return ("Answered", false)
        case "dismissed": return ("Skipped", true)
        default: return ("Status unavailable", true)
        }
    }

    private static func numeric(_ value: Any?) -> Double? {
        if let value = value as? Double { return value.isFinite ? value : nil }
        if let value = value as? Int { return Double(value) }
        if let value = value as? Int64 { return Double(value) }
        if let value = value as? NSNumber { return value.doubleValue.isFinite ? value.doubleValue : nil }
        return nil
    }
}

enum RemoteComputerWebProcessCrashRecovery: Equatable, Sendable {
    case reload
    case failClosed
}

struct RemoteComputerWebProcessCrashPolicy: Equatable, Sendable {
    static let automaticReloadLimit = RemoteComputerShellModel.crashLimit
    static let crashWindowMilliseconds = RemoteComputerShellModel.crashWindowMilliseconds

    private(set) var crashCount = 0
    private(set) var lastCrashAtMilliseconds: Int64?
    private(set) var failedClosed = false

    mutating func recordCrash(
        atMilliseconds now: Int64
    ) -> RemoteComputerWebProcessCrashRecovery {
        let sameWindow =
            lastCrashAtMilliseconds.map {
                now >= $0 && now - $0 < Self.crashWindowMilliseconds
            } ?? false
        crashCount = sameWindow ? crashCount + 1 : 1
        lastCrashAtMilliseconds = now
        failedClosed = crashCount > Self.automaticReloadLimit
        return failedClosed ? .failClosed : .reload
    }

    mutating func resetForExplicitReload() {
        crashCount = 0
        lastCrashAtMilliseconds = nil
        failedClosed = false
    }
}

private struct RemoteComputerWebView: UIViewRepresentable {
    let reloadToken: Int
    @Binding var status: String
    @Binding var errorMessage: String?
    let onNavigationStarted: @MainActor () -> Void
    let onNavigationFinished: @MainActor () -> Void
    let onNavigationFailed: @MainActor () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(
            status: $status,
            errorMessage: $errorMessage,
            onNavigationStarted: onNavigationStarted,
            onNavigationFinished: onNavigationFinished,
            onNavigationFailed: onNavigationFailed
        )
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.mediaTypesRequiringUserActionForPlayback = []

        // This restricted surface deliberately has no native script handler or
        // injected WebMCP bridge. Computer lifecycle state uses the app's
        // Coordinator RPC plus this exact WebKit navigation lifecycle.
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.accessibilityIdentifier = "remote-computer-webview"
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        webView.isInspectable = false
        context.coordinator.webView = webView
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard context.coordinator.loadedReloadToken != reloadToken else { return }
        context.coordinator.loadedReloadToken = reloadToken
        context.coordinator.prepareExplicitReload()
        webView.load(
            URLRequest(
                url: remoteComputerURL,
                cachePolicy: .useProtocolCachePolicy
            )
        )
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.stopLoading()
        webView.navigationDelegate = nil
        coordinator.webView = nil
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        @Binding private var status: String
        @Binding private var errorMessage: String?
        private let onNavigationStarted: @MainActor () -> Void
        private let onNavigationFinished: @MainActor () -> Void
        private let onNavigationFailed: @MainActor () -> Void
        weak var webView: WKWebView?
        var loadedReloadToken: Int?
        private var crashPolicy = RemoteComputerWebProcessCrashPolicy()

        init(
            status: Binding<String>,
            errorMessage: Binding<String?>,
            onNavigationStarted: @escaping @MainActor () -> Void,
            onNavigationFinished: @escaping @MainActor () -> Void,
            onNavigationFailed: @escaping @MainActor () -> Void
        ) {
            _status = status
            _errorMessage = errorMessage
            self.onNavigationStarted = onNavigationStarted
            self.onNavigationFinished = onNavigationFinished
            self.onNavigationFailed = onNavigationFailed
        }

        func webView(
            _ webView: WKWebView,
            didStartProvisionalNavigation navigation: WKNavigation?
        ) {
            errorMessage = nil
            status = "正在安全连接…"
            onNavigationStarted()
        }

        func webView(
            _ webView: WKWebView,
            didFinish navigation: WKNavigation?
        ) {
            if errorMessage == nil {
                status = "已安全连接"
            }
            onNavigationFinished()
        }

        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation?,
            withError error: Error
        ) {
            handleNavigationError(error)
        }

        func webView(
            _ webView: WKWebView,
            didFail navigation: WKNavigation?,
            withError error: Error
        ) {
            handleNavigationError(error)
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            onNavigationFailed()
            let now = Int64((Date().timeIntervalSince1970 * 1_000).rounded())
            switch crashPolicy.recordCrash(atMilliseconds: now) {
            case .reload:
                errorMessage = nil
                status = "远程电脑页面停止响应，正在自动恢复…"
                webView.reload()
            case .failClosed:
                status = "连接已中断"
                errorMessage = "远程电脑页面连续停止响应，请手动重新连接。"
            }
        }

        func prepareExplicitReload() {
            crashPolicy.resetForExplicitReload()
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = navigationAction.request.url else {
                decisionHandler(.cancel)
                return
            }

            if isAllowedRemoteComputerURL(url) {
                if navigationAction.targetFrame == nil {
                    webView.load(URLRequest(url: url))
                    decisionHandler(.cancel)
                } else {
                    decisionHandler(.allow)
                }
                return
            }

            if navigationAction.targetFrame?.isMainFrame != false {
                status = "已阻止外部导航"
                errorMessage = "远程电脑页面只允许访问 https://fabushi.ombhrum.com。"
                onNavigationFailed()
            }
            decisionHandler(.cancel)
        }

        private func handleNavigationError(_ error: Error) {
            let nsError = error as NSError
            guard nsError.code != NSURLErrorCancelled else { return }
            status = "连接失败"
            errorMessage = nsError.localizedDescription
            onNavigationFailed()
        }

        private func isAllowedRemoteComputerURL(_ url: URL) -> Bool {
            url.scheme?.lowercased() == "https"
                && url.host?.lowercased() == remoteComputerOriginHost
                && url.user == nil
                && url.password == nil
                && (url.port == nil || url.port == 443)
        }
    }
}
