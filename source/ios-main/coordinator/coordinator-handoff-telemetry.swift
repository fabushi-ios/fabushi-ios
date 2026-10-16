import Foundation

struct CoordinatorHandoffTelemetryEvent: Equatable, Sendable {
    enum Phase: String, Sendable {
        case requested
        case adopted
        case invokeFailed = "invoke_failed"
        case timeout
    }

    let phase: Phase
    let leg: String?
    let timeoutMilliseconds: Int?
}

actor CoordinatorHandoffTelemetry {
    private var events: [CoordinatorHandoffTelemetryEvent] = []

    func requested() {
        events.append(.init(phase: .requested, leg: nil, timeoutMilliseconds: nil))
    }

    func adopted(leg: String) {
        events.append(.init(phase: .adopted, leg: leg, timeoutMilliseconds: nil))
    }

    func invokeFailed(leg: String) {
        events.append(.init(phase: .invokeFailed, leg: leg, timeoutMilliseconds: nil))
    }

    func exitTimeout(milliseconds: Int) {
        events.append(.init(phase: .timeout, leg: nil, timeoutMilliseconds: milliseconds))
    }

    func snapshot() -> [CoordinatorHandoffTelemetryEvent] {
        events
    }
}
