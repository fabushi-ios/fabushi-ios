import Foundation

struct EgressWebSocketOptions: Equatable, Sendable {
    let headers: [String: String]
}

protocol ExitWebSocket: Sendable {
    func send(_ data: Data) async throws
    func receive() async throws -> Data
    func close()
}

let EGRESS_TUNNEL_OPEN_READY_STATE = 1
