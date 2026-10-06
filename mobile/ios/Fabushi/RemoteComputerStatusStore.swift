import Foundation

struct RemoteComputerStatusStoreSnapshot: Equatable, Sendable {
    var status: RemoteComputerAgentBoxSnapshot?
    var readState: RemoteComputerShellReadState
    var isEnsureStarting: Bool

    static let empty = Self(
        status: nil,
        readState: .unknown,
        isEnsureStarting: false
    )
}

@MainActor
protocol RemoteComputerStatusStoreSourcing: AnyObject {
    func read(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot?
    func ensure(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot?
    func handBack(agentID: String, trigger: String) async throws
}

struct RemoteComputerStatusDeadlineExceeded: Error, Equatable {}

@MainActor
private final class RemoteComputerStatusDeadlineGate {
    private var continuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?

    init(_ continuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>) {
        self.continuation = continuation
    }

    func finish(_ result: Result<RemoteComputerAgentBoxSnapshot?, Error>) {
        guard let continuation else { return }
        self.continuation = nil
        continuation.resume(with: result)
    }
}

struct RemoteComputerStatusDeadline: Sendable {
    let timeoutNanoseconds: UInt64

    init(timeoutNanoseconds: UInt64 = 15_000_000_000) {
        self.timeoutNanoseconds = timeoutNanoseconds
    }

    @MainActor
    func run(
        request: Task<RemoteComputerAgentBoxSnapshot?, Error>
    ) async throws -> RemoteComputerAgentBoxSnapshot? {
        try await withCheckedThrowingContinuation { continuation in
            let gate = RemoteComputerStatusDeadlineGate(continuation)
            Task { @MainActor in
                do {
                    gate.finish(.success(try await request.value))
                } catch {
                    gate.finish(.failure(error))
                }
            }
            Task { @MainActor in
                do {
                    try await Task.sleep(nanoseconds: timeoutNanoseconds)
                    gate.finish(.failure(RemoteComputerStatusDeadlineExceeded()))
                } catch {
                    // The source request owns its own lifecycle. A cancelled timer
                    // must never cancel the late source result.
                }
            }
        }
    }
}

@MainActor
final class RemoteComputerStatusStore: ObservableObject {
    static let recordLimit = 32

    @Published private(set) var version = 0
    @Published private(set) var diskPressure: String?
    @Published private(set) var vncUserPresent = false

    private final class Record {
        let agentID: String
        var status: RemoteComputerAgentBoxSnapshot?
        var settledReadState: RemoteComputerShellReadState?
        var readAttempt = 0
        var pendingRead: Task<Void, Never>?
        var pendingReadToken: UUID?
        var pendingEnsure: Task<Void, Never>?
        var pendingEnsureToken: UUID?
        var isEnsureStarting = false
        var watchers = 0

        init(agentID: String) {
            self.agentID = agentID
        }

        var snapshot: RemoteComputerStatusStoreSnapshot {
            .init(
                status: status,
                readState: status == nil ? settledReadState ?? .unknown : .known,
                isEnsureStarting: isEnsureStarting
            )
        }
    }

    private let source: any RemoteComputerStatusStoreSourcing
    private let deadline: RemoteComputerStatusDeadline
    private var records: [String: Record] = [:]
    private var recordOrder: [String] = []
    private var demanded: Set<String> = []
    private var actionListeners: [UUID: (Any) -> Void] = [:]
    private var connected = false
    private var disposed = false
    private var ensureGeneration = 0

    init(
        source: any RemoteComputerStatusStoreSourcing,
        deadline: RemoteComputerStatusDeadline = .init()
    ) {
        self.source = source
        self.deadline = deadline
    }

    func snapshot(for agentID: String?) -> RemoteComputerStatusStoreSnapshot {
        guard let agentID else { return .empty }
        return records[agentID]?.snapshot ?? .empty
    }

    func status(for agentID: String?) -> RemoteComputerAgentBoxSnapshot? {
        guard let agentID else { return nil }
        return records[agentID]?.status
    }

    func readState(for agentID: String?) -> RemoteComputerShellReadState {
        snapshot(for: agentID).readState
    }

    func hasDemanded(_ agentID: String?) -> Bool {
        guard let agentID else { return false }
        return demanded.contains(agentID)
    }

    func mostRecentStatus() -> RemoteComputerAgentBoxSnapshot? {
        for agentID in recordOrder.reversed() {
            if let status = records[agentID]?.status {
                return status
            }
        }
        return nil
    }

    @discardableResult
    func retain(_ agentID: String) -> @MainActor () -> Void {
        let record = recordFor(agentID)
        record.watchers += 1
        if connected {
            refresh(agentID)
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

    func refresh(_ agentID: String) {
        let record = recordFor(agentID)
        guard !disposed, connected, record.pendingRead == nil else { return }

        record.readAttempt &+= 1
        let attempt = record.readAttempt
        if record.status == nil, record.settledReadState == .error {
            record.settledReadState = nil
            publish(record)
        }

        let source = source
        let sourceRequest = Task { @MainActor in
            try await source.read(agentID: agentID)
        }
        let deadline = deadline
        let token = UUID()
        record.pendingReadToken = token
        let pending = Task { @MainActor [weak self, weak record] in
            guard let self, let record else { return }
            let result: Result<RemoteComputerAgentBoxSnapshot?, Error>
            do {
                result = .success(try await deadline.run(request: sourceRequest))
            } catch {
                result = .failure(error)
            }

            if record.pendingReadToken == token {
                record.pendingRead = nil
                record.pendingReadToken = nil
            }
            guard !self.disposed else { return }

            switch result {
            case .success(let status):
                if record.readAttempt == attempt, let status {
                    self.cache(status)
                } else {
                    self.settleRead(record, attempt: attempt, state: .known)
                }
            case .failure(let error):
                self.settleRead(record, attempt: attempt, state: .error)
                if error is RemoteComputerStatusDeadlineExceeded {
                    Task { @MainActor [weak self, weak record] in
                        guard let self, let record else { return }
                        do {
                            if let late = try await sourceRequest.value,
                               !self.disposed,
                               record.readAttempt == attempt
                            {
                                self.cache(late)
                            }
                        } catch {
                            self.settleRead(record, attempt: attempt, state: .error)
                        }
                    }
                }
            }
        }
        record.pendingRead = pending
    }

    func ensure(_ agentID: String) {
        let record = recordFor(agentID)
        guard !disposed else { return }
        demanded.insert(agentID)
        guard record.pendingEnsure == nil else { return }

        let generation = ensureGeneration
        record.isEnsureStarting = true
        publish(record)

        let source = source
        let sourceRequest = Task { @MainActor in
            try await source.ensure(agentID: agentID)
        }
        let deadline = deadline
        let token = UUID()
        record.pendingEnsureToken = token
        let pending = Task { @MainActor [weak self, weak record] in
            guard let self, let record else { return }
            let result: Result<RemoteComputerAgentBoxSnapshot?, Error>
            do {
                result = .success(try await deadline.run(request: sourceRequest))
            } catch {
                result = .failure(error)
            }

            if record.pendingEnsureToken == token {
                record.pendingEnsure = nil
                record.pendingEnsureToken = nil
                record.isEnsureStarting = false
                if !self.disposed {
                    self.publish(record)
                }
            }

            guard !self.disposed, generation == self.ensureGeneration else { return }
            switch result {
            case .success(let status):
                if let status {
                    self.cache(status)
                }
            case .failure(let error):
                self.recordFailure(record)
                if error is RemoteComputerStatusDeadlineExceeded {
                    Task { @MainActor [weak self] in
                        guard let self else { return }
                        if let late = try? await sourceRequest.value,
                           let late,
                           !self.disposed,
                           generation == self.ensureGeneration
                        {
                            self.cache(late)
                        }
                    }
                }
            }
        }
        record.pendingEnsure = pending
    }

    func handBack(_ agentID: String, trigger: String = "button") async {
        guard !disposed else { return }
        try? await source.handBack(agentID: agentID, trigger: trigger)
    }

    func recordReadFailure(_ agentID: String) {
        guard !disposed else { return }
        recordFailure(recordFor(agentID))
    }

    func ingest(status: RemoteComputerAgentBoxSnapshot) {
        guard !disposed else { return }
        let record = recordFor(status.agentID)
        record.readAttempt &+= 1
        record.pendingRead = nil
        record.pendingReadToken = nil
        cache(status)
    }

    func ingestDiskPressure(_ value: String?) {
        guard !disposed else { return }
        if diskPressure != value {
            diskPressure = value
            version &+= 1
        }
    }

    func ingestComputerAction(_ value: Any) {
        guard !disposed else { return }
        for listener in actionListeners.values {
            listener(value)
        }
    }

    func ingestVNCUserPresence(_ isPresent: Bool) {
        guard !disposed else { return }
        if vncUserPresent != isPresent {
            vncUserPresent = isPresent
            version &+= 1
        }
    }

    @discardableResult
    func subscribeComputerActions(
        _ listener: @escaping (Any) -> Void
    ) -> @MainActor () -> Void {
        guard !disposed else { return {} }
        let id = UUID()
        actionListeners[id] = listener
        return { [weak self] in
            self?.actionListeners.removeValue(forKey: id)
        }
    }

    func connect() {
        guard !disposed, !connected else { return }
        connected = true
        for record in records.values where record.watchers > 0 {
            refresh(record.agentID)
        }
    }

    func noteReconnect() {
        guard !disposed, connected else { return }
        for record in records.values where record.watchers > 0 {
            record.readAttempt &+= 1
            record.pendingRead = nil
            record.pendingReadToken = nil
            refresh(record.agentID)
            if demanded.contains(record.agentID) {
                ensure(record.agentID)
            }
        }
    }

    func noteWindowFocus() {
        guard !disposed, connected else { return }
        for record in records.values where record.watchers > 0 {
            refresh(record.agentID)
            if demanded.contains(record.agentID) {
                ensure(record.agentID)
            }
        }
    }

    func reset() {
        guard !disposed else { return }
        connected = false
        ensureGeneration &+= 1
        for record in records.values {
            record.readAttempt &+= 1
            record.pendingRead = nil
            record.pendingReadToken = nil
            record.pendingEnsure = nil
            record.pendingEnsureToken = nil
            record.status = nil
            record.settledReadState = nil
            record.isEnsureStarting = false
            if record.watchers > 0 {
                publish(record)
            }
        }
        for agentID in recordOrder where records[agentID]?.watchers == 0 {
            records.removeValue(forKey: agentID)
        }
        recordOrder.removeAll { records[$0] == nil }
        demanded.removeAll()
        diskPressure = nil
        vncUserPresent = false
        version &+= 1
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        connected = false
        ensureGeneration &+= 1
        for record in records.values {
            record.readAttempt &+= 1
            record.pendingRead = nil
            record.pendingReadToken = nil
            record.pendingEnsure = nil
            record.pendingEnsureToken = nil
        }
        actionListeners.removeAll()
    }

    private func recordFor(_ agentID: String) -> Record {
        if let existing = records[agentID] {
            return existing
        }
        let record = Record(agentID: agentID)
        records[agentID] = record
        recordOrder.append(agentID)
        evictRecordsIfNeeded()
        return record
    }

    private func cache(_ status: RemoteComputerAgentBoxSnapshot) {
        let record = recordFor(status.agentID)
        record.status = status
        record.settledReadState = nil
        recordOrder.removeAll { $0 == record.agentID }
        recordOrder.append(record.agentID)
        publish(record)
    }

    private func settleRead(
        _ record: Record,
        attempt: Int,
        state: RemoteComputerShellReadState
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

    private func publish(_ record: Record) {
        _ = record
        version &+= 1
    }

    private func evictRecordsIfNeeded() {
        guard records.count > Self.recordLimit else { return }
        for agentID in recordOrder {
            guard let record = records[agentID],
                  record.watchers == 0,
                  record.pendingRead == nil,
                  record.pendingEnsure == nil
            else { continue }
            records.removeValue(forKey: agentID)
            recordOrder.removeAll { $0 == agentID }
            if records.count <= Self.recordLimit {
                return
            }
        }
    }
}
