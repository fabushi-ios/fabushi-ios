import Foundation

enum CoordinatorCarrierChannel: String, CaseIterable, Equatable, Sendable {
    case control = "coordinator-control"
    case data = "coordinator-data"
    case mainData = "coordinator-main-data"
}

enum CoordinatorCarrierError: Error, Equatable, Sendable {
    case emptyAppVersion
    case emptyDataDirectory
    case missingLocalHumanIdentity
    case unknownChannel(String)
    case closed
}

struct ValidatedCoordinatorBootstrap: Equatable, Sendable {
    let value: CoordinatorBootstrap
}

extension CoordinatorBootstrap {
    func validatedForCarrier() throws -> ValidatedCoordinatorBootstrap {
        guard !processConfig.appVersion.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw CoordinatorCarrierError.emptyAppVersion
        }
        guard !processConfig.dataDir.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw CoordinatorCarrierError.emptyDataDirectory
        }
        return ValidatedCoordinatorBootstrap(value: self)
    }
}

extension ValidatedCoordinatorBootstrap {
    func requiringLocalHumanIdentity() throws -> ValidatedCoordinatorBootstrap {
        guard let localHumanId = value.processConfig.localHumanId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !localHumanId.isEmpty
        else {
            throw CoordinatorCarrierError.missingLocalHumanIdentity
        }
        return self
    }
}

struct CoordinatorCarrierEnvelope: Equatable, Sendable {
    let channel: String
    let frame: CoordinatorFrame

    init(channel: CoordinatorCarrierChannel, frame: CoordinatorFrame) {
        self.channel = channel.rawValue
        self.frame = frame
    }

    init(wireChannel: String, frame: CoordinatorFrame) {
        channel = wireChannel
        self.frame = frame
    }

    var classifiedChannel: CoordinatorCarrierChannel? {
        CoordinatorCarrierChannel(rawValue: channel)
    }
}

struct CoordinatorCarrierMessage: Equatable, Sendable {
    let channel: CoordinatorCarrierChannel
    let frame: CoordinatorFrame
}

@MainActor
final class CoordinatorCarrier {
    let bootstrap: ValidatedCoordinatorBootstrap

    private var queued: [CoordinatorCarrierMessage] = []
    private(set) var isClosed = false

    init(bootstrap: ValidatedCoordinatorBootstrap) {
        self.bootstrap = bootstrap
    }

    var pendingMessageCount: Int { queued.count }
    var firstMessage: CoordinatorCarrierMessage? { queued.first }

    func post(_ channel: CoordinatorCarrierChannel, frame: CoordinatorFrame) throws {
        guard !isClosed else { throw CoordinatorCarrierError.closed }
        queued.append(.init(channel: channel, frame: frame))
    }

    func accept(_ envelope: CoordinatorCarrierEnvelope) throws {
        guard let channel = envelope.classifiedChannel else {
            throw CoordinatorCarrierError.unknownChannel(envelope.channel)
        }
        try post(channel, frame: envelope.frame)
    }

    @discardableResult
    func popFirst() -> CoordinatorCarrierMessage? {
        guard !queued.isEmpty else { return nil }
        return queued.removeFirst()
    }

    func drain() -> [CoordinatorCarrierMessage] {
        defer { queued.removeAll(keepingCapacity: true) }
        return queued
    }

    func close() {
        guard !isClosed else { return }
        isClosed = true
        queued.removeAll()
    }
}

@MainActor
final class InProcessCoordinatorPort: CoordinatorPort {
    weak var peer: InProcessCoordinatorPort?

    var onFrame: ((CoordinatorFrame) -> Void)? {
        didSet { flushPendingFrames() }
    }
    var onDataFrame: ((CoordinatorFrame) -> Void)? {
        didSet { flushPendingFrames() }
    }
    var onMainDataFrame: ((CoordinatorFrame) -> Void)? {
        didSet { flushPendingFrames() }
    }
    var onClose: (() -> Void)?

    let bootstrap: ValidatedCoordinatorBootstrap
    private let carrier: CoordinatorCarrier
    private(set) var isClosed = false

    private init(bootstrap: ValidatedCoordinatorBootstrap) {
        self.bootstrap = bootstrap
        carrier = CoordinatorCarrier(bootstrap: bootstrap)
    }

    var pendingMessageCount: Int { carrier.pendingMessageCount }

    func post(_ frame: CoordinatorFrame) {
        try? post(frame, on: .control)
    }

    func post(_ frame: CoordinatorFrame, on channel: CoordinatorCarrierChannel) throws {
        guard !isClosed else { throw CoordinatorCarrierError.closed }
        guard let peer, !peer.isClosed else { return }
        try peer.receive(.init(channel: channel, frame: frame))
    }

    func acceptEnvelope(_ envelope: CoordinatorCarrierEnvelope) throws {
        try receive(envelope)
    }

    func close() {
        guard !isClosed else { return }
        isClosed = true
        carrier.close()
        onClose?()
        peer?.peerClosed()
    }

    private func receive(_ envelope: CoordinatorCarrierEnvelope) throws {
        guard !isClosed else { throw CoordinatorCarrierError.closed }
        try carrier.accept(envelope)
        flushPendingFrames()
    }

    private func flushPendingFrames() {
        guard !isClosed else { return }
        while let message = carrier.firstMessage {
            let handler: ((CoordinatorFrame) -> Void)?
            switch message.channel {
            case .control:
                handler = onFrame
            case .data:
                handler = onDataFrame
            case .mainData:
                handler = onMainDataFrame
            }
            guard let handler else { return }
            _ = carrier.popFirst()
            handler(message.frame)
            if isClosed { return }
        }
    }

    private func peerClosed() {
        guard !isClosed else { return }
        isClosed = true
        carrier.close()
        onClose?()
    }

    static func makePair(
        bootstrap: ValidatedCoordinatorBootstrap
    ) -> (client: InProcessCoordinatorPort, server: InProcessCoordinatorPort) {
        let client = InProcessCoordinatorPort(bootstrap: bootstrap)
        let server = InProcessCoordinatorPort(bootstrap: bootstrap)
        client.peer = server
        server.peer = client
        return (client, server)
    }
}
