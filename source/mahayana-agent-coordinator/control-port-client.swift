import Foundation

struct ControlPortCallError: LocalizedError, Equatable, Sendable {
    let code: String
    let message: String

    var errorDescription: String? { "\(code): \(message)" }
}

@MainActor
final class CoordinatorControlPortClient {
    enum Settlement: Equatable, Sendable {
        case shutdownRequested
        case portClosed
        case protocolBreach(String)
    }

    private let port: CoordinatorPort
    private var pending: [String: CheckedContinuation<CoordinatorPayload, Error>] = [:]
    private var nextRequestID = 0
    private var started = false

    private(set) var readyObserved = false
    private(set) var settlement: Settlement?

    init(port: CoordinatorPort, autoStart: Bool = true) {
        self.port = port
        if autoStart { start() }
    }

    func start() {
        guard !started, settlement == nil else { return }
        started = true
        port.post(.hello(protocolVersion: CoordinatorProtocol.version))
    }

    func call(method: String, args: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        guard settlement == nil else {
            throw ControlPortCallError(code: CoordinatorProtocol.disconnected, message: "control port is settled")
        }
        if !started { start() }
        nextRequestID += 1
        let requestId = "c-\(nextRequestID)"
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                pending[requestId] = continuation
                port.post(.request(requestId: requestId, method: method, args: args))
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.cancel(requestId: requestId) }
        }
    }

    func cancel(requestId: String) {
        guard settlement == nil, pending[requestId] != nil else { return }
        port.post(.cancel(requestId: requestId))
    }

    func receive(_ frame: CoordinatorFrame) {
        guard settlement == nil else { return }
        switch frame {
        case .ready(let version):
            guard !readyObserved else {
                protocolBreach("ready repeated on a live control session")
                return
            }
            guard version == CoordinatorProtocol.version else {
                protocolBreach("unsupported coordinator protocol version \(version)")
                return
            }
            readyObserved = true

        case .reply(let requestId, let outcome):
            guard let continuation = pending.removeValue(forKey: requestId) else { return }
            switch outcome {
            case .ok(let value):
                continuation.resume(returning: value)
            case .failed(let failure):
                continuation.resume(throwing: ControlPortCallError(code: failure.code, message: failure.message))
            }

        case .shutdown(let reason, let detail):
            let failure = disconnectedFailure(
                detail ?? "coordinator shutdown: \(shutdownReasonName(reason))"
            )
            if reason == .requested {
                settle(.shutdownRequested, failure: failure)
            } else {
                settle(
                    .protocolBreach(detail ?? "coordinator shutdown: \(shutdownReasonName(reason))"),
                    failure: failure
                )
            }

        case .hello:
            protocolBreach("main posted a client-direction hello frame")
        case .request:
            protocolBreach("main posted a client-direction request frame")
        case .cancel:
            protocolBreach("main posted a client-direction cancel frame")
        case .event:
            protocolBreach("main posted a client-direction event frame")
        }
    }

    func postEvent(family: String, payload: CoordinatorPayload) {
        guard settlement == nil else { return }
        port.post(.event(family: family, payload: payload))
    }

    func portClosed() {
        settle(.portClosed, failure: disconnectedFailure("control port closed"))
    }

    func shutdown() {
        guard settlement == nil else { return }
        port.post(.shutdown(reason: .requested, detail: nil))
        settle(.shutdownRequested, failure: disconnectedFailure("shutdown requested"))
    }

    private func protocolBreach(_ detail: String) {
        port.post(.shutdown(reason: .protocolError, detail: detail))
        settle(.protocolBreach(detail), failure: disconnectedFailure(detail))
    }

    private func disconnectedFailure(_ message: String) -> ControlPortCallError {
        ControlPortCallError(code: CoordinatorProtocol.disconnected, message: message)
    }

    private func shutdownReasonName(_ reason: CoordinatorShutdownReason) -> String {
        switch reason {
        case .requested: "Requested"
        case .protocolError: "ProtocolError"
        }
    }

    private func settle(_ value: Settlement, failure: ControlPortCallError) {
        guard settlement == nil else { return }
        settlement = value
        for continuation in pending.values { continuation.resume(throwing: failure) }
        pending.removeAll()
        port.close()
    }
}
