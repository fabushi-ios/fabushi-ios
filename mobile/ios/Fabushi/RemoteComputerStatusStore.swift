import Combine
import Foundation

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

    var isAgentScope: Bool {
        agentID?.isEmpty == false
    }
}

struct RemoteComputerAgentBoxDiskPressureSnapshot: Equatable, Sendable {
    let level: String
    let availableBytes: Int64?
    let totalBytes: Int64?
}

struct RemoteComputerAgentBoxAction: Equatable, Sendable {
    let agentID: String?
    let kind: String
    let x: Double?
    let y: Double?
}

struct RemoteComputerAgentBoxSnapshot: Equatable, Sendable {
    let agentID: String
    let state: String
    let vncURL: URL?
    let imageUpdateAvailable: Bool
    let diskPressure: RemoteComputerAgentBoxDiskPressureSnapshot?

    init(
        agentID: String,
        state: String,
        vncURL: URL?,
        imageUpdateAvailable: Bool,
        diskPressure: RemoteComputerAgentBoxDiskPressureSnapshot? = nil
    ) {
        self.agentID = agentID
        self.state = state
        self.vncURL = vncURL
        self.imageUpdateAvailable = imageUpdateAvailable
        self.diskPressure = diskPressure
    }

    var isReadyForVNC: Bool {
        state == "running" && vncURL != nil
    }
}

enum RemoteComputerAgentBoxReadState: String, Equatable, Sendable {
    case unknown
    case known
    case error
}

@MainActor
protocol RemoteComputerAgentBoxSourcing: AnyObject {
    func status(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot?
    func ensure(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot
    func release(agentID: String, trigger: String) async throws
}

@MainActor
final class IOSRemoteComputerAgentBoxSource: RemoteComputerAgentBoxSourcing {
    enum SourceError: LocalizedError {
        case bridgeUnavailable
        case invalidResponse
        case agentScopeMismatch
        case unsafeVNCURL
        case backend(status: Int, message: String)

        var errorDescription: String? {
            switch self {
            case .bridgeUnavailable:
                return "Agent 电脑 Host bridge 不可用。"
            case .invalidResponse:
                return "Agent 电脑控制面返回了无效响应。"
            case .agentScopeMismatch:
                return "Agent 电脑响应与当前 Agent scope 不一致。"
            case .unsafeVNCURL:
                return "Agent 电脑控制面没有返回可验证的 HTTPS VNC 地址。"
            case .backend(let status, let message):
                let detail = message.trimmingCharacters(in: .whitespacesAndNewlines)
                return detail.isEmpty
                    ? "Agent 电脑控制面请求失败（HTTP \(status)）。"
                    : String(detail.prefix(240))
            }
        }
    }

    static let statusMethod = "getForeverBoxStatus"
    static let ensureMethod = "ensureForeverBox"
    static let releaseMethod = "handBackForeverBox"

    private let bridge: IOSPreloadBridge?

    init(bridge: IOSPreloadBridge?) {
        self.bridge = bridge
    }

    func status(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot? {
        guard let payload = try await hostAgentBoxRequest(
            method: Self.statusMethod,
            params: ["id": agentID]
        ) else {
            return nil
        }
        return try Self.projectStatus(payload, expectedAgentID: agentID)
    }

    func ensure(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot {
        guard let payload = try await hostAgentBoxRequest(
            method: Self.ensureMethod,
            params: ["id": agentID]
        ) else {
            throw SourceError.invalidResponse
        }
        return try Self.projectStatus(payload, expectedAgentID: agentID)
    }

    func release(agentID: String, trigger: String) async throws {
        _ = try await hostAgentBoxRequest(
            method: Self.releaseMethod,
            params: [
                "id": agentID,
                "trigger": trigger,
            ]
        )
    }

    static func projectStatus(
        _ payload: [String: Any],
        expectedAgentID: String
    ) throws -> RemoteComputerAgentBoxSnapshot {
        let object = (payload["box"] as? [String: Any]) ?? payload
        if let returnedAgentID = object["agentId"] as? String,
           returnedAgentID != expectedAgentID
        {
            throw SourceError.agentScopeMismatch
        }

        guard let state = object["state"] as? String,
              ["running", "starting", "hibernated", "stopped", "local"].contains(state)
        else {
            throw SourceError.invalidResponse
        }

        let rawVNC = (object["vncUrl"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        var vncURL: URL?
        if let rawVNC, !rawVNC.isEmpty {
            guard let candidate = URL(string: rawVNC),
                  candidate.scheme?.lowercased() == "https",
                  candidate.host?.isEmpty == false,
                  candidate.user == nil,
                  candidate.password == nil,
                  (candidate.port == nil || candidate.port == 443)
            else {
                throw SourceError.unsafeVNCURL
            }
            vncURL = candidate
        }

        return .init(
            agentID: expectedAgentID,
            state: state,
            vncURL: vncURL,
            imageUpdateAvailable: object["imageUpdateAvailable"] as? Bool ?? false,
            diskPressure: projectDiskPressure(object["diskPressure"])
        )
    }

    private static func projectDiskPressure(_ value: Any?) -> RemoteComputerAgentBoxDiskPressureSnapshot? {
        if let level = value as? String, !level.isEmpty {
            return .init(level: level, availableBytes: nil, totalBytes: nil)
        }
        guard let object = value as? [String: Any] else { return nil }
        let level = ((object["level"] as? String) ?? (object["status"] as? String) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !level.isEmpty else { return nil }
        return .init(
            level: level,
            availableBytes: int64(object["availableBytes"]),
            totalBytes: int64(object["totalBytes"])
        )
    }

    private static func int64(_ value: Any?) -> Int64? {
        if let value = value as? Int64 { return value }
        if let value = value as? Int { return Int64(value) }
        if let value = value as? NSNumber { return value.int64Value }
        return nil
    }

    private func hostAgentBoxRequest(
        method: String,
        params: [String: Any]
    ) async throws -> [String: Any]? {
        guard let bridge else { throw SourceError.bridgeUnavailable }

        let value = try await bridge.request(
            method: method,
            params: params
        ).value

        guard let envelope = value as? [String: Any],
              let ok = envelope["ok"] as? Bool
        else {
            throw SourceError.invalidResponse
        }
        guard ok else {
            let status = (envelope["statusCode"] as? NSNumber)?.intValue
                ?? envelope["statusCode"] as? Int
                ?? 0
            let message: String
            if let data = envelope["data"] as? [String: Any],
               let candidate = (data["message"] as? String) ?? (data["error"] as? String)
            {
                message = candidate
            } else {
                message = envelope["bodyText"] as? String ?? ""
            }
            throw SourceError.backend(status: status, message: message)
        }

        if envelope["data"] is NSNull || envelope["data"] == nil {
            return nil
        }
        guard let data = envelope["data"] as? [String: Any] else {
            throw SourceError.invalidResponse
        }
        return data
    }
}

private enum RemoteComputerAgentBoxDeadlineError: Error {
    case exceeded
}

@MainActor
final class RemoteComputerAgentBoxOwner: ObservableObject {
    static let recordLimit = 32
    static let defaultStatusTimeoutMilliseconds: UInt64 = 15_000

    @Published private(set) var snapshot: RemoteComputerAgentBoxSnapshot?
    @Published private(set) var readState: RemoteComputerAgentBoxReadState = .unknown
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var reloadRevision = 0
    @Published private(set) var version = 0
    @Published private(set) var diskPressureSnapshot: RemoteComputerAgentBoxDiskPressureSnapshot?
    @Published private(set) var vncUserPresent = false

    private final class Record {
        let agentID: String
        var status: RemoteComputerAgentBoxSnapshot?
        var settledReadState: RemoteComputerAgentBoxReadState?
        var readAttempt: UInt64 = 0
        var isEnsureStarting = false
        var watchers = 0
        var lastTouch: UInt64 = 0

        init(agentID: String) {
            self.agentID = agentID
        }
    }

    private struct PendingRead {
        let attempt: UInt64
        let task: Task<Void, Never>
    }

    private struct PendingEnsure {
        let generation: UInt64
        let task: Task<Void, Never>
    }

    private let source: any RemoteComputerAgentBoxSourcing
    private let statusTimeoutMilliseconds: UInt64
    private var records: [String: Record] = [:]
    private var demanded = Set<String>()
    private var pendingReads: [String: PendingRead] = [:]
    private var pendingEnsures: [String: PendingEnsure] = [:]
    private var computerActionListeners: [UUID: (RemoteComputerAgentBoxAction) -> Void] = [:]
    private var activeScopeKey: String?
    private var activeAgentID: String?
    private var activeRetainedAgentID: String?
    private var connected = false
    private var disposed = false
    private var ensureGeneration: UInt64 = 0
    private var touchSequence: UInt64 = 0

    init(
        source: any RemoteComputerAgentBoxSourcing,
        statusTimeoutMilliseconds: UInt64 = RemoteComputerAgentBoxOwner.defaultStatusTimeoutMilliseconds
    ) {
        self.source = source
        self.statusTimeoutMilliseconds = max(1, statusTimeoutMilliseconds)
    }

    var vncURL: URL? {
        snapshot?.isReadyForVNC == true ? snapshot?.vncURL : nil
    }

    var cachedRecordCount: Int {
        records.count
    }

    func status(for agentID: String?) -> RemoteComputerAgentBoxSnapshot? {
        guard let agentID else { return nil }
        return records[agentID]?.status
    }

    func readState(for agentID: String?) -> RemoteComputerAgentBoxReadState {
        guard let agentID, let record = records[agentID] else { return .unknown }
        return readState(for: record)
    }

    func mostRecentStatus() -> RemoteComputerAgentBoxSnapshot? {
        records.values
            .filter { $0.status != nil }
            .max { $0.lastTouch < $1.lastTouch }?
            .status
    }

    func hasDemanded(_ agentID: String?) -> Bool {
        guard let agentID else { return false }
        return demanded.contains(agentID)
    }

    @discardableResult
    func retain(agentID: String) -> () -> Void {
        let record = recordFor(agentID)
        record.watchers += 1
        touch(record)
        if connected {
            Task { @MainActor [weak self] in
                await self?.refresh(agentID: agentID)
            }
        }
        var released = false
        return { [weak self, weak record] in
            guard !released else { return }
            released = true
            guard let self, let record else { return }
            record.watchers = max(0, record.watchers - 1)
            self.evictRecordsIfNeeded()
        }
    }

    func connect(scope: RemoteComputerScope) async {
        guard !disposed, let agentID = scope.agentID, !agentID.isEmpty else {
            await disconnect(trigger: "scope-cleared")
            return
        }

        if let activeScopeKey, activeScopeKey != scope.scopeKey {
            await invalidateAndRelease(trigger: "scope-changed")
        }

        connected = true
        activeScopeKey = scope.scopeKey
        activeAgentID = agentID
        if activeRetainedAgentID != agentID {
            releaseActiveRetention()
            let record = recordFor(agentID)
            record.watchers += 1
            activeRetainedAgentID = agentID
        }
        publish(recordFor(agentID))

        await ensure(agentID: agentID)

        Task { @MainActor [weak self] in
            await self?.refresh(agentID: agentID)
        }
    }

    func refresh(agentID: String) async {
        guard !disposed, connected else { return }
        if let pending = pendingReads[agentID] {
            await pending.task.value
            return
        }

        let record = recordFor(agentID)
        record.readAttempt &+= 1
        let attempt = record.readAttempt
        if record.status == nil, record.settledReadState == .error {
            record.settledReadState = nil
            publish(record)
        }

        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performRefresh(agentID: agentID, attempt: attempt)
        }
        pendingReads[agentID] = .init(attempt: attempt, task: task)
        await task.value
        if pendingReads[agentID]?.attempt == attempt {
            pendingReads[agentID] = nil
        }
        evictRecordsIfNeeded()
    }

    func ensure(agentID: String) async {
        guard !disposed else { return }
        demanded.insert(agentID)
        if let pending = pendingEnsures[agentID] {
            await pending.task.value
            return
        }

        let record = recordFor(agentID)
        record.isEnsureStarting = true
        publish(record)
        let generation = ensureGeneration
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performEnsure(agentID: agentID, generation: generation)
        }
        pendingEnsures[agentID] = .init(generation: generation, task: task)
        await task.value
        if pendingEnsures[agentID]?.generation == generation {
            pendingEnsures[agentID] = nil
        }
        evictRecordsIfNeeded()
    }

    func handBack(agentID: String, trigger: String = "button") async {
        guard !disposed else { return }
        do {
            try await source.release(agentID: agentID, trigger: trigger)
        } catch {
            if activeAgentID == agentID {
                errorMessage = String(error.localizedDescription.prefix(240))
            }
            return
        }

        demanded.remove(agentID)
        if let record = records[agentID] {
            record.readAttempt &+= 1
            record.status = nil
            record.settledReadState = .known
            record.isEnsureStarting = false
            pendingReads[agentID] = nil
            pendingEnsures[agentID] = nil
            publish(record)
        }
    }

    func recordReadFailure(agentID: String) {
        guard !disposed else { return }
        recordFailure(recordFor(agentID))
    }

    func ingestForeverBox(_ status: RemoteComputerAgentBoxSnapshot) {
        guard !disposed else { return }
        let record = recordFor(status.agentID)
        record.readAttempt &+= 1
        pendingReads[status.agentID] = nil
        cacheStatus(status)
    }

    func ingestBoxDiskPressure(_ value: RemoteComputerAgentBoxDiskPressureSnapshot?) {
        guard !disposed else { return }
        diskPressureSnapshot = value
        version &+= 1
    }

    func subscribeComputerActions(
        _ listener: @escaping (RemoteComputerAgentBoxAction) -> Void
    ) -> () -> Void {
        guard !disposed else { return {} }
        let id = UUID()
        computerActionListeners[id] = listener
        return { [weak self] in
            self?.computerActionListeners[id] = nil
        }
    }

    func ingestComputerAction(_ value: RemoteComputerAgentBoxAction) {
        guard !disposed else { return }
        for listener in computerActionListeners.values {
            listener(value)
        }
    }

    func ingestVncUserPresence(isPresent: Bool) {
        guard !disposed, vncUserPresent != isPresent else { return }
        vncUserPresent = isPresent
        version &+= 1
    }

    func noteReconnect() async {
        guard !disposed, connected, let agentID = activeAgentID else { return }
        reloadRevision &+= 1
        let record = recordFor(agentID)
        record.readAttempt &+= 1
        pendingReads[agentID] = nil

        ensureGeneration &+= 1
        pendingEnsures[agentID] = nil
        record.isEnsureStarting = false
        publish(record)

        Task { @MainActor [weak self] in
            await self?.refresh(agentID: agentID)
        }
        if demanded.contains(agentID) {
            await ensure(agentID: agentID)
        }
    }

    func noteWindowFocus() {
        guard !disposed, connected else { return }
        let watched = records.values.filter { $0.watchers > 0 }.map(\.agentID)
        for agentID in watched {
            Task { @MainActor [weak self] in
                guard let self else { return }
                await self.refresh(agentID: agentID)
                if self.demanded.contains(agentID) {
                    await self.ensure(agentID: agentID)
                }
            }
        }
    }

    func disconnect(trigger: String) async {
        guard activeAgentID != nil || snapshot != nil || isLoading else {
            connected = false
            return
        }
        connected = false
        await invalidateAndRelease(trigger: trigger)
    }

    func reset() {
        guard !disposed else { return }
        connected = false
        ensureGeneration &+= 1

        for record in records.values {
            record.readAttempt &+= 1
            record.status = nil
            record.settledReadState = nil
            record.isEnsureStarting = false
            if record.watchers > 0 {
                publish(record)
            }
        }
        records = records.filter { $0.value.watchers > 0 }
        pendingReads.removeAll()
        pendingEnsures.removeAll()
        demanded.removeAll()
        diskPressureSnapshot = nil
        vncUserPresent = false
        errorMessage = nil
        if let activeAgentID, let record = records[activeAgentID] {
            publish(record)
        } else {
            snapshot = nil
            readState = .unknown
            isLoading = false
        }
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        connected = false
        ensureGeneration &+= 1
        for record in records.values {
            record.readAttempt &+= 1
        }
        pendingReads.removeAll()
        pendingEnsures.removeAll()
        computerActionListeners.removeAll()
        demanded.removeAll()
        releaseActiveRetention()
        activeScopeKey = nil
        activeAgentID = nil
        snapshot = nil
        readState = .unknown
        isLoading = false
        errorMessage = nil
        vncUserPresent = false
    }

    private func performRefresh(agentID: String, attempt: UInt64) async {
        let request = Task { @MainActor [source] in
            try await source.status(agentID: agentID)
        }

        do {
            let next = try await awaitWithDeadline(request)
            guard !disposed, let record = records[agentID], record.readAttempt == attempt else { return }
            if let next {
                cacheStatus(next)
            } else {
                settleRead(record, attempt: attempt, state: .known)
            }
        } catch RemoteComputerAgentBoxDeadlineError.exceeded {
            guard !disposed, let record = records[agentID] else { return }
            settleRead(record, attempt: attempt, state: .error)
            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let late = try await request.value
                    guard !self.disposed,
                          let record = self.records[agentID],
                          record.readAttempt == attempt
                    else { return }
                    if let late {
                        self.cacheStatus(late)
                    } else {
                        self.settleRead(record, attempt: attempt, state: .known)
                    }
                } catch {
                    guard let record = self.records[agentID] else { return }
                    self.settleRead(record, attempt: attempt, state: .error)
                }
            }
        } catch {
            guard !disposed, let record = records[agentID] else { return }
            settleRead(record, attempt: attempt, state: .error)
        }
    }

    private func performEnsure(agentID: String, generation: UInt64) async {
        let request = Task { @MainActor [source] in
            try await source.ensure(agentID: agentID)
        }

        do {
            let next = try await awaitWithDeadline(request)
            guard !disposed, generation == ensureGeneration else { return }
            cacheStatus(next)
            if activeAgentID == agentID {
                errorMessage = next.isReadyForVNC
                    ? nil
                    : "Agent 电脑已响应，但当前没有可用的 HTTPS VNC 会话。"
            }
        } catch RemoteComputerAgentBoxDeadlineError.exceeded {
            guard !disposed, generation == ensureGeneration else { return }
            recordFailure(recordFor(agentID))
            if activeAgentID == agentID {
                errorMessage = "Agent 电脑状态读取超时；后台结果到达后会自动接纳。"
            }
            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let late = try await request.value
                    guard !self.disposed, generation == self.ensureGeneration else { return }
                    self.cacheStatus(late)
                    if self.activeAgentID == agentID {
                        self.errorMessage = late.isReadyForVNC
                            ? nil
                            : "Agent 电脑已响应，但当前没有可用的 HTTPS VNC 会话。"
                    }
                } catch {}
            }
        } catch {
            guard !disposed, generation == ensureGeneration else { return }
            recordFailure(recordFor(agentID))
            if activeAgentID == agentID {
                errorMessage = String(error.localizedDescription.prefix(240))
            }
        }

        if let record = records[agentID], generation == ensureGeneration {
            record.isEnsureStarting = false
            publish(record)
        }
    }

    private func awaitWithDeadline<T: Sendable>(_ task: Task<T, Error>) async throws -> T {
        let timeoutNanoseconds = statusTimeoutMilliseconds * 1_000_000
        return try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask {
                try await task.value
            }
            group.addTask {
                try await Task.sleep(nanoseconds: timeoutNanoseconds)
                throw RemoteComputerAgentBoxDeadlineError.exceeded
            }
            defer { group.cancelAll() }
            guard let value = try await group.next() else {
                throw RemoteComputerAgentBoxDeadlineError.exceeded
            }
            return value
        }
    }

    private func recordFor(_ agentID: String) -> Record {
        if let record = records[agentID] {
            touch(record)
            return record
        }
        let record = Record(agentID: agentID)
        records[agentID] = record
        touch(record)
        evictRecordsIfNeeded()
        return record
    }

    private func touch(_ record: Record) {
        touchSequence &+= 1
        record.lastTouch = touchSequence
    }

    private func readState(for record: Record) -> RemoteComputerAgentBoxReadState {
        record.status != nil ? .known : record.settledReadState ?? .unknown
    }

    private func publish(_ record: Record) {
        touch(record)
        version &+= 1
        guard activeAgentID == record.agentID else { return }
        snapshot = record.status
        readState = readState(for: record)
        isLoading = record.isEnsureStarting
    }

    private func cacheStatus(_ status: RemoteComputerAgentBoxSnapshot) {
        let record = recordFor(status.agentID)
        record.status = status
        record.settledReadState = nil
        if let pressure = status.diskPressure {
            diskPressureSnapshot = pressure
        }
        publish(record)
    }

    private func settleRead(
        _ record: Record,
        attempt: UInt64,
        state: RemoteComputerAgentBoxReadState
    ) {
        guard record.readAttempt == attempt,
              record.status == nil,
              record.settledReadState != state
        else { return }
        record.settledReadState = state
        publish(record)
    }

    private func recordFailure(_ record: Record) {
        guard record.status == nil, record.settledReadState != .error else { return }
        record.settledReadState = .error
        publish(record)
    }

    private func evictRecordsIfNeeded() {
        while records.count > Self.recordLimit {
            let candidates = records.values.filter { record in
                record.watchers == 0
                    && pendingReads[record.agentID] == nil
                    && pendingEnsures[record.agentID] == nil
                    && record.agentID != activeAgentID
            }
            guard let candidate = candidates.min(by: { left, right in
                left.lastTouch < right.lastTouch
            }) else {
                return
            }
            records[candidate.agentID] = nil
        }
    }

    private func releaseActiveRetention() {
        guard let activeRetainedAgentID,
              let record = records[activeRetainedAgentID]
        else {
            self.activeRetainedAgentID = nil
            return
        }
        record.watchers = max(0, record.watchers - 1)
        self.activeRetainedAgentID = nil
        evictRecordsIfNeeded()
    }

    private func invalidateAndRelease(trigger: String) async {
        ensureGeneration &+= 1
        let agentID = activeAgentID
        if let agentID, let record = records[agentID] {
            record.readAttempt &+= 1
            record.isEnsureStarting = false
            pendingReads[agentID] = nil
            pendingEnsures[agentID] = nil
        }

        activeScopeKey = nil
        activeAgentID = nil
        snapshot = nil
        readState = .unknown
        isLoading = false
        errorMessage = nil
        vncUserPresent = false
        releaseActiveRetention()

        guard let agentID else { return }
        try? await source.release(agentID: agentID, trigger: trigger)
        demanded.remove(agentID)
        if let record = records[agentID] {
            record.status = nil
            record.settledReadState = .known
            publish(record)
        }
        evictRecordsIfNeeded()
    }
}
