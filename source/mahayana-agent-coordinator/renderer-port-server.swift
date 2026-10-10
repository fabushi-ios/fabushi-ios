import Foundation

@MainActor
final class RendererPortServer {
    enum Settlement: Equatable, Sendable {
        case shutdownRequested
        case portClosed
        case protocolBreach(String)
    }

    enum Phase: Equatable, Sendable {
        case awaitingHello
        case serving
        case settled
    }

    typealias RequestDispatcher = @MainActor (_ method: String, _ args: CoordinatorPayload) async -> CoordinatorReplyOutcome

    private let port: CoordinatorPort
    private let dispatch: RequestDispatcher
    private let onServing: @MainActor () -> Void
    private var inFlight: [String: Task<Void, Never>] = [:]

    private(set) var phase: Phase = .awaitingHello
    private(set) var settlement: Settlement?

    init(
        port: CoordinatorPort,
        dispatch: @escaping RequestDispatcher,
        onServing: @escaping @MainActor () -> Void = {}
    ) {
        self.port = port
        self.dispatch = dispatch
        self.onServing = onServing
    }

    func receive(_ frame: CoordinatorFrame) {
        guard phase != .settled else { return }

        switch frame {
        case .shutdown:
            settle(.shutdownRequested)

        case .reply:
            protocolBreach("renderer posted a server-direction reply")

        case .event:
            protocolBreach("renderer posted a server-direction event")

        case .ready:
            protocolBreach("renderer posted a server-direction ready frame")

        case .hello(let version):
            guard phase == .awaitingHello else {
                protocolBreach("renderer repeated hello on a live session")
                return
            }
            guard version == CoordinatorProtocol.version else {
                protocolBreach("unsupported coordinator protocol version \(version)")
                return
            }
            phase = .serving
            port.post(.ready(protocolVersion: CoordinatorProtocol.version))
            onServing()

        case .request(let requestId, let method, let args):
            guard phase == .serving else {
                protocolBreach("request arrived before hello")
                return
            }
            guard inFlight[requestId] == nil else {
                protocolBreach("requestId \(requestId) reused while in flight")
                return
            }
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                let outcome = await self.dispatch(method, args)
                guard !Task.isCancelled, self.phase == .serving else { return }
                self.inFlight.removeValue(forKey: requestId)
                self.port.post(.reply(requestId: requestId, outcome: outcome))
            }
            inFlight[requestId] = task

        case .cancel(let requestId):
            guard phase == .serving else {
                protocolBreach("cancel arrived before hello")
                return
            }
            guard let task = inFlight.removeValue(forKey: requestId) else { return }
            task.cancel()
            port.post(.reply(
                requestId: requestId,
                outcome: .failed(.init(code: CoordinatorProtocol.cancelled, message: "request cancelled"))
            ))
        }
    }

    func postEvent(family: String, payload: CoordinatorPayload) {
        guard phase == .serving, !family.isEmpty else { return }
        port.post(.event(family: family, payload: payload))
    }

    func portClosed() {
        settle(.portClosed)
    }

    func shutdown() {
        port.post(.shutdown(reason: .requested, detail: nil))
        settle(.shutdownRequested)
    }

    private func protocolBreach(_ detail: String) {
        port.post(.shutdown(reason: .protocolError, detail: detail))
        settle(.protocolBreach(detail))
    }

    private func settle(_ value: Settlement) {
        guard phase != .settled else { return }
        phase = .settled
        settlement = value
        for task in inFlight.values { task.cancel() }
        inFlight.removeAll()
        port.close()
    }
}
