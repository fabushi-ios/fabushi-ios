import Foundation

let KIND_OPEN: UInt8 = 1
let KIND_DATA: UInt8 = 2
let KIND_CLOSE: UInt8 = 3
let SOCKET_HIGH_WATER_MARK = 1 << 20
let RECONNECT_INITIAL_DELAY_MS = 2_000
let RECONNECT_MAX_DELAY_MS = 30_000

func isSharedCgnat(_ octets: [Int]) -> Bool {
    octets.count >= 2 && octets[0] == 100 && (octets[1] & 192) == 64
}

func isReservedV4(_ octets: [Int]) -> Bool {
    guard octets.count == 4 else { return true }
    return (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && (octets[1] & 254) == 18)
        || octets[0] >= 240
}

func expandV6(_ ip: String) -> [Int]? {
    var value = ip.split(separator: "%", maxSplits: 1).first.map(String.init)?.lowercased() ?? ""
    var tail: [Int] = []

    if let lastColon = value.lastIndex(of: ":") {
        let possibleV4 = String(value[value.index(after: lastColon)...])
        if possibleV4.contains(".") {
            let bytes = possibleV4.split(separator: ".").compactMap { Int($0) }
            guard bytes.count == 4, bytes.allSatisfy({ (0...255).contains($0) }) else { return nil }
            tail = [(bytes[0] << 8) | bytes[1], (bytes[2] << 8) | bytes[3]]
            value = String(value[...lastColon])
        }
    }

    let halves = value.components(separatedBy: "::")
    guard halves.count <= 2 else { return nil }

    func groups(_ part: String) -> [Int]? {
        if part.isEmpty { return [] }
        var result: [Int] = []
        for token in part.split(separator: ":", omittingEmptySubsequences: true) {
            guard let number = Int(token, radix: 16), (0...0xffff).contains(number) else { return nil }
            result.append(number)
        }
        return result
    }

    guard let head = groups(halves[0]) else { return nil }
    let back: [Int]
    if halves.count == 2 {
        guard let parsed = groups(halves[1]) else { return nil }
        back = parsed
        let padding = 8 - head.count - back.count - tail.count
        guard padding >= 0 else { return nil }
        return head + Array(repeating: 0, count: padding) + back + tail
    }
    back = []
    let all = head + back + tail
    return all.count == 8 ? all : nil
}

func embeddedV4(_ segments: [Int]) -> [Int]? {
    guard segments.count == 8 else { return nil }
    func tail() -> [Int] {
        [segments[6] >> 8, segments[6] & 255, segments[7] >> 8, segments[7] & 255]
    }
    if segments[0..<5].allSatisfy({ $0 == 0 }), segments[5] == 0xffff { return tail() }
    if segments[0] == 100, segments[1] == 65_435, segments[2..<6].allSatisfy({ $0 == 0 }) { return tail() }
    if segments[0..<6].allSatisfy({ $0 == 0 }), !(segments[6] == 0 && segments[7] <= 1) { return tail() }
    return nil
}

func isBlockedV4(_ octets: [Int]) -> Bool {
    guard octets.count == 4, octets.allSatisfy({ (0...255).contains($0) }) else { return true }
    let a = octets[0], b = octets[1]
    let privateIp = a == 10 || (a == 172 && (16...31).contains(b)) || (a == 192 && b == 168)
    return privateIp
        || a == 127
        || (a == 169 && b == 254)
        || octets.allSatisfy({ $0 == 255 })
        || (a == 192 && b == 0 && octets[2] == 2)
        || (a == 198 && b == 51 && octets[2] == 100)
        || (a == 203 && b == 0 && octets[2] == 113)
        || octets.allSatisfy({ $0 == 0 })
        || (224...239).contains(a)
        || isSharedCgnat(octets)
        || isReservedV4(octets)
}

func isBlockedV6(_ ip: String) -> Bool {
    guard let segments = expandV6(ip) else { return true }
    if let embedded = embeddedV4(segments) { return isBlockedV4(embedded) }
    let head = segments[0]
    return (segments[0..<7].allSatisfy({ $0 == 0 }) && segments[7] == 1)
        || segments.allSatisfy({ $0 == 0 })
        || (head & 0xff00) == 0xff00
        || (head & 0xfe00) == 0xfc00
        || (head & 0xffc0) == 0xfe80
}

func isBlockedIp(_ ip: String) -> Bool {
    if ip.contains(":") { return isBlockedV6(ip) }
    let parts = ip.split(separator: ".").compactMap { Int($0) }
    return parts.count == 4 ? isBlockedV4(parts) : true
}

enum EgressFrame: Equatable, Sendable {
    case open(streamId: UInt32, host: String, port: UInt16)
    case data(streamId: UInt32, payload: Data)
    case close(streamId: UInt32)
}

private func readUInt32BE(_ bytes: Data, offset: Int) -> UInt32 {
    bytes[offset..<(offset + 4)].reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
}

func decodeFrame(_ bytes: Data) -> EgressFrame? {
    guard bytes.count >= 5 else { return nil }
    let kind = bytes[0]
    let streamId = readUInt32BE(bytes, offset: 1)
    switch kind {
    case KIND_OPEN:
        guard bytes.count >= 7 else { return nil }
        let port = (UInt16(bytes[5]) << 8) | UInt16(bytes[6])
        let host = String(data: bytes.dropFirst(7), encoding: .utf8) ?? ""
        return .open(streamId: streamId, host: host, port: port)
    case KIND_DATA:
        return .data(streamId: streamId, payload: Data(bytes.dropFirst(5)))
    case KIND_CLOSE:
        return .close(streamId: streamId)
    default:
        return nil
    }
}

private func framePrefix(kind: UInt8, streamId: UInt32) -> Data {
    Data([
        kind,
        UInt8((streamId >> 24) & 0xff),
        UInt8((streamId >> 16) & 0xff),
        UInt8((streamId >> 8) & 0xff),
        UInt8(streamId & 0xff),
    ])
}

func encodeData(_ streamId: UInt32, payload: Data) -> Data {
    var out = framePrefix(kind: KIND_DATA, streamId: streamId)
    out.append(payload)
    return out
}

func encodeClose(_ streamId: UInt32) -> Data {
    framePrefix(kind: KIND_CLOSE, streamId: streamId)
}

func encodeOpen(_ streamId: UInt32, host: String, port: UInt16) -> Data {
    var out = framePrefix(kind: KIND_OPEN, streamId: streamId)
    out.append(UInt8((port >> 8) & 0xff))
    out.append(UInt8(port & 0xff))
    out.append(Data(host.utf8))
    return out
}

/// iOS does not resolve arbitrary hostnames for a local TCP egress proxy.
/// Literal public IPs can be classified locally, while DNS/TCP relay belongs
/// to a Remote Runner implementation.
func resolveChecked(_ host: String, allowPrivate: Bool) -> String? {
    if host.contains(":") || host.split(separator: ".").count == 4 {
        return allowPrivate || !isBlockedIp(host) ? host : nil
    }
    return nil
}

protocol EgressRemoteRelay: Sendable {
    func relay(_ frame: EgressFrame) async throws -> [EgressFrame]
}

final class EgressTunnelExitClient: @unchecked Sendable, EgressTunnelClient {
    private let lock = NSLock()
    private let remoteRelay: (any EgressRemoteRelay)?
    private let onStatus: (EgressTunnelStatus) -> Void
    private var stopped = true
    private var state = "off"
    private var relayedStreams = 0
    private var activeStreams: Set<UInt32> = []
    private var lastError: String?

    init(
        remoteRelay: (any EgressRemoteRelay)?,
        onStatus: @escaping (EgressTunnelStatus) -> Void
    ) {
        self.remoteRelay = remoteRelay
        self.onStatus = onStatus
    }

    func start() {
        lock.lock()
        stopped = false
        state = remoteRelay == nil ? "remote-runner-required" : "connected"
        let status = snapshotLocked()
        lock.unlock()
        onStatus(status)
    }

    func stop() {
        lock.lock()
        stopped = true
        state = "off"
        activeStreams.removeAll()
        let status = snapshotLocked()
        lock.unlock()
        onStatus(status)
    }

    func getStatus() -> EgressTunnelStatus {
        lock.lock()
        defer { lock.unlock() }
        return snapshotLocked()
    }

    func handleFrame(_ bytes: Data) async -> [Data] {
        guard let frame = decodeFrame(bytes) else { return [] }

        guard let remoteRelay else {
            if case .open(let streamId, _, _) = frame {
                let status = lock.withLock {
                    lastError = "iOS local TCP egress is disabled; Remote Runner required"
                    return snapshotLocked()
                }
                onStatus(status)
                return [encodeClose(streamId)]
            }
            return []
        }

        updateStreamAccounting(frame)
        do {
            let responses = try await remoteRelay.relay(frame)
            return responses.map(encodeFrame)
        } catch {
            let status = lock.withLock {
                lastError = String(describing: type(of: error))
                return snapshotLocked()
            }
            onStatus(status)
            if case .open(let streamId, _, _) = frame {
                return [encodeClose(streamId)]
            }
            return []
        }
    }

    private func updateStreamAccounting(_ frame: EgressFrame) {
        lock.lock()
        switch frame {
        case .open(let id, _, _):
            if activeStreams.insert(id).inserted { relayedStreams += 1 }
        case .close(let id):
            activeStreams.remove(id)
        case .data:
            break
        }
        let status = snapshotLocked()
        lock.unlock()
        onStatus(status)
    }

    private func snapshotLocked() -> EgressTunnelStatus {
        .init(
            state: state,
            relayedStreams: relayedStreams,
            activeStreams: activeStreams.count,
            lastError: lastError
        )
    }

    private func encodeFrame(_ frame: EgressFrame) -> Data {
        switch frame {
        case .open(let id, let host, let port): return encodeOpen(id, host: host, port: port)
        case .data(let id, let payload): return encodeData(id, payload: payload)
        case .close(let id): return encodeClose(id)
        }
    }
}
