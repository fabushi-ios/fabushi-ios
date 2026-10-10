import Foundation

let SEND_TRACE_SAMPLE_RATIO = 1.0
private let ZERO_TRACE_ID = String(repeating: "0", count: 32)
private let ZERO_SPAN_ID = String(repeating: "0", count: 16)

struct ParsedTraceparent: Equatable, Sendable {
    let traceId: String
    let spanId: String
    let traceFlags: Int
}

private func isLowerHex(_ value: String, count: Int) -> Bool {
    value.count == count && value.allSatisfy { ("0"..."9").contains(String($0)) || ("a"..."f").contains(String($0)) }
}

func parseTraceparent(_ traceparent: String?) -> ParsedTraceparent? {
    guard let traceparent else { return nil }
    let parts = traceparent.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: "-", omittingEmptySubsequences: false).map(String.init)
    guard parts.count == 4 else { return nil }
    let version = parts[0], traceId = parts[1], spanId = parts[2], flags = parts[3]
    guard version == "00",
          isLowerHex(traceId, count: 32), traceId != ZERO_TRACE_ID,
          isLowerHex(spanId, count: 16), spanId != ZERO_SPAN_ID,
          isLowerHex(flags, count: 2),
          let traceFlags = Int(flags, radix: 16) else { return nil }
    return .init(traceId: traceId, spanId: spanId, traceFlags: traceFlags)
}

private func randomHex(_ byteLength: Int) -> String {
    var generator = SystemRandomNumberGenerator()
    return (0..<byteLength).map { _ in
        String(format: "%02x", UInt8.random(in: 0...255, using: &generator))
    }.joined()
}

func shouldSampleSend(ratio: Double = SEND_TRACE_SAMPLE_RATIO, random: () -> Double = { Double.random(in: 0..<1) }) -> Bool {
    guard ratio > 0 else { return false }
    if ratio >= 1 { return true }
    return random() < ratio
}

func mintTraceparent(sampled: Bool = true) -> (traceparent: String, traceId: String, spanId: String) {
    let traceId = randomHex(16)
    let spanId = randomHex(8)
    return ("00-\(traceId)-\(spanId)-\(sampled ? "01" : "00")", traceId, spanId)
}

func deriveChildTraceparent(_ parent: String) -> (traceparent: String, spanId: String)? {
    guard let parsed = parseTraceparent(parent) else { return nil }
    let spanId = randomHex(8)
    let flags = (parsed.traceFlags & 1) == 1 ? "01" : "00"
    return ("00-\(parsed.traceId)-\(spanId)-\(flags)", spanId)
}
