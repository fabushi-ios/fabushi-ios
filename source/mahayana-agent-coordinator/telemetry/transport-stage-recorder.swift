import Foundation

struct CoordinatorTransportStage: Equatable, Sendable {
    let requestId: String
    let stage: String
    let timestampMilliseconds: Int64
}

actor TransportStageRecorder {
    private let capacity: Int
    private var records: [CoordinatorTransportStage] = []

    init(capacity: Int = 512) {
        self.capacity = max(1, capacity)
    }

    func record(requestId: String, stage: String, now: Date = Date()) {
        guard !requestId.isEmpty, !stage.isEmpty else { return }
        records.append(.init(
            requestId: requestId,
            stage: stage,
            timestampMilliseconds: Int64(now.timeIntervalSince1970 * 1_000)
        ))
        if records.count > capacity {
            records.removeFirst(records.count - capacity)
        }
    }

    func snapshot() -> [CoordinatorTransportStage] {
        records
    }

    func reset() {
        records.removeAll(keepingCapacity: true)
    }
}
