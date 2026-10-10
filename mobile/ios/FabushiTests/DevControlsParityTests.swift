import XCTest
@testable import Fabushi

@MainActor
private final class DevControlsProductionTestHost: MahayanaHostRequesting {
    private(set) var methods: [String] = []

    func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult {
        methods.append(method)
        return MahayanaHostJSONResult(value: ["method": method])
    }
}

final class DevControlsParityTests: XCTestCase {
    func testDevCapabilityRequiresDebugBuildAndExplicitOptIn() {
        XCTAssertEqual(
            IOSDevCapability.resolve(
                isDebugBuild: true,
                environment: [IOSDevCapability.environmentKey: "1"]
            ).preloadKind,
            .devControls
        )
        XCTAssertEqual(
            IOSDevCapability.resolve(
                isDebugBuild: false,
                environment: [IOSDevCapability.environmentKey: "1"]
            ).preloadKind,
            .primary
        )
        XCTAssertEqual(
            IOSDevCapability.resolve(
                isDebugBuild: true,
                environment: [:]
            ).preloadKind,
            .primary
        )
    }

    func testDevControlsGateFailsClosedWhenDisabled() {
        let gate = IOSDevControlsGate(enabled: false)
        XCTAssertThrowsError(try gate.requireEnabled()) { error in
            XCTAssertEqual(error as? IOSDevControlsGateError, .disabled)
        }
    }

    @MainActor
    func testGatewayOfflineBlocksProductionRequestBeforeLatency() async throws {
        var sleptNanoseconds: UInt64?
        let offline = IOSDevGatewayOfflineControl()
        let latency = IOSDevNetworkLatency { nanoseconds in
            sleptNanoseconds = nanoseconds
        }
        _ = latency.setMilliseconds(25)
        let adapter = IOSNativeDevControlAdapter(
            gate: IOSDevControlsGate(enabled: true),
            gatewayOffline: offline,
            networkLatency: latency
        )

        _ = try await adapter.route(
            method: "dev.setGatewayOffline",
            params: ["induced": true]
        )

        do {
            try await adapter.beforeProductionRequest()
            XCTFail("offline developer control must fail before Host dispatch")
        } catch let error as IOSDevGatewayOfflineError {
            XCTAssertEqual(error, .induced)
        }
        XCTAssertNil(sleptNanoseconds)

        _ = try await adapter.route(
            method: "dev.setGatewayOffline",
            params: ["induced": false]
        )
        try await adapter.beforeProductionRequest()
        XCTAssertEqual(sleptNanoseconds, 25_000_000)
    }

    @MainActor
    func testNetworkLatencyClampsToReferenceMaximum() async throws {
        var sleptNanoseconds: UInt64?
        let latency = IOSDevNetworkLatency { nanoseconds in
            sleptNanoseconds = nanoseconds
        }
        let adapter = IOSNativeDevControlAdapter(
            gate: IOSDevControlsGate(enabled: true),
            networkLatency: latency
        )

        let routed = try await adapter.route(
            method: "dev.setNetworkLatency",
            params: ["ms": 50_000]
        )
        guard case .handled(let value) = routed,
              let object = value as? [String: Any],
              let milliseconds = object["ms"] as? Int else {
            return XCTFail("expected handled latency result")
        }
        XCTAssertEqual(milliseconds, IOSDevNetworkLatency.maximumMilliseconds)

        try await adapter.beforeProductionRequest()
        XCTAssertEqual(sleptNanoseconds, 10_000_000_000)
    }

    @MainActor
    func testCoordinatorProductionRequestAppliesOfflineAndLatencyBeforeHost() async throws {
        var sleptNanoseconds: UInt64?
        let host = DevControlsProductionTestHost()
        let supervisor = MahayanaLocalHostSupervisor(
            host: host,
            factory: { host }
        )
        let offline = IOSDevGatewayOfflineControl()
        let latency = IOSDevNetworkLatency { nanoseconds in
            sleptNanoseconds = nanoseconds
        }
        let adapter = IOSNativeDevControlAdapter(
            gate: IOSDevControlsGate(enabled: true),
            gatewayOffline: offline,
            networkLatency: latency
        )
        let coordinator = MahayanaCoordinator(
            hostSupervisor: supervisor,
            devControlAdapter: adapter
        )

        _ = try await coordinator.request(
            method: "dev.setGatewayOffline",
            params: ["induced": true]
        )
        XCTAssertTrue(host.methods.isEmpty)

        do {
            _ = try await coordinator.request(method: "listAgents")
            XCTFail("offline production request must not reach Host")
        } catch let error as MahayanaCoordinator.CoordinatorError {
            XCTAssertTrue(error.localizedDescription.contains("intentionally offline"))
        }
        XCTAssertTrue(host.methods.isEmpty)
        XCTAssertNil(sleptNanoseconds)

        _ = try await coordinator.request(
            method: "dev.setGatewayOffline",
            params: ["induced": false]
        )
        _ = try await coordinator.request(
            method: "dev.setNetworkLatency",
            params: ["ms": 37]
        )
        let result = try await coordinator.request(method: "listAgents")

        XCTAssertEqual(sleptNanoseconds, 37_000_000)
        XCTAssertEqual(host.methods, ["listAgents"])
        let object = try XCTUnwrap(result.value as? [String: String])
        XCTAssertEqual(object["method"], "listAgents")
    }

    @MainActor
    func testGatewayOfflineStateSurvivesCoordinatorPortRelaunch() async throws {
        let offline = IOSDevGatewayOfflineControl()
        let adapter = IOSNativeDevControlAdapter(
            gate: IOSDevControlsGate(enabled: true),
            gatewayOffline: offline
        )
        _ = try await adapter.route(
            method: "dev.setGatewayOffline",
            params: ["offline": true]
        )
        adapter.coordinatorDidLaunch()
        XCTAssertTrue(offline.isInduced)
        XCTAssertTrue(offline.reapplyAfterCoordinatorLaunch())
    }

    func testPreloadContractKeepsNativeNetworkControlsLocal() {
        XCTAssertEqual(IOSDevControlsContract.disposition(for: "setGatewayOffline"), .local)
        XCTAssertEqual(IOSDevControlsContract.disposition(for: "gatewayOfflineStatus"), .local)
        XCTAssertEqual(IOSDevControlsContract.disposition(for: "setNetworkLatency"), .local)
        XCTAssertEqual(IOSDevControlsContract.disposition(for: "networkLatencyStatus"), .local)
    }
}
