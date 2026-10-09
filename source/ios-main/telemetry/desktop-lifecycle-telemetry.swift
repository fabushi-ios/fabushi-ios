import Foundation

enum IOSLifecycleTelemetryLevel: String, Equatable, Sendable {
    case info
    case warn
    case error
}

enum IOSLifecycleTelemetryFamily: String, CaseIterable, Equatable, Sendable {
    case startup
    case processRecovery
    case rendererLifecycle
    case coordinatorHandoff
    case localExecLifecycle
    case uncleanExit
    case desktopSession
    case desktopSignin
}

struct IOSLifecycleTelemetryRecord: Equatable, Sendable {
    let family: IOSLifecycleTelemetryFamily
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]
}

@MainActor
final class IOSLifecycleReporter {
    static let preAttachBufferLimit = 100
    private var uploader: (@MainActor (IOSLifecycleTelemetryRecord) -> Void)?
    private var buffered: [IOSLifecycleTelemetryRecord] = []

    func attach(_ uploader: @escaping @MainActor (IOSLifecycleTelemetryRecord) -> Void) {
        self.uploader = uploader
        let pending = buffered
        buffered.removeAll(keepingCapacity: true)
        pending.forEach(uploader)
    }

    func report(
        _ family: IOSLifecycleTelemetryFamily,
        level: IOSLifecycleTelemetryLevel = .info,
        metadata: [String: String] = [:]
    ) {
        let record = IOSLifecycleTelemetryRecord(family: family, level: level, metadata: metadata)
        guard let uploader else {
            if buffered.count >= Self.preAttachBufferLimit { buffered.removeFirst() }
            buffered.append(record)
            return
        }
        uploader(record)
    }

    var bufferedCount: Int { buffered.count }
}
