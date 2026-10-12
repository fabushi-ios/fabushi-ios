import Foundation

enum TeamRulesLoadResult<T: Sendable>: Sendable {
    case rules([T])
    case noTeam
    case incomplete
}

actor SandTeamRulesResolver<T: Sendable> {
    typealias Loader = @Sendable () async throws -> TeamRulesLoadResult<T>
    typealias FailureReporter = @Sendable (Error) -> Void

    private let load: Loader
    private let reportLoadFailure: FailureReporter
    private var snapshot: [T]?
    private var inFlight: Task<Void, Never>?

    init(load: @escaping Loader, reportLoadFailure: @escaping FailureReporter) {
        self.load = load
        self.reportLoadFailure = reportLoadFailure
    }

    func start() {
        ensureLoad()
    }

    func refresh() async {
        if let current = inFlight { await current.value }
        ensureLoad()
        if let current = inFlight { await current.value }
    }

    func resolveRules() async -> [T]? {
        if let snapshot { return snapshot }
        ensureLoad()
        if let current = inFlight { await current.value }
        return snapshot
    }

    private func ensureLoad() {
        guard inFlight == nil else { return }
        inFlight = Task { [weak self] in
            guard let self else { return }
            do {
                let result = try await self.load()
                await self.apply(result)
            } catch {
                self.reportLoadFailure(error)
            }
            await self.finishLoad()
        }
    }

    private func apply(_ result: TeamRulesLoadResult<T>) {
        switch result {
        case .rules(let rules):
            snapshot = rules
        case .noTeam:
            snapshot = []
        case .incomplete:
            break
        }
    }

    private func finishLoad() {
        inFlight = nil
    }
}
