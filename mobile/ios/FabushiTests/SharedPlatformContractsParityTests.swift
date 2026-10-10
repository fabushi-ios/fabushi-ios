import XCTest
@testable import Fabushi

private struct PlatformContractConnectError: ConnectErrorLike {
    let connectCode: Int
    let connectMetadata: [String: String]
}

private struct PlatformContractSettings: Equatable {
    var mode: String?
}

final class SharedPlatformContractsParityTests: XCTestCase {
    func testAuthBoxRuntimeAndEnvironmentPolicies() {
        XCTAssertEqual(
            cursorAccountSlot(.init(kind: "logged-in", authId: "auth-1", email: "a@example.com")),
            "auth-1"
        )
        XCTAssertNil(cursorAccountSlot(.init(kind: "logged-out", authId: "auth-1")))
        XCTAssertTrue(isSandBoxRuntime("remote"))
        XCTAssertTrue(isSandBoxRuntime("local-docker"))
        XCTAssertFalse(isSandBoxRuntime("desktop"))
        XCTAssertTrue(isValidEnvironmentName("SAFE_NAME_1"))
        XCTAssertFalse(isValidEnvironmentName("1BAD"))
    }

    func testBoxSecretValidationAndRedactionList() {
        XCTAssertNil(validateBoxSecretKey("API_KEY"))
        XCTAssertNotNil(validateBoxSecretKey("PATH"))
        XCTAssertNotNil(validateBoxSecretKey("SAND_SECRET"))
        XCTAssertNotNil(validateBoxSecretKey("cursor_sandbox_token"))
        XCTAssertNil(validateBoxSecrets(["API_KEY": "value"]))

        let env = buildBoxSecretsEnv(["Z_KEY": "z", "A_KEY": "a"])
        XCTAssertEqual(env["A_KEY"], "a")
        XCTAssertEqual(env["Z_KEY"], "z")
        XCTAssertEqual(env[BOX_SECRET_REDACTION_NAMES_ENV_VAR], "A_KEY,Z_KEY")
    }

    func testBoxMigrationAndHandbackDecisions() {
        let parsed = parseSandBoxMigrationOperationId("op-1")
        XCTAssertEqual(parsed?.value, "op-1")
        XCTAssertTrue(isSameSandBoxMigrationOperation(parsed, .init(value: "op-1")))
        XCTAssertFalse(isSameSandBoxMigrationOperation(parsed, nil))

        XCTAssertEqual(decideBoxHandBack(nil, trigger: "dismissed"), .none)
        XCTAssertEqual(
            decideBoxHandBack(.init(requestId: "req-1"), trigger: "dismissed"),
            .resume(requestId: "req-1", trigger: "dismissed", resolution: .dismissed)
        )
        XCTAssertEqual(
            decideBoxHandBack(.init(requestId: "req-1"), trigger: "user"),
            .resume(requestId: "req-1", trigger: "user", resolution: .handedBack)
        )
    }

    func testChannelAndListenerContracts() throws {
        XCTAssertEqual(listenerIntegrationManifest("github")?.displayName, "GitHub")
        XCTAssertNil(listenerIntegrationManifest("discord"))

        let address = try XCTUnwrap(parseChannelAddress(" slack : C123 "))
        XCTAssertEqual(address, .init(platform: "slack", chat: "C123"))
        XCTAssertEqual(formatChannelAddress(address), "slack:C123")
        XCTAssertNil(parseChannelAddress("missing"))
        XCTAssertFalse(hasChannelsToShow(manifests: CONNECTOR_MANIFESTS, connections: [Int]()))
        XCTAssertTrue(hasChannelsToShow(manifests: CONNECTOR_MANIFESTS, connections: [1]))
    }

    func testGatewayWireAndBlockedMessageContracts() {
        XCTAssertEqual(GATEWAY_API_PREFIX, "/api")
        XCTAssertEqual(GATEWAY_AUTH_SCHEME, "Bearer")
        XCTAssertEqual(GATEWAY_LOCAL_EXEC_REQUESTS_PATH, "/local-exec/requests")

        let encoded = encodeSandBoxBlockedMessage(.init(
            reason: "maintenance",
            title: "Temporarily unavailable",
            detail: "Try later"
        ))
        XCTAssertTrue(hasSandBoxBlockedMarker(encoded))

        let nested = NSError(
            domain: "outer",
            code: 1,
            userInfo: [
                NSUnderlyingErrorKey: NSError(
                    domain: "inner",
                    code: 2,
                    userInfo: [NSLocalizedDescriptionKey: encoded]
                ),
            ]
        )
        XCTAssertEqual(findSandBoxBlockedMessage(nested), encoded)
    }

    func testConnectErrorClassificationAndRetryAfter() {
        let rate = PlatformContractConnectError(
            connectCode: CONNECT_CODE_RESOURCE_EXHAUSTED,
            connectMetadata: ["retry-after": "2"]
        )
        XCTAssertTrue(isRateLimitConnectError(rate))
        XCTAssertTrue(isTransientConnectError(rate))
        XCTAssertFalse(isInvalidArgumentConnectError(rate))
        XCTAssertEqual(getConnectRetryAfterMs(rate, nowMs: 0), 2_000)

        let invalid = PlatformContractConnectError(
            connectCode: CONNECT_CODE_INVALID_ARGUMENT,
            connectMetadata: [:]
        )
        XCTAssertTrue(isInvalidArgumentConnectError(invalid))
        XCTAssertFalse(isTransientConnectError(invalid))
    }

    func testInferenceUsageStartsAtZeroForEveryProvider() {
        let usage = emptySandInferenceRouterUsage()
        XCTAssertEqual(usage.schemaVersion, 1)
        XCTAssertEqual(Set(usage.providers.keys), Set(SandInferenceProvider.allCases))
        XCTAssertTrue(usage.providers.values.allSatisfy {
            $0.requests == 0
                && $0.inputTokens == 0
                && $0.outputTokens == 0
                && $0.cacheReadTokens == 0
                && $0.cacheWriteTokens == 0
                && $0.lastUsedAt == nil
        })
    }

    func testFeedbackAndLocalExecBounds() {
        XCTAssertEqual(SAND_FEEDBACK_MESSAGE_MAX_CHARS, 10_000)
        XCTAssertTrue(isValidFeedbackSentryEventID(String(repeating: "a", count: 32)))
        XCTAssertFalse(isValidFeedbackSentryEventID("not-an-event"))
        XCTAssertEqual(describeLocalExecBytes(1024 * 1024), "1.0 MiB")
        XCTAssertEqual(
            maxLocalExecUploadFrameBytes(3),
            4 + 64 * 1024
        )
        XCTAssertTrue(localExecFileTooLargeMessage(
            actualBytes: 2 * 1024 * 1024,
            maxBytes: 1024 * 1024
        ).contains("2.0 MiB"))
    }

    func testInvariantReportsWithoutLeakingCallerMessage() throws {
        var report: SandInvariantReport?
        let dispose = try installInvariantReporter { report = $0 }
        defer { dispose() }

        do {
            try invariant(false, "sensitive-debug-message")
            XCTFail("invariant must throw")
        } catch let violation as SandInvariantViolation {
            XCTAssertEqual(violation.message, STRIPPED_INVARIANT_MESSAGE)
            XCTAssertFalse(violation.message.contains("sensitive-debug-message"))
        }

        XCTAssertEqual(report?.name, "SandInvariantViolation")
    }

    @MainActor
    func testHostSettingsFieldReconcilesMirrorAndRejectsStaleTruth() async {
        var remote = PlatformContractSettings(mode: nil)
        var local: String?

        let port = HostSettingsPort<PlatformContractSettings, String>(
            isReadable: { true },
            read: { remote },
            write: { value in
                remote.mode = value
                return remote
            },
            value: { $0.mode }
        )
        let mirror = HostSettingsMirror<String>(
            read: { local },
            write: { local = $0 },
            clear: { local = nil }
        )
        let field = BoxSettingsField(port: port, mirror: mirror)

        local = "stale"
        let cleared = await field.absorbFromBox()
        XCTAssertEqual(cleared, .cleared)
        XCTAssertNil(local)

        let persisted = await field.apply("dark")
        XCTAssertEqual(persisted, .persisted("dark"))
        XCTAssertEqual(local, "dark")
        XCTAssertEqual(remote.mode, "dark")
        let reconciled = await field.reconcile()
        XCTAssertEqual(reconciled, "dark")

        remote.mode = "light"
        let repainted = await field.absorbFromBox()
        XCTAssertEqual(repainted, .repainted)
        XCTAssertEqual(local, "light")
    }

    func testStaticPolicyConstantsRemainCanonical() {
        XCTAssertEqual(DEFAULT_SAND_BOX_RUNTIME, .remote)
        XCTAssertEqual(SAND_DISABLED_NOTIFICATION_CONFIG.minIntervalMs, 5_000)
        XCTAssertEqual(SAND_DISABLED_NOTIFICATION_CONFIG.maxPerWindow, 10)
        XCTAssertEqual(LOCAL_EXEC_DAEMON_DISCOVERY_FILENAME, "local-exec-daemon.json")
        XCTAssertEqual(SAND_LOCAL_EXEC_SUPERVISED_WINDOW_MS, 90_000)
    }

    func testSharedRPCRegistriesAndWireVersionsRemainPinned() {
        XCTAssertTrue(CoordinatorMainMethodRegistry.contains("getHostSettings"))
        XCTAssertTrue(CoordinatorMainMethodRegistry.contains("setGatewayPaused"))
        XCTAssertFalse(CoordinatorMainMethodRegistry.contains("unknownMethod"))

        XCTAssertTrue(CoordinatorMethodRegistry.contains("sendPrompt"))
        XCTAssertTrue(CoordinatorMethodRegistry.contains("executeRoutedMcpTool"))
        XCTAssertTrue(CoordinatorMethodRegistry.contains("runAgentWorkflowNow"))
        XCTAssertFalse(CoordinatorMethodRegistry.contains("unknownMethod"))

        XCTAssertEqual(SharedRPCContracts.coordinatorProtocolVersion, 1)
        XCTAssertEqual(SharedRPCContracts.clientSideToolV2WireVersion, 1)
        XCTAssertEqual(CoordinatorProtocol.unknownMethod, "unknown-method")
        XCTAssertEqual(CoordinatorProtocol.cancelled, "cancelled")
        XCTAssertEqual(CoordinatorProtocol.transportStateFamily, "coordinator-transport-state")
        XCTAssertEqual(ClientSideToolV2Transport.family, "client-side-tool-v2")
        XCTAssertEqual(ClientSideToolV2Transport.accountSlot, "host")
    }

    func testRemoteComputerRPCPayloadsPreserveTypedWireShape() throws {
        XCTAssertEqual(
            RemoteComputerMethod.allCases.map(\.rawValue),
            ["readClipboard", "writeClipboard", "reportUserPresence"]
        )

        let encoder = JSONEncoder()
        let decoder = JSONDecoder()

        let clipboard = RemoteClipboardWriteRequest(text: "hello\nclipboard")
        XCTAssertEqual(
            try decoder.decode(RemoteClipboardWriteRequest.self, from: encoder.encode(clipboard)),
            clipboard
        )

        let presence = RemoteUserPresenceRequest(active: true, timestampMilliseconds: 1_725_840_000_123)
        XCTAssertEqual(
            try decoder.decode(RemoteUserPresenceRequest.self, from: encoder.encode(presence)),
            presence
        )

        let wire = ClientSideToolV2WireMessage(messageType: "tool.result", bytes: Data([0, 1, 2, 255]))
        XCTAssertEqual(wire.encoding, "protobuf-base64")
        XCTAssertEqual(wire.decodedBytes, Data([0, 1, 2, 255]))
    }
}
