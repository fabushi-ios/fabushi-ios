import XCTest
@testable import Fabushi

private struct TestLocalToolGate: SandLocalToolGate {
    let decision: SandLocalToolDecision

    func authorize(
        scope: SandLocalToolScope?,
        request: SandLocalToolRequest
    ) async -> SandLocalToolDecision {
        decision
    }
}

final class SharedPermissionsAndNotificationsParityTests: XCTestCase {
    func testLocalExecProcessIdentityRequiresExactGenerationArguments() {
        let command = "/opt/fabushi/local-exec --sand-local-exec-generation=g-1 --serve"
        XCTAssertTrue(commandCarriesLocalExecGeneration(
            command,
            entryRealpath: "/opt/fabushi/local-exec",
            generationToken: "g-1"
        ))
        XCTAssertFalse(commandCarriesLocalExecGeneration(
            command.replacingOccurrences(
                of: "--sand-local-exec-generation=g-1",
                with: "--sand-local-exec-generation=g-1-suffix"
            ),
            entryRealpath: "/opt/fabushi/local-exec",
            generationToken: "g-1"
        ))

        let identity = LocalExecProcessIdentity(
            pid: 42,
            startEpochMs: 1_000,
            command: command,
            entryRealpath: "/opt/fabushi/local-exec",
            generationToken: "g-1"
        )
        XCTAssertTrue(sameLocalExecProcessIdentity(identity, identity))
        XCTAssertTrue(localExecDiscoveryTimeMatchesProcess(
            1_020,
            processStartEpochMs: 1_000,
            observedAtMs: 1_030
        ))
        XCTAssertFalse(localExecDiscoveryTimeMatchesProcess(
            70_001,
            processStartEpochMs: 1_000,
            observedAtMs: 70_001
        ))
    }

    func testLocalToolPermissionCeilingAndResourceCoverage() {
        XCTAssertTrue(isSandLocalToolAction("read-file"))
        XCTAssertFalse(isSandLocalToolAction("spawn-daemon"))
        XCTAssertEqual(normalizeSandLocalToolPermission("bogus"), "ask")
        XCTAssertEqual(resolveSandLocalToolPermission("always", adminCeiling: "ask"), "ask")
        XCTAssertEqual(resolveSandLocalToolPermission("never", adminCeiling: "always"), "never")

        XCTAssertEqual(
            sandTerminalFilePath("/tmp/terminals/", shellId: "shell-1"),
            "/tmp/terminals/shell-1.txt"
        )
        XCTAssertTrue(isTerminalFile("/tmp/terminals/shell-1.txt", terminalsFolder: "/tmp/terminals"))
        XCTAssertFalse(isTerminalFile("/tmp/terminals/nested/shell-1.txt", terminalsFolder: "/tmp/terminals"))

        let approval = SandLocalToolApproval(
            action: "run-command",
            target: "swift test",
            resourcePath: "/tmp/terminals"
        )
        let attachedRead = SandLocalToolRequest(
            action: "read-file",
            target: "/tmp/terminals/shell-1.txt",
            attachToResourcePath: "/tmp/terminals"
        )
        XCTAssertTrue(localToolApprovalCovers(approval, request: attachedRead))
    }

    func testDescribeLocalExecMapsOnlyKnownCapabilityMessages() {
        let shell = describeLocalExec(
            .init(message: .init(
                caseName: "backgroundShellSpawnArgs",
                value: .init(command: "swift test", isBackground: true)
            )),
            terminalsFolder: "/tmp/terminals"
        )
        XCTAssertEqual(shell?.action, "run-command")
        XCTAssertEqual(shell?.target, "swift test")
        XCTAssertEqual(shell?.outlivesScope, true)

        XCTAssertNil(describeLocalExec(
            .init(message: .init(caseName: "spawnArbitraryProcess", value: .init())),
            terminalsFolder: "/tmp/terminals"
        ))
    }

    func testLocalToolAuthorizationFailsClosed() async {
        let denied = TestLocalToolGate(decision: .init(allowed: false, reason: "denied"))
        do {
            _ = try await authorizeLocalToolAction(
                gate: denied,
                scope: .init(agentId: "agent-1"),
                request: .init(action: "read-file", target: "/tmp/example")
            )
            XCTFail("denied local action must throw")
        } catch let error as SandLocalToolPermissionDeniedError {
            XCTAssertEqual(error.reason, "denied")
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testIOSLocalExecRoutesOnlyAllowlistedDeviceCapabilitiesLocally() async {
        let supervisor = IOSLocalExecSupervisor()

        let clipboard = await supervisor.route(capabilityName: "clipboardRead")
        let openURL = await supervisor.route(capabilityName: "openExternalURL")
        let share = await supervisor.route(capabilityName: "shareItem")
        let shell = await supervisor.route(capabilityName: "shell.exec")
        let process = await supervisor.route(capabilityName: "process.spawn")
        let box = await supervisor.route(capabilityName: "box.read")
        let arbitrary = await supervisor.route(capabilityName: "spawnArbitraryProcess")

        XCTAssertEqual(clipboard, .local(.clipboardRead))
        XCTAssertEqual(openURL, .local(.openExternalURL))
        XCTAssertEqual(share, .unavailable("shareItem"))
        XCTAssertEqual(shell, .remote)
        XCTAssertEqual(process, .remote)
        XCTAssertEqual(box, .remote)
        XCTAssertEqual(arbitrary, .unavailable("spawnArbitraryProcess"))
    }

    func testMcpCustomInstructionSelectionIsBoundedDeduplicatedAndStable() {
        XCTAssertEqual(clampMcpCustomInstruction(String(repeating: "x", count: 700)).count, 500)
        XCTAssertFalse(getDefaultMcpCustomInstruction(" Hex ").isEmpty)

        let entries = selectConnectedMcpCustomInstructions(
            ["zeta", "hex", "zeta"],
            instructionsByServer: ["zeta": "  keep raw values  "]
        )
        XCTAssertEqual(entries.map(\.name), ["hex", "zeta"])
        XCTAssertEqual(entries.last?.instructions, "keep raw values")

        let section = buildMcpCustomInstructionsSystemPromptSection(
            ["zeta"],
            instructionsByServer: ["zeta": "use the API"]
        )
        XCTAssertTrue(section?.contains("- zeta: use the API") == true)
    }

    func testOAuthCallbackPageEscapesUntrustedConnectorName() {
        let success = renderMcpOAuthSuccessPage(serverName: "<Hex & Co>")
        XCTAssertTrue(success.contains("&lt;Hex &amp; Co&gt; connected"))
        XCTAssertFalse(success.contains("<Hex & Co> connected"))

        let failure = renderMcpOAuthErrorPage(serverName: "GitHub")
        XCTAssertTrue(failure.contains("GitHub — Authentication failed"))
        XCTAssertTrue(failure.contains("OAuth callback failed."))
    }

    func testNotificationDeciderNeedsNewMessageAndRespectsFocusAndThrottle() {
        let baseline = NotificationSnapshot(
            id: "agent-1",
            name: "Builder",
            isRunning: true,
            awaitingReason: nil,
            notifyEnabled: true,
            isHiddenFromSidebar: false,
            lastMessageId: "m1",
            lastMessagePreview: "Working"
        )
        let done = NotificationSnapshot(
            id: "agent-1",
            name: "Builder",
            isRunning: false,
            awaitingReason: nil,
            notifyEnabled: true,
            isHiddenFromSidebar: false,
            lastMessageId: "m2",
            lastMessagePreview: "Finished"
        )

        let decider = SandOsNotificationDecider()
        decider.seedBaseline([baseline])
        let first = decider.decide(agents: [done], isWindowFocused: false, nowMs: 10_000)
        XCTAssertEqual(first.map(\.kind), [.agentDone])
        XCTAssertEqual(buildNotificationContent(first[0]).body, "Finished")

        XCTAssertTrue(decider.decide(
            agents: [done],
            isWindowFocused: false,
            nowMs: 11_000
        ).isEmpty)

        let focusedDecider = SandOsNotificationDecider()
        focusedDecider.seedBaseline([baseline])
        XCTAssertTrue(focusedDecider.decide(
            agents: [done],
            isWindowFocused: true,
            nowMs: 20_000
        ).isEmpty)
    }

    func testIosWebauthnAvailabilityUsesNativeSignerPlatforms() {
        XCTAssertTrue(sandWebauthnSignerShips("ios"))
        XCTAssertTrue(sandWebauthnSignerShips("iPadOS"))
        XCTAssertFalse(sandWebauthnSignerShips("win32"))
        XCTAssertTrue(sandWebauthnProxyMirroredEnablement(true, platform: "ios"))
        XCTAssertFalse(sandWebauthnProxyMirroredEnablement(false, platform: "ios"))
    }

    func testWindowChromeKeepsPureCompatibilityMathWithoutEmulatingDesktopChrome() {
        XCTAssertTrue(IOS_USES_NATIVE_SCENE_CHROME)
        XCTAssertEqual(blendRgbaOverHex((255, 255, 255), alpha: 0.5, backgroundHex: "#000000"), "#808080")
        XCTAssertEqual(titleBarOverlaySymbolColor("#000000"), "#FFFFFF")
        XCTAssertEqual(titleBarOverlaySymbolColor("#FFFFFF"), "#000000")
        XCTAssertEqual(windowsTitleBarOverlayHeight(true), 43)
        XCTAssertEqual(windowsTitleBarOverlayHeight(false), 51)
    }
    func testAPNsEntitlementFollowsBuildConfiguration() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let entitlements = try String(
            contentsOf: iosRoot.appendingPathComponent("Fabushi/Fabushi.entitlements"),
            encoding: .utf8
        )
        let project = try String(
            contentsOf: iosRoot.appendingPathComponent("project.yml"),
            encoding: .utf8
        )

        XCTAssertTrue(entitlements.contains("<string>$(APS_ENVIRONMENT)</string>"))
        XCTAssertFalse(entitlements.contains("<string>development</string>"))
        XCTAssertTrue(project.contains("APS_ENVIRONMENT: development"))
        XCTAssertTrue(project.contains("APS_ENVIRONMENT: production"))
    }

    @MainActor
    func testHumanCallVoIPRegistrationIsStrictAndLogoutRevokesEndpoint() {
        let defaults = UserDefaults(suiteName: "fabushi-human-call-voip-test")!
        defaults.removePersistentDomain(forName: "fabushi-human-call-voip-test")
        defer { defaults.removePersistentDomain(forName: "fabushi-human-call-voip-test") }

        defaults.set(String(repeating: "AB", count: 16), forKey: HumanCallSystemCoordinator.voIPTokenDefaultsKey)
        XCTAssertEqual(
            FabushiRemoteDeviceGateway.currentVoIPToken(defaults: defaults),
            String(repeating: "ab", count: 16)
        )

        defaults.set(String(repeating: "a", count: 33), forKey: HumanCallSystemCoordinator.voIPTokenDefaultsKey)
        XCTAssertNil(FabushiRemoteDeviceGateway.currentVoIPToken(defaults: defaults))

        defaults.set("not-hex-token", forKey: HumanCallSystemCoordinator.voIPTokenDefaultsKey)
        XCTAssertNil(FabushiRemoteDeviceGateway.currentVoIPToken(defaults: defaults))

        XCTAssertEqual(
            FabushiRemoteDeviceGateway.logoutRegistrationMessage(),
            ["type": "unregister", "reason": "logout"]
        )
    }


    func testPushKitTokenLifecycleUsesShippingGatewayOwner() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let runtime = try String(
            contentsOf: iosRoot
                .appendingPathComponent("../../source/ios-main/FabushiRuntime.swift")
                .standardizedFileURL,
            encoding: .utf8
        )
        let gateway = try String(
            contentsOf: iosRoot.appendingPathComponent("Fabushi/FabushiRemoteDeviceGateway.swift"),
            encoding: .utf8
        )
        let coordinator = try String(
            contentsOf: iosRoot.appendingPathComponent("Fabushi/HumanCallSystemCoordinator.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(runtime.contains("let remoteDeviceGateway: FabushiRemoteDeviceGateway"))
        XCTAssertTrue(runtime.contains("bindVoIPTokenChangeHandler"))
        XCTAssertTrue(runtime.contains("remoteDeviceGateway.voIPTokenDidChange()"))
        XCTAssertTrue(gateway.contains("func voIPTokenDidChange() async"))
        XCTAssertTrue(gateway.contains("await refreshConnection()"))
        XCTAssertTrue(coordinator.contains("await self?.voIPTokenChangeHandler?()"))
    }


}
