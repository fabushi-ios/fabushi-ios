import Foundation

/// Narrow renderer-facing bridge. This is the only runtime surface intended for
/// SwiftUI feature models. Host and Coordinator stay behind IOSMainRuntime.
///
/// Foundation JSON values are converted at this compatibility edge, then every
/// production request crosses the same typed request/reply/cancel port protocol
/// as the Grok renderer/coordinator boundary.
@MainActor
final class IOSPreloadBridge {
    struct JSONResult: @unchecked Sendable {
        let value: Any
    }

    private let server: RendererPortServer
    private let client: IOSCoordinatorPortClient

    init(main: IOSMainRuntime) {
        let pair = InProcessCoordinatorPort.makePair(bootstrap: main.coordinatorBootstrap)
        let server = main.makeRendererPortServer(port: pair.server)

        pair.server.onFrame = { [weak server] frame in
            server?.receive(frame)
        }
        pair.server.onClose = { [weak server] in
            server?.portClosed()
        }

        self.server = server
        client = IOSCoordinatorPortClient(port: pair.client)
    }

    func request(method: String, params: [String: Any] = [:]) async throws -> JSONResult {
        let payload = try CoordinatorPayload.fromFoundation(params)
        let result = try await client.request(method: method, args: payload)
        return JSONResult(value: result.foundationValue)
    }

    func getLinkMetadata(url: String) async throws -> JSONResult {
        try await request(method: "getLinkMetadata", params: ["url": url])
    }

    func listAllAutomations() async throws -> JSONResult {
        try await request(method: "listAllAutomations")
    }

    func shutdown() {
        client.shutdown()
    }
}
