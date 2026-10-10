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
