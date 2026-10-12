import XCTest
@testable import Fabushi

final class GatewayTelemetryParityTests: XCTestCase {
    func testReachabilityProjectionPreservesNativeCauseContext() {
        let url = URL(string: "https://gateway.example.test/rpc")!
        let reachable = GatewayTelemetryProjection.reachability(
            url: url,
            outcome: .reachable,
            requestId: "request-1",
            attempt: 0
        )
        XCTAssertEqual(reachable.level, "info")
        XCTAssertEqual(reachable.metadata["outcome"], "reachable")
        XCTAssertEqual(reachable.metadata["base_url_kind"], "remote")
        XCTAssertEqual(reachable.metadata["host_kind"], "hostname")
        XCTAssertEqual(reachable.metadata["secure_transport"], "true")

        let dns = GatewayTelemetryProjection.reachability(
            url: url,
            outcome: .dns,
            requestId: "request-1",
            attempt: 1
        )
        XCTAssertEqual(dns.level, "warn")
        XCTAssertEqual(dns.metadata["outcome"], "dns")
        XCTAssertEqual(dns.metadata["attempt"], "1")
    }

    func testDNSProjectionCarriesEpisodeIdentityWithoutCredentials() {
        let episode = GatewayDNSFailureEpisode(
            host: "gateway.example.test",
            requestId: "request-2",
            episode: 3,
            diagnostic: .init(
                host: "gateway.example.test",
                kind: .hostname,
                isSecureTransport: true
            )
        )
        let projection = GatewayTelemetryProjection.dns(episode)
        XCTAssertEqual(projection.event, "gateway_dns_diagnostic")
        XCTAssertEqual(projection.level, "warn")
        XCTAssertEqual(projection.metadata["episode"], "3")
        XCTAssertEqual(projection.metadata["diagnostic_owner"], "ios_native")
    }

    func testTransportProjectionPreservesStageDurationAndError() {
        let report = CoordinatorTransportStageReport(
            accountSlot: "host",
            clientNonce: "nonce-1",
            stage: "gateway-post",
            attempt: 1,
            traceparent: "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01",
            startEpochMilliseconds: 1000,
            durationMilliseconds: 250,
            isError: true
        )
        let projection = GatewayTelemetryProjection.transport(report)
        XCTAssertEqual(projection.event, "coordinator_transport_stage")
        XCTAssertEqual(projection.level, "warn")
        XCTAssertEqual(projection.metadata["duration_ms"], "250")
        XCTAssertEqual(projection.metadata["is_error"], "true")
    }

    func testShippingRuntimeRoutesGatewayTelemetryIntoDurableLifecycleOwner() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let main = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        let coordinator = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/mahayana-agent-coordinator/MahayanaCoordinator.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(main.contains("gatewayTelemetryReporter: { projection in"))
        XCTAssertTrue(main.contains("reporter.report("))
        XCTAssertTrue(coordinator.contains("telemetrySink: gatewayTelemetryReporter"))
    }
}
