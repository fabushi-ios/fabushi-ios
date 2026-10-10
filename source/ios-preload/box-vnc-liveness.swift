import Foundation

struct IOSVNCLivenessCounters: Codable, Equatable, Sendable {
    let keys: Int64
    let clicks: Int64
    let moves: Int64
    let drawOps: Int64
    let inBytes: Int64

    var isValid: Bool {
        keys >= 0 && clicks >= 0 && moves >= 0 && drawOps >= 0 && inBytes >= 0
    }

    static func parse(json: String) -> IOSVNCLivenessCounters? {
        guard let data = json.data(using: .utf8),
              let value = try? JSONDecoder().decode(IOSVNCLivenessCounters.self, from: data),
              value.isValid
        else { return nil }
        return value
    }
}

struct IOSVNCLivenessReport: Equatable, Sendable {
    let phase = "post_connect"
    let stallMilliseconds: Int64
    let keys: Int64
    let clicks: Int64
    let moves: Int64
    let inBytes: Int64
}

struct IOSVNCLivenessDetector: Sendable {
    static let windowMilliseconds: Int64 = 10_000
    static let minimumImpactfulInputs: Int64 = 3

    private struct Delta: Equatable, Sendable {
        let atMilliseconds: Int64
        let keys: Int64
        let clicks: Int64
        let moves: Int64
        let drawOps: Int64
        let inBytes: Int64
    }

    private var last: IOSVNCLivenessCounters?
    private var samples: [Delta] = []
    private var coveredSinceMilliseconds: Int64?
    private var episodeFired = false

    mutating func reset() {
        last = nil
        samples.removeAll(keepingCapacity: true)
        coveredSinceMilliseconds = nil
        episodeFired = false
    }

    private mutating func rebaseline(nowMilliseconds: Int64, counters: IOSVNCLivenessCounters) {
        reset()
        last = counters
        coveredSinceMilliseconds = nowMilliseconds
    }

    mutating func sample(
        nowMilliseconds: Int64,
        counters: IOSVNCLivenessCounters
    ) -> IOSVNCLivenessReport? {
        guard counters.isValid else {
            reset()
            return nil
        }
        guard let previous = last else {
            rebaseline(nowMilliseconds: nowMilliseconds, counters: counters)
            return nil
        }

        let delta = Delta(
            atMilliseconds: nowMilliseconds,
            keys: counters.keys - previous.keys,
            clicks: counters.clicks - previous.clicks,
            moves: counters.moves - previous.moves,
            drawOps: counters.drawOps - previous.drawOps,
            inBytes: counters.inBytes - previous.inBytes
        )
        guard delta.keys >= 0, delta.clicks >= 0, delta.moves >= 0,
              delta.drawOps >= 0, delta.inBytes >= 0
        else {
            rebaseline(nowMilliseconds: nowMilliseconds, counters: counters)
            return nil
        }

        last = counters
        samples.append(delta)
        let windowStart = nowMilliseconds - Self.windowMilliseconds
        samples.removeAll { $0.atMilliseconds <= windowStart }

        if delta.drawOps > 0 { episodeFired = false }
        if episodeFired { return nil }
        guard let coveredSinceMilliseconds,
              nowMilliseconds - coveredSinceMilliseconds >= Self.windowMilliseconds
        else { return nil }

        let keys = samples.reduce(Int64(0)) { $0 + $1.keys }
        let clicks = samples.reduce(Int64(0)) { $0 + $1.clicks }
        let moves = samples.reduce(Int64(0)) { $0 + $1.moves }
        let drawOps = samples.reduce(Int64(0)) { $0 + $1.drawOps }
        let inBytes = samples.reduce(Int64(0)) { $0 + $1.inBytes }

        guard keys + clicks >= Self.minimumImpactfulInputs else { return nil }
        guard drawOps == 0, inBytes == 0 else { return nil }

        episodeFired = true
        let oldestInput = samples.first { $0.keys + $0.clicks > 0 }
        return IOSVNCLivenessReport(
            stallMilliseconds: nowMilliseconds - (oldestInput?.atMilliseconds ?? nowMilliseconds),
            keys: keys,
            clicks: clicks,
            moves: moves,
            inBytes: inBytes
        )
    }
}
