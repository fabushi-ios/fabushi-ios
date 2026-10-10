import Foundation

/// Narrow renderer-facing bridge. This is the only runtime surface intended for
/// SwiftUI feature models. Host and Coordinator stay behind IOSMainRuntime.
///
/// Foundation JSON values are converted at this compatibility edge, then every
/// production request crosses the same typed request/reply/cancel port protocol
/// as the Grok renderer/coordinator boundary.
enum IOSFeatureEventBrokerError: LocalizedError, Equatable {
    case timedOut

    var errorDescription: String? {
        switch self {
        case .timedOut:
            return "Timed out waiting for a FeatureHost event"
        }
    }
}

@MainActor
final class IOSFeatureEventBroker {
    typealias Receiver = @MainActor (_ timeoutMilliseconds: Int) async throws -> CoordinatorPayload?
    typealias Predicate = ([String: Any]) -> Bool

    private struct Waiter {
        let id: UUID
        let expiresAt: Date
        let predicate: Predicate
        let continuation: CheckedContinuation<CoordinatorPayload, Error>
    }

    private let receive: Receiver
    private let receiveTimeoutMilliseconds: Int
    private let bufferLimit: Int
    private var waiters: [UUID: Waiter] = [:]
    private var waiterOrder: [UUID] = []
    private var buffered: [CoordinatorPayload] = []
    private var pumpTask: Task<Void, Never>?
    private var disposed = false

    init(
        receiveTimeoutMilliseconds: Int = 80,
        bufferLimit: Int = 256,
        receive: @escaping Receiver
    ) {
        self.receiveTimeoutMilliseconds = max(1, receiveTimeoutMilliseconds)
        self.bufferLimit = max(1, bufferLimit)
        self.receive = receive
    }

    func next(
        deadlineMilliseconds: Int,
        matching predicate: @escaping Predicate
    ) async throws -> CoordinatorPayload {
        guard !disposed else { throw CancellationError() }
        guard deadlineMilliseconds > 0 else {
            throw IOSFeatureEventBrokerError.timedOut
        }

        if let bufferedEvent = takeBuffered(matching: predicate) {
            return bufferedEvent
        }

        let id = UUID()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                if Task.isCancelled || disposed {
                    continuation.resume(throwing: CancellationError())
                    return
                }
                waiters[id] = Waiter(
                    id: id,
                    expiresAt: Date().addingTimeInterval(
                        Double(deadlineMilliseconds) / 1_000
                    ),
                    predicate: predicate,
                    continuation: continuation
                )
                waiterOrder.append(id)
                startPumpIfNeeded()
            }
        } onCancel: {
            Task { @MainActor [weak self] in
                self?.cancelWaiter(id)
            }
        }
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        pumpTask?.cancel()
        pumpTask = nil
        buffered.removeAll()
        failAll(with: CancellationError())
    }

    private func startPumpIfNeeded() {
        guard pumpTask == nil, !disposed, !waiters.isEmpty else { return }
        pumpTask = Task { @MainActor [weak self] in
            await self?.runPump()
        }
    }

    private func runPump() async {
        defer {
            pumpTask = nil
            if !disposed, !waiters.isEmpty {
                startPumpIfNeeded()
            }
        }

        while !disposed, !waiters.isEmpty, !Task.isCancelled {
            expireWaiters()
            guard !waiters.isEmpty else { break }
            do {
                if let event = try await receive(receiveTimeoutMilliseconds) {
                    route(event)
                }
            } catch is CancellationError {
                if Task.isCancelled || disposed { break }
            } catch {
                failAll(with: error)
                break
            }
        }
    }

    private func route(_ event: CoordinatorPayload) {
        expireWaiters()
        guard let foundationEvent = event.foundationValue as? [String: Any] else { return }
        for id in waiterOrder {
            guard let waiter = waiters[id], waiter.predicate(foundationEvent) else { continue }
            removeWaiter(id)
            waiter.continuation.resume(returning: event)
            return
        }

        buffered.append(event)
        if buffered.count > bufferLimit {
            buffered.removeFirst(buffered.count - bufferLimit)
        }
    }

    private func takeBuffered(matching predicate: Predicate) -> CoordinatorPayload? {
        guard let index = buffered.firstIndex(where: { event in
            guard let foundationEvent = event.foundationValue as? [String: Any] else { return false }
            return predicate(foundationEvent)
        }) else { return nil }
        return buffered.remove(at: index)
    }

    private func expireWaiters() {
        let now = Date()
        let expired = waiterOrder.filter {
            guard let waiter = waiters[$0] else { return false }
            return waiter.expiresAt <= now
        }
        for id in expired {
            guard let waiter = waiters[id] else { continue }
            removeWaiter(id)
            waiter.continuation.resume(throwing: IOSFeatureEventBrokerError.timedOut)
        }
    }

    private func cancelWaiter(_ id: UUID) {
        guard let waiter = waiters[id] else { return }
        removeWaiter(id)
        waiter.continuation.resume(throwing: CancellationError())
    }

    private func removeWaiter(_ id: UUID) {
        waiters.removeValue(forKey: id)
        waiterOrder.removeAll { $0 == id }
    }

    private func failAll(with error: Error) {
        let current = waiterOrder.compactMap { waiters[$0] }
        waiters.removeAll()
        waiterOrder.removeAll()
        for waiter in current {
            waiter.continuation.resume(throwing: error)
        }
    }
}

@MainActor
final class IOSPreloadBridge {
    struct JSONResult: @unchecked Sendable {
        let value: Any
    }

    typealias RendererEventObserver = @MainActor (_ family: String, _ payload: CoordinatorPayload) -> Void

    private let server: RendererPortServer
    private let client: IOSCoordinatorPortClient
    private var featureEventBroker: IOSFeatureEventBroker?
    private var rendererEventObservers: [UUID: RendererEventObserver] = [:]
    private var bufferedRendererEvents: [(String, CoordinatorPayload)] = []
    private let rendererEventBufferLimit = 256

    init(main: IOSMainRuntime) {
        let pair = InProcessCoordinatorPort.makePair(bootstrap: main.coordinatorBootstrap)
        let server = main.makeRendererPortServer(port: pair.server)

        pair.server.onFrame = { [weak server] frame in
            server?.receive(frame)
        }
        pair.server.onClose = { [weak server] in
            server?.portClosed()
        }

        self.server = server
        client = IOSCoordinatorPortClient(port: pair.client)
        client.setEventHandler { [weak self] family, payload in
            self?.routeRendererEvent(family: family, payload: payload)
        }
        featureEventBroker = IOSFeatureEventBroker { [weak self] timeoutMilliseconds in
            guard let self else { throw CancellationError() }
            let result = try await self.requestPayload(
                method: "feature.receive",
                params: ["timeoutMs": timeoutMilliseconds]
            )
            guard case .object = result else { return nil }
            return result
        }
    }

    private func requestPayload(
        method: String,
        params: [String: Any] = [:]
    ) async throws -> CoordinatorPayload {
        let payload = try CoordinatorPayload.fromFoundation(params)
        return try await client.request(method: method, args: payload)
    }

    func request(method: String, params: [String: Any] = [:]) async throws -> JSONResult {
        let result = try await requestPayload(method: method, params: params)
        return JSONResult(value: result.foundationValue)
    }

    func getLinkMetadata(url: String) async throws -> JSONResult {
        try await request(method: "getLinkMetadata", params: ["url": url])
    }

    func listAllAutomations() async throws -> JSONResult {
        try await request(method: "listAllAutomations")
    }

    func receiveFeatureEvent(
        deadlineMilliseconds: Int,
        matching predicate: @escaping IOSFeatureEventBroker.Predicate
    ) async throws -> JSONResult {
        guard let featureEventBroker else { throw CancellationError() }
        let event = try await featureEventBroker.next(
            deadlineMilliseconds: deadlineMilliseconds,
            matching: predicate
        )
        return JSONResult(value: event.foundationValue)
    }

    func addRendererEventObserver(_ observer: @escaping RendererEventObserver) -> UUID {
        let id = UUID()
        rendererEventObservers[id] = observer
        if !bufferedRendererEvents.isEmpty {
            let buffered = bufferedRendererEvents
            bufferedRendererEvents.removeAll()
            for (family, payload) in buffered {
                observer(family, payload)
            }
        }
        return id
    }

    func removeRendererEventObserver(_ id: UUID) {
        rendererEventObservers.removeValue(forKey: id)
    }

    private func routeRendererEvent(family: String, payload: CoordinatorPayload) {
        if rendererEventObservers.isEmpty {
            bufferedRendererEvents.append((family, payload))
            if bufferedRendererEvents.count > rendererEventBufferLimit {
                bufferedRendererEvents.removeFirst(bufferedRendererEvents.count - rendererEventBufferLimit)
            }
            return
        }
        for observer in rendererEventObservers.values {
            observer(family, payload)
        }
    }

    func shutdown() {
        featureEventBroker?.dispose()
        featureEventBroker = nil
        rendererEventObservers.removeAll()
        bufferedRendererEvents.removeAll()
        client.shutdown()
    }
}
