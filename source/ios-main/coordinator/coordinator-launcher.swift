import Foundation

@MainActor
final class IOSCoordinatorLaunchHandle {
    let rendererClient: CoordinatorControlPortClient
    let rendererServer: RendererPortServer
    let clientPort: InProcessCoordinatorPort
    let serverPort: InProcessCoordinatorPort

    init(main: IOSMainRuntime) throws {
        let pair = InProcessCoordinatorPort.makePair(
            bootstrap: try main.humanScopedCoordinatorBootstrap()
        )
        let server = main.makeRendererPortServer(port: pair.server)
        let client = CoordinatorControlPortClient(port: pair.client, autoStart: false)

        pair.server.onFrame = { [weak server] frame in server?.receive(frame) }
        pair.server.onClose = { [weak server] in server?.portClosed() }
        pair.client.onFrame = { [weak client] frame in client?.receive(frame) }
        pair.client.onClose = { [weak client] in client?.portClosed() }

        rendererClient = client
        rendererServer = server
        clientPort = pair.client
        serverPort = pair.server
        client.start()
    }

    func dispose() {
        rendererClient.shutdown()
    }
}

enum IOSCoordinatorLauncher {
    @MainActor
    static func launch(main: IOSMainRuntime) throws -> IOSCoordinatorLaunchHandle {
        try IOSCoordinatorLaunchHandle(main: main)
    }
}
