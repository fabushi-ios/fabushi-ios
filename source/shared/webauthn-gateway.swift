import Foundation

let GATEWAY_WEBAUTHN_REQUESTS_PATH = "/webauthn/requests"
let GATEWAY_WEBAUTHN_RESPONSES_PATH = "/webauthn/responses"
let SAND_WEBAUTHN_HEARTBEAT_INTERVAL_MS: Int64 = 10_000
let SAND_WEBAUTHN_LIVENESS_WINDOW_MS: Int64 = 30_000
let SAND_WEBAUTHN_CEREMONY_TIMEOUT_MS: Int64 = 120_000
let SAND_NO_WEBAUTHN_MACHINE_MESSAGE = "Your computer isn't connected right now, so the security key can't be reached. Open Fabushi on the machine your key is plugged into and try again."
let SAND_WEBAUTHN_MACHINE_UNAVAILABLE_MESSAGE = "Your computer looks disconnected, so the security key can't be reached. Reconnect it and try again."

enum SandWebAuthnOriginClass: String, Equatable, Sendable {
    case cursorCom = "cursor_com"
    case subdomain
    case external
}

func sandWebAuthnOriginClass(_ origin: String) -> SandWebAuthnOriginClass {
    guard let host = URL(string: origin)?.host?.lowercased() else { return .external }
    if host == "cursor.com" { return .cursorCom }
    return host.hasSuffix(".cursor.com") ? .subdomain : .external
}

struct WebAuthnCeremony: Equatable, Sendable {
    let kind: String
    let origin: String
    var payload: [String: String] = [:]
}

enum WebAuthnRequestFrame: Equatable, Sendable {
    case welcome(providerId: String)
    case ceremony(requestId: String, ceremony: WebAuthnCeremony)
    case cancel(requestId: String)
}

enum WebAuthnResponseFrame: Equatable, Sendable {
    case hello(computerId: String?, label: String?)
    case ping
    case stage(requestId: String, stage: String, outcome: String)
    case result(requestId: String, credentialJson: String)
    case error(requestId: String, name: String, message: String, code: String?)
}
