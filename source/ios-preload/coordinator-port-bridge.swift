import Foundation

/// iOS renderer-side consumer for the transferred Coordinator port.
///
/// This is the native counterpart of Grok's preload coordinator-port bridge:
/// SwiftUI never receives IOSMainRuntime or MahayanaHostRuntime. Requests,
/// cancellation, events, and lifecycle settlement cross this typed port.
@MainActor
final class IOSCoordinatorPortClient {
    enum Settlement: Equatable, Sendable {
        case shutdownRequested
        case portClosed
        case protocolBreach(String)
    }

    struct PortError: LocalizedError, Equatable, Sendable {
        let code: String
        let message: String
        let transportKind: String?
        let details: [String: CoordinatorPayload]?

        init(
            code: String,
            message: String,
            transportKind: String? = nil,
            details: [String: CoordinatorPayload]? = nil
        ) {
            self.code = code
            self.message = message
            self.transportKind = transportKind
            self.details = details
        }

        var errorDescription: String? { "\(code): \(message)" }
    }

    private let port: InProcessCoordinatorPort
    private let failureTelemetry: IOSCoordinatorFailureTelemetryTracker?
    private let telemetryGeneration: UInt64
    private var pending: [String: CheckedContinuation<CoordinatorPayload, Error>] = [:]
    private var nextRequestID = 0
    private var eventHandler: ((String, CoordinatorPayload) -> Void)?
    private var settlementHandler: ((Settlement) -> Void)?

    private(set) var readyObserved = false
    private(set) var settlement: Settlement?

    init(
        port: InProcessCoordinatorPort,
        failureTelemetry: IOSCoordinatorFailureTelemetryTracker? = nil,
        telemetryGeneration: UInt64 = 0
    ) {
        self.port = port
        self.failureTelemetry = failureTelemetry
        self.telemetryGeneration = telemetryGeneration
        port.onFrame = { [weak self] frame in
            self?.receive(frame)
        }
        port.onClose = { [weak self] in
            self?.settle(.portClosed)
        }
        port.post(.hello(protocolVersion: CoordinatorProtocol.version))
    }

    func request(method: String, args: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        guard settlement == nil else {
            throw PortError(code: "port-settled", message: "coordinator port is unavailable")
        }
        guard readyObserved else {
            throw PortError(code: "port-not-ready", message: "coordinator handshake has not completed")
        }

        nextRequestID += 1
        let requestID = "r-\(nextRequestID)"

        do {
            let value = try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<CoordinatorPayload, Error>) in
                    if Task.isCancelled {
                        continuation.resume(throwing: CancellationError())
                        return
                    }
                    pending[requestID] = continuation
                    port.post(.request(requestId: requestID, method: method, args: args))
                }
            } onCancel: {
                Task { @MainActor [weak self] in
                    self?.cancel(requestID)
                }
            }
            failureTelemetry?.noteSuccess(method: method, generation: telemetryGeneration)
            return value
        } catch let error as PortError {
            failureTelemetry?.noteFailure(
                method: method,
                error: error,
                generation: telemetryGeneration
            )
            throw error
        }
    }

    func setEventHandler(_ handler: ((String, CoordinatorPayload) -> Void)?) {
        eventHandler = handler
    }

    func setSettlementHandler(_ handler: ((Settlement) -> Void)?) {
        settlementHandler = handler
        if let settlement {
            settlementHandler = nil
            handler?(settlement)
        }
    }

    func shutdown() {
        commitSettlement(
            .shutdownRequested,
            peerNotification: .shutdown(reason: .requested, detail: nil)
        )
    }

    private func cancel(_ requestID: String) {
        guard settlement == nil, let continuation = pending.removeValue(forKey: requestID) else { return }
        port.post(.cancel(requestId: requestID))
        continuation.resume(throwing: CancellationError())
    }

    private func receive(_ frame: CoordinatorFrame) {
        guard settlement == nil else { return }

        switch frame {
        case .ready(let version):
            guard !readyObserved else {
                protocolBreach("ready repeated on a live renderer session")
                return
            }
            guard version == CoordinatorProtocol.version else {
                protocolBreach("unsupported coordinator protocol version \(version)")
                return
            }
            readyObserved = true

        case .reply(let requestID, let outcome):
            guard let continuation = pending.removeValue(forKey: requestID) else { return }
            switch outcome {
            case .ok(let value):
                continuation.resume(returning: value)
            case .failed(let failure):
                continuation.resume(throwing: PortError(
                    code: failure.code,
                    message: failure.message,
                    transportKind: failure.transportKind,
                    details: failure.details
                ))
            }

        case .event(let family, let payload):
            eventHandler?(family, payload)

        case .shutdown(let reason, let detail):
            if reason == .requested {
                settle(.shutdownRequested)
            } else {
                settle(.protocolBreach(detail ?? "peer reported protocol breach"))
            }

        case .hello:
            protocolBreach("coordinator posted a renderer-direction hello frame")
        case .request:
            protocolBreach("coordinator posted a renderer-direction request frame")
        case .cancel:
            protocolBreach("coordinator posted a renderer-direction cancel frame")
        }
    }

    private func protocolBreach(_ detail: String) {
        commitSettlement(
            .protocolBreach(detail),
            peerNotification: .shutdown(reason: .protocolError, detail: detail)
        )
    }

    private func settle(_ value: Settlement) {
        commitSettlement(value, peerNotification: nil)
    }

    private func commitSettlement(
        _ value: Settlement,
        peerNotification: CoordinatorFrame?
    ) {
        guard settlement == nil else { return }
        settlement = value
        if let peerNotification {
            port.post(peerNotification)
        }

        let handler = settlementHandler
        settlementHandler = nil
        let error = PortError(code: "port-settled", message: "coordinator port settled before reply")
        for continuation in pending.values {
            continuation.resume(throwing: error)
        }
        pending.removeAll()
        eventHandler = nil
        if !port.isClosed {
            port.close()
        }
        handler?(value)
    }
}
