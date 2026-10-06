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

struct RemoteComputerStatusDeadline: Sendable {
    let timeoutNanoseconds: UInt64

    init(timeoutNanoseconds: UInt64 = 15_000_000_000) {
        self.timeoutNanoseconds = timeoutNanoseconds
    }

    func run(
        _ request: Task<RemoteComputerAgentBoxSnapshot?, Error>
    ) async throws -> RemoteComputerAgentBoxSnapshot? {
        try await withThrowingTaskGroup(
            of: RemoteComputerAgentBoxSnapshot?.self
        ) { group in
            group.addTask {
                try await request.value
            }
            group.addTask {
                try await Task.sleep(nanoseconds: timeoutNanoseconds)
                throw RemoteComputerStatusDeadlineExceeded()
            }
            guard let first = try await group.next() else {
                throw CancellationError()
            }
            group.cancelAll()
            return first
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
        var pendingEnsure: Task<Void, Never>?
        var isEnsureStarting = false
        var watchers = 0
        var lastAccess = 0

        init(agentID: String) {
            self.agentID = agentID
        }

        var snapshot: RemoteComputerStatusStoreSnapshot {
            .init(
                status: status,
                readState: status == nil
                    ? settledReadState ?? .unknown
                    : .known,
                isEnsureStarting: isEnsureStarting
            )
        }
    }

    private let source: any RemoteComputerStatusStoreSourcing
    private let deadline: RemoteComputerStatusDeadline
    private var records: [String: Record] = [:]
    private var demanded: Set<String> = []
    private var actionListeners: [UUID: (Any) -> Void] = [:]
    private var connected = false
    private var disposed = false
    private var ensureGeneration = 0
    private var accessClock = 0

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

    func mostRecentStatus() -> RemoteComputerAgentBoxSnapshot? {
        records.values
            .filter { $0.status != nil }
            .max { $0.lastAccess < $1.lastAccess }?
            .status
    }

    @discardableResult
    func retain(_ agentID: String) -> () -> Void {
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
        let request = Task { @MainActor in
            try await source.read(agentID: agentID)
        }
        let deadline = deadline
        let pending = Task { @MainActor [weak self, weak record] in
            guard let self, let record else { return }
            do {
                let status = try await deadline.run(request)
                guard !self.disposed, record.readAttempt == attempt else { return }
                if let status {
                    self.cache(status)
                } else {
                    record.settledReadState = .known
                    self.publish(record)
                }
            } catch is RemoteComputerStatusDeadlineExceeded {
                guard !self.disposed, record.readAttempt == attempt else { return }
                record.settledReadState = .error
                self.publish(record)
                Task { @MainActor [weak self, weak record] in
                    guard let self, let record else { return }
                    if let late = try? await request.value,
                       let late,
                       !self.disposed,
                       record.readAttempt == attempt
                    {
                        self.cache(late)
                    }
                }
            } catch {
                guard !self.disposed, record.readAttempt == attempt else { return }
                record.settledReadState = .error
                self.publish(record)
            }

            if record.pendingRead?.isCancelled == false {
                record.pendingRead = nil
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
        let request = Task { @MainActor in
            try await source.ensure(agentID: agentID)
        }
        let deadline = deadline
        let pending = Task { @MainActor [weak self, weak record] in
            guard let self, let record else { return }
            defer {
                if record.pendingEnsure?.isCancelled == false {
                    record.pendingEnsure = nil
                    record.isEnsureStarting = false
                    if !self.disposed {
                        self.publish(record)
                    }
                }
            }

            do {
                if let status = try await deadline.run(request),
                   !self.disposed,
                   generation == self.ensureGeneration
                {
                    self.cache(status)
                }
            } catch is RemoteComputerStatusDeadlineExceeded {
                guard !self.disposed, generation == self.ensureGeneration else { return }
                if record.status == nil {
                    record.settledReadState = .error
                    self.publish(record)
                }
                Task { @MainActor [weak self] in
                    guard let self,
                          let late = try? await request.value,
                          let late,
                          !self.disposed,
                          generation == self.ensureGeneration
                    else { return }
                    self.cache(late)
                }
            } catch {
                guard !self.disposed, generation == self.ensureGeneration else { return }
                if record.status == nil {
                    record.settledReadState = .error
                    self.publish(record)
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
        let record = recordFor(agentID)
        guard record.status == nil else { return }
        record.settledReadState = .error
        publish(record)
    }

    func ingest(status: RemoteComputerAgentBoxSnapshot) {
        guard !disposed else { return }
        let record = recordFor(status.agentID)
        record.readAttempt &+= 1
        record.pendingRead?.cancel()
        record.pendingRead = nil
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
    ) -> () -> Void {
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
            record.pendingRead?.cancel()
            record.pendingRead = nil
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
            record.pendingRead?.cancel()
            record.pendingRead = nil
            record.pendingEnsure?.cancel()
            record.pendingEnsure = nil
            record.status = nil
            record.settledReadState = nil
            record.isEnsureStarting = false
        }
        records = records.filter { $0.value.watchers > 0 }
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
            record.pendingRead?.cancel()
            record.pendingEnsure?.cancel()
            record.pendingRead = nil
            record.pendingEnsure = nil
        }
        actionListeners.removeAll()
    }

    private func recordFor(_ agentID: String) -> Record {
        if let existing = records[agentID] {
            touch(existing)
            return existing
        }
        let record = Record(agentID: agentID)
        touch(record)
        records[agentID] = record
        evictRecordsIfNeeded()
        return record
    }

    private func cache(_ status: RemoteComputerAgentBoxSnapshot) {
        let record = recordFor(status.agentID)
        record.status = status
        record.settledReadState = nil
        touch(record)
        publish(record)
    }

    private func touch(_ record: Record) {
        accessClock &+= 1
        record.lastAccess = accessClock
    }

    private func publish(_ record: Record) {
        touch(record)
        version &+= 1
    }

    private func evictRecordsIfNeeded() {
        guard records.count > Self.recordLimit else { return }
        let candidates = records.values
            .filter {
                $0.watchers == 0
                    && $0.pendingRead == nil
                    && $0.pendingEnsure == nil
            }
            .sorted { $0.lastAccess < $1.lastAccess }
        for record in candidates {
            records.removeValue(forKey: record.agentID)
            if records.count <= Self.recordLimit {
                return
            }
        }
    }
}
