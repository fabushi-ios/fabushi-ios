import Foundation

struct GatewayDNSDiagnostic: Equatable, Sendable {
    enum HostKind: String, Sendable {
        case loopback
        case hostname
        case ipLiteral
        case missing
    }

    let host: String?
    let kind: HostKind
    let isSecureTransport: Bool
}

struct GatewayDNSFailureEpisode: Equatable, Sendable {
    let host: String
    let requestId: String
    let episode: UInt64
    let diagnostic: GatewayDNSDiagnostic
}

@MainActor
final class GatewayDNSDiagnosticTracker {
    static let throttleSeconds: TimeInterval = 60

    private let now: () -> Date
    private var lastProbeAtByHost: [String: Date] = [:]
    private var episodeByHost: [String: UInt64] = [:]
    private var activeEpisodeHosts = Set<String>()
    private(set) var emitted: [GatewayDNSFailureEpisode] = []

    init(now: @escaping () -> Date = Date.init) {
        self.now = now
    }

    @discardableResult
    func recordFailure(url: URL, requestId: String) -> GatewayDNSFailureEpisode? {
        let diagnostic = GatewayDNSDiagnostics.inspect(url)
        guard let host = diagnostic.host, !host.isEmpty else { return nil }
        let key = host.lowercased()
        let timestamp = now()
        if let last = lastProbeAtByHost[key] {
            guard timestamp.timeIntervalSince(last) >= Self.throttleSeconds else { return nil }
            // A quiet window closes the prior episode even if no successful probe arrived.
            activeEpisodeHosts.remove(key)
        }

        lastProbeAtByHost[key] = timestamp
        activeEpisodeHosts.insert(key)
        let next = (episodeByHost[key] ?? 0) &+ 1
        episodeByHost[key] = next

        let event = GatewayDNSFailureEpisode(
            host: host,
            requestId: requestId,
            episode: next,
            diagnostic: diagnostic
        )
        emitted.append(event)
        return event
    }

    func recordSuccess(url: URL) {
        guard let host = GatewayDNSDiagnostics.inspect(url).host else { return }
        activeEpisodeHosts.remove(host.lowercased())
    }

    func episodeActive(host: String) -> Bool {
        activeEpisodeHosts.contains(host.lowercased())
    }
}

enum GatewayDNSDiagnostics {
    static func inspect(_ url: URL) -> GatewayDNSDiagnostic {
        guard let host = url.host, !host.isEmpty else {
            return .init(host: nil, kind: .missing, isSecureTransport: url.scheme == "https")
        }
        let lower = host.lowercased()
        let kind: GatewayDNSDiagnostic.HostKind
        if lower == "localhost" || lower == "127.0.0.1" || lower == "::1" {
            kind = .loopback
        } else if lower.allSatisfy({ $0.isNumber || $0 == "." || $0 == ":" }) {
            kind = .ipLiteral
        } else {
            kind = .hostname
        }
        return .init(host: host, kind: kind, isSecureTransport: url.scheme == "https")
    }
}
