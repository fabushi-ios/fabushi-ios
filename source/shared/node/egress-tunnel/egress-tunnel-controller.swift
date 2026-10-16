import Foundation

struct EgressTunnelStatus: Equatable, Sendable {
    let state: String
    let relayedStreams: Int
    let activeStreams: Int
    let lastError: String?

    init(
        state: String,
        relayedStreams: Int,
        activeStreams: Int,
        lastError: String? = nil
    ) {
        self.state = state
        self.relayedStreams = relayedStreams
        self.activeStreams = activeStreams
        self.lastError = lastError
    }
}

let OFF_STATUS = EgressTunnelStatus(state: "off", relayedStreams: 0, activeStreams: 0)
let ANYRUN_NETWORK_TOKEN_HEADER = "x-anyrun-network-token"

protocol EgressTunnelClient: AnyObject {
    func start()
    func stop()
}

func configsEqual(_ a: EgressTunnelConfig?, _ b: EgressTunnelConfig?) -> Bool {
    a == b
}

func configFromEnv(_ env: [String: String] = ProcessInfo.processInfo.environment) -> EgressTunnelConfig? {
    guard let url = env["SAND_EGRESS_TUNNEL_URL"], !url.isEmpty,
          let bearer = env["SAND_EGRESS_TUNNEL_BEARER"], !bearer.isEmpty else {
        return nil
    }
    let token = env["SAND_EGRESS_TUNNEL_NETWORK_TOKEN"]
    return .init(
        url: url,
        bearer: bearer,
        headers: token.map { $0.isEmpty ? [:] : [ANYRUN_NETWORK_TOKEN_HEADER: $0] }.flatMap { $0.isEmpty ? nil : $0 },
        allowPrivateTargets: env["SAND_EGRESS_TUNNEL_ALLOW_PRIVATE"] == "1"
    )
}

final class EgressTunnelController {
    typealias ClientFactory = (
        _ config: EgressTunnelConfig,
        _ onStatus: @escaping (EgressTunnelStatus) -> Void
    ) -> any EgressTunnelClient

    private var enabled = false
    private var boxConfig: EgressTunnelConfig?
    private var activeConfig: EgressTunnelConfig?
    private var client: (any EgressTunnelClient)?
    private(set) var lastStatus = OFF_STATUS

    private let env: [String: String]
    private let onStatus: (EgressTunnelStatus) -> Void
    private let createClient: ClientFactory

    init(
        env: [String: String] = ProcessInfo.processInfo.environment,
        onStatus: @escaping (EgressTunnelStatus) -> Void,
        createClient: @escaping ClientFactory
    ) {
        self.env = env
        self.onStatus = onStatus
        self.createClient = createClient
    }

    func setEnabled(_ enabled: Bool) {
        guard self.enabled != enabled else { return }
        self.enabled = enabled
        reconcile()
    }

    func setBoxConnection(_ config: EgressTunnelConfig?) {
        boxConfig = config
        reconcile()
    }

    func getStatus() -> EgressTunnelStatus {
        lastStatus
    }

    func dispose() {
        enabled = false
        reconcile()
    }

    private func resolveConfig() -> EgressTunnelConfig? {
        configFromEnv(env) ?? boxConfig
    }

    private func reconcile() {
        let desired = enabled ? resolveConfig() : nil
        guard let desired else {
            client?.stop()
            client = nil
            activeConfig = nil
            publish(OFF_STATUS)
            return
        }
        if client != nil, configsEqual(desired, activeConfig) { return }

        client?.stop()
        activeConfig = desired
        let created = createClient(desired) { [weak self] status in
            self?.publish(status)
        }
        client = created
        created.start()
    }

    private func publish(_ status: EgressTunnelStatus) {
        lastStatus = status
        onStatus(status)
    }
}
