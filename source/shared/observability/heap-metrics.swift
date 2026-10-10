import Foundation

struct HeapMetricsReport: Equatable, Sendable {
    let usedBytes: Double
    let limitBytes: Double
    var loadedAgents: Int? = nil
    var loadedTranscriptEntries: Int? = nil
    var idleMinutesLast15m: Int? = nil
}

private func heapMetricDouble(_ value: Any?) -> Double? {
    if let value = value as? Double { return value.isFinite ? value : nil }
    if let value = value as? Int { return Double(value) }
    if let value = value as? Int64 { return Double(value) }
    if let value = value as? NSNumber {
        let result = value.doubleValue
        return result.isFinite ? result : nil
    }
    return nil
}

private func heapMetricCount(_ value: Any?) -> Int? {
    if let value = value as? Int, value >= 0 { return value }
    if let value = value as? NSNumber {
        let double = value.doubleValue
        guard double.isFinite, double >= 0, double.rounded() == double, double <= Double(Int.max) else { return nil }
        return Int(double)
    }
    return nil
}

func parseHeapMetricsReport(_ value: Any) -> HeapMetricsReport? {
    guard let dictionary = value as? [String: Any],
          let usedBytes = heapMetricDouble(dictionary["usedBytes"]),
          usedBytes >= 0,
          let limitBytes = heapMetricDouble(dictionary["limitBytes"]),
          limitBytes > 0 else { return nil }
    return .init(
        usedBytes: usedBytes == 0 ? 0 : usedBytes,
        limitBytes: limitBytes,
        loadedAgents: heapMetricCount(dictionary["loadedAgents"]),
        loadedTranscriptEntries: heapMetricCount(dictionary["loadedTranscriptEntries"]),
        idleMinutesLast15m: heapMetricCount(dictionary["idleMinutesLast15m"])
    )
}
