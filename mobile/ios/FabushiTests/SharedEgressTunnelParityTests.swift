import XCTest
@testable import Fabushi

private final class FakeEgressClient: EgressTunnelClient {
    private(set) var starts = 0
    private(set) var stops = 0
    func start() { starts += 1 }
    func stop() { stops += 1 }
}

private actor FakeRemoteEgressRelay: EgressRemoteRelay {
    private(set) var frames: [EgressFrame] = []

    func relay(_ frame: EgressFrame) async throws -> [EgressFrame] {
        frames.append(frame)
        switch frame {
        case .open(let id, _, _):
            return [.data(streamId: id, payload: Data("ok".utf8))]
        default:
            return []
        }
    }

    func count() -> Int { frames.count }
}

final class SharedEgressTunnelParityTests: XCTestCase {
    func testBoxConnectionDerivesDirectAndPodProxyWebSocketUrls() {
        XCTAssertEqual(
            deriveEgressTunnelWsUrl("https://box.example.test/path?q=1", podProxied: false),
            "wss://box.example.test:8790/"
        )
        XCTAssertEqual(
            deriveEgressTunnelWsUrl("https://pod-123.region.example.test/path", podProxied: true),
            "wss://pod-8790.region.example.test/"
        )
        XCTAssertNil(
            deriveEgressTunnelWsUrl("https://pod.region.example.test", podProxied: true)
        )

        XCTAssertEqual(
            boxConnectionToEgressConfig(.init(
                baseUrl: "https://box.example.test",
                token: "secret"
            )),
            .init(
                url: "wss://box.example.test:8790/",
                bearer: "secret",
                headers: nil,
                allowPrivateTargets: false
            )
        )
    }

    func testEnvironmentConfigAndControllerReconcile() {
        let env = [
            "SAND_EGRESS_TUNNEL_URL": "wss://env.example.test/",
            "SAND_EGRESS_TUNNEL_BEARER": "bearer",
            "SAND_EGRESS_TUNNEL_NETWORK_TOKEN": "network",
            "SAND_EGRESS_TUNNEL_ALLOW_PRIVATE": "1",
        ]
        let config = configFromEnv(env)
        XCTAssertEqual(config?.headers?[ANYRUN_NETWORK_TOKEN_HEADER], "network")
        XCTAssertEqual(config?.allowPrivateTargets, true)

        var created: [FakeEgressClient] = []
        var statuses: [EgressTunnelStatus] = []
        let controller = EgressTunnelController(
            env: env,
            onStatus: { statuses.append($0) },
            createClient: { _, _ in
                let client = FakeEgressClient()
                created.append(client)
                return client
            }
        )
        controller.setEnabled(true)
        XCTAssertEqual(created.count, 1)
        XCTAssertEqual(created[0].starts, 1)
        controller.setEnabled(true)
        XCTAssertEqual(created.count, 1)
        controller.setEnabled(false)
        XCTAssertEqual(created[0].stops, 1)
        XCTAssertEqual(controller.getStatus(), OFF_STATUS)
        XCTAssertEqual(statuses.last, OFF_STATUS)
    }

    func testIpPolicyBlocksPrivateLoopbackDocumentationAndMappedAddresses() {
        XCTAssertTrue(isBlockedIp("10.0.0.1"))
        XCTAssertTrue(isBlockedIp("127.0.0.1"))
        XCTAssertTrue(isBlockedIp("192.0.2.1"))
        XCTAssertTrue(isBlockedIp("100.64.0.1"))
        XCTAssertFalse(isBlockedIp("8.8.8.8"))
        XCTAssertTrue(isBlockedIp("::1"))
        XCTAssertTrue(isBlockedIp("fc00::1"))
        XCTAssertTrue(isBlockedIp("::ffff:127.0.0.1"))
        XCTAssertFalse(isBlockedIp("2606:4700:4700::1111"))
        XCTAssertNil(resolveChecked("example.com", allowPrivate: false))
        XCTAssertEqual(resolveChecked("8.8.8.8", allowPrivate: false), "8.8.8.8")
    }

    func testFrameCodecRoundTrips() {
        let open = encodeOpen(7, host: "example.com", port: 443)
        XCTAssertEqual(decodeFrame(open), .open(streamId: 7, host: "example.com", port: 443))
        let data = encodeData(7, payload: Data([1, 2, 3]))
        XCTAssertEqual(decodeFrame(data), .data(streamId: 7, payload: Data([1, 2, 3])))
        XCTAssertEqual(decodeFrame(encodeClose(7)), .close(streamId: 7))
    }

    func testIOSExitClientFailsClosedWithoutRemoteRunner() async {
        var statuses: [EgressTunnelStatus] = []
        let client = EgressTunnelExitClient(remoteRelay: nil) { statuses.append($0) }
        client.start()
        XCTAssertEqual(client.getStatus().state, "remote-runner-required")

        let response = await client.handleFrame(encodeOpen(9, host: "8.8.8.8", port: 443))
        XCTAssertEqual(response, [encodeClose(9)])
        XCTAssertTrue(client.getStatus().lastError?.contains("Remote Runner") == true)
        XCTAssertFalse(statuses.isEmpty)
    }

    func testIOSExitClientDelegatesRelayToRemoteRunner() async {
        let relay = FakeRemoteEgressRelay()
        let client = EgressTunnelExitClient(remoteRelay: relay) { _ in }
        client.start()
        let response = await client.handleFrame(encodeOpen(11, host: "example.com", port: 443))
        XCTAssertEqual(response, [encodeData(11, payload: Data("ok".utf8))])
        let count = await relay.count()
        XCTAssertEqual(count, 1)
        XCTAssertEqual(client.getStatus().relayedStreams, 1)
        XCTAssertEqual(client.getStatus().activeStreams, 1)
    }

    func testWebSocketFactoryRejectsNonWebSocketSchemes() {
        XCTAssertNil(createWebSocket("https://example.test", options: .init(headers: [:])))
        XCTAssertNil(createWebSocket("not a url", options: .init(headers: [:])))
    }
}
