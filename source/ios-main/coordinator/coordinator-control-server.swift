import Foundation

@MainActor
final class CoordinatorControlServer {
    enum Settlement: Equatable, Sendable {
        case clean(String)
        case protocolBreach(String)
    }

    static let allowedEventFamilies: Set<String> = [
        "transport-connected", "transport-down", "agents-event", "agents-roster-seed"
    ]

    private let port: CoordinatorPort
    private let executors: CoordinatorControlExecutors
    private let onEvent: @MainActor (String, CoordinatorPayload) -> Void
    private let onProblem: @MainActor (String) -> Void

    private var helloObserved = false
    private var inFlight: [String: Task<Void, Never>] = [:]
    private(set) var settlement: Settlement?

    init(
        port: CoordinatorPort,
        executors: CoordinatorControlExecutors,
        onEvent: @escaping @MainActor (String, CoordinatorPayload) -> Void,
        onProblem: @escaping @MainActor (String) -> Void
    ) {
        self.port = port
        self.executors = executors
        self.onEvent = onEvent
        self.onProblem = onProblem
    }

    func receive(_ frame: CoordinatorFrame) {
        guard settlement == nil else { return }
        switch frame {
        case .hello(let version):
            guard !helloObserved else { return breach("coordinator repeated hello on the control port") }
            guard version == CoordinatorProtocol.version else { return breach("unsupported coordinator protocol version \(version)") }
            helloObserved = true
            port.post(.ready(protocolVersion: CoordinatorProtocol.version))

        case .shutdown:
            settle(.clean("shutdown"))

        case .ready:
            breach("coordinator posted a main-direction ready frame")

        case .reply:
            breach("coordinator posted a main-direction reply frame")

        case .request(let requestId, let method, let args):
            guard helloObserved else { return breach("coordinator posted request before hello") }
            guard inFlight[requestId] == nil else { return breach("control requestId reused while in flight") }
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                let outcome = await self.executors.execute(method: method, args: args)
                guard !Task.isCancelled, self.settlement == nil else { return }
                self.inFlight.removeValue(forKey: requestId)
                self.port.post(.reply(requestId: requestId, outcome: outcome))
            }
            inFlight[requestId] = task

        case .cancel(let requestId):
            guard let task = inFlight.removeValue(forKey: requestId) else { return }
            task.cancel()
            port.post(.reply(
                requestId: requestId,
                outcome: .failed(.init(code: CoordinatorProtocol.cancelled, message: "request cancelled"))
            ))

        case .event(let family, let payload):
            guard helloObserved else { return breach("coordinator posted event before hello") }
            guard Self.allowedEventFamilies.contains(family) else {
                onProblem("coordinator emitted undeclared control event family: \(family)")
                return
            }
            onEvent(family, payload)
        }
    }

    func portClosed() {
        settle(.clean("port-closed"))
    }

    func dispose() {
        guard settlement == nil else { return }
        port.post(.shutdown(reason: .requested, detail: nil))
        settle(.clean("disposed"))
    }

    private func breach(_ detail: String) {
        onProblem(detail)
        port.post(.shutdown(reason: .protocolError, detail: detail))
        settle(.protocolBreach(detail))
    }

    private func settle(_ value: Settlement) {
        guard settlement == nil else { return }
        settlement = value
        inFlight.values.forEach { $0.cancel() }
        inFlight.removeAll()
        port.close()
    }
}
