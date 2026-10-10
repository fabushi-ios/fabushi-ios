import XCTest
@testable import Fabushi

@MainActor
private final class SettingsParityHost: MahayanaHostRequesting {
    func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult {
        .init(value: ["method": method])
    }
}

final class SharedSettingsParityTests: XCTestCase {
    private func makeRoot() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    func testLegacyPartialSettingsArePreservedAndNormalized() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let legacy: [String: Any] = [
            "version": 1,
            "themePreference": "dark",
            "webauthnProxyEnabled": false,
            "updateTrackOverride": "nightly",
            "mcpBoxServers": ["alpha", "alpha", ""],
            "mcpCustomInstructionsByServerId": [
                "12": "keep",
                "bad": "drop",
            ],
            "mcpDisabledToolsByServerId": [
                "12": ["search", "search", ""],
                "0": ["drop"],
            ],
            "settingsMigrations": [],
            "agentDefaultModel": [
                "modelId": "claude-opus-4-8",
                "maxMode": false,
                "parameters": [
                    ["id": "fast", "value": "true"],
                    ["id": "thinking", "value": "true"],
                ],
            ],
        ]
        try JSONSerialization.data(withJSONObject: legacy)
            .write(to: path, options: .atomic)

        let store = SandSettingsStore(settingsPath: path.path)
        let loaded = store.load()

        XCTAssertEqual(loaded.themePreference, "dark")
        XCTAssertFalse(loaded.webauthnProxyEnabled)
        XCTAssertEqual(loaded.mcpBoxServers, ["alpha"])
        XCTAssertEqual(loaded.mcpCustomInstructionsByServerId, ["12": "keep"])
        XCTAssertEqual(loaded.mcpDisabledToolsByServerId, ["12": ["search"]])
        XCTAssertEqual(loaded.agentDefaultModel?.maxMode, true)
        XCTAssertEqual(
            loaded.agentDefaultModel?.parameters.first(where: { $0.id == "fast" })?.value,
            "false"
        )
        XCTAssertTrue(loaded.settingsMigrations.contains(SAND_DOWNGRADE_MAX_FAST_MIGRATION_ID))

        XCTAssertEqual(store.getUpdateTrackOverride(), .stable)
        XCTAssertEqual(store.load().updateTrackOverride, "stable")
    }

    func testFabushiIsFreshInferenceDefaultAndExplicitProviderPersists() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)

        XCTAssertEqual(store.getInferenceProvider(), .fabushi)
        store.setInferenceProvider(.openrouter)

        let restored = SandSettingsStore(settingsPath: path.path)
        XCTAssertEqual(restored.getInferenceProvider(), .openrouter)
    }

    func testAccountScopePreservesFirstOwnerThenClearsCrossAccountState() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)

        store.setMcpCustomInstructions(["GitHub": "owner-only"])
        store.setMcpCustomInstructionsByServerId(["12": "owner-only"])
        store.setMcpDisabledToolsByServerId(["12": ["delete"]])
        store.setAgentDefaultModel(.init(
            modelId: "claude-opus-4-8",
            maxMode: false,
            parameters: [.init(id: "thinking", value: "true")]
        ))

        store.scopeToAccount("owner-a")
        XCTAssertEqual(store.getMcpCustomInstructions()["GitHub"], "owner-only")
        XCTAssertNotNil(store.getAgentDefaultModel())

        store.setHasSeenOnboarding(true)
        store.scopeToAccount("owner-b")
        let scoped = store.load()

        XCTAssertTrue(scoped.mcpCustomInstructions.isEmpty)
        XCTAssertTrue(scoped.mcpCustomInstructionsByServerId.isEmpty)
        XCTAssertTrue(scoped.mcpDisabledToolsByServerId.isEmpty)
        XCTAssertNil(scoped.agentDefaultModel)
        XCTAssertNil(scoped.hasSeenOnboarding)
        XCTAssertEqual(scoped.mcpCustomInstructionsAccountScope, "owner-b")
    }

    @MainActor
    func testAccountAuthorizerClaimsFirstUnscopedSettingsAndAbandonsForeignScope() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        store.setMcpCustomInstructions(["GitHub": "legacy-owner"])
        store.setHasSeenOnboarding(true)

        var localHumanID: String?
        let authorizer = IOSAccountAuthorizer(
            applyAccountScope: { slot in
                if let slot {
                    store.scopeToAccount(slot)
                } else {
                    store.clearAccountScope()
                }
            },
            applyLocalHumanIdentity: { slot in
                localHumanID = slot
            }
        )

        XCTAssertEqual(
            authorizer.authorizeSettledHostSlot("owner-a", previousSlot: nil),
            .ready(slot: "owner-a")
        )
        XCTAssertEqual(store.getMcpCustomInstructions()["GitHub"], "legacy-owner")
        XCTAssertEqual(store.load().hasSeenOnboarding, true)
        XCTAssertEqual(localHumanID, "owner-a")

        XCTAssertEqual(
            authorizer.authorizeSettledHostSlot("owner-b", previousSlot: "owner-a"),
            .ready(slot: "owner-b")
        )
        let replaced = store.load()
        XCTAssertTrue(replaced.mcpCustomInstructions.isEmpty)
        XCTAssertNil(replaced.hasSeenOnboarding)
        XCTAssertEqual(replaced.mcpCustomInstructionsAccountScope, "owner-b")
        XCTAssertEqual(localHumanID, "owner-b")
    }

    func testDefaultsFailClosedForInvalidOrCorruptSettings() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        try Data(#"{ "version": 99, "themePreference": "dark" }"#.utf8).write(to: path)

        let unsupported = SandSettingsStore(settingsPath: path.path).load()
        XCTAssertEqual(unsupported.version, SETTINGS_VERSION)
        XCTAssertEqual(unsupported.themePreference, nil)
        XCTAssertTrue(unsupported.webauthnProxyEnabled)

        try Data("not-json".utf8).write(to: path)
        let corrupt = SandSettingsStore(settingsPath: path.path).load()
        XCTAssertEqual(corrupt.version, SETTINGS_VERSION)
        XCTAssertTrue(corrupt.mcpBoxServers.isEmpty)
        XCTAssertEqual(corrupt.conciergeConsent, "unset")
    }
    func testCurrentMainUiAndCallPreferencesNormalizeAndPersist() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)

        store.setUiPreferences(.init(
            locale: "AR-sa",
            direction: .auto,
            reducedMotion: true,
            highContrast: true,
            textScale: 9
        ))
        store.setCallMediaPreferences(.init(
            microphoneId: " mic-1 ",
            cameraId: String(repeating: "x", count: 513)
        ))

        let restored = SandSettingsStore(settingsPath: path.path)
        let ui = restored.getUiPreferences()
        XCTAssertEqual(ui.locale, "ar-sa")
        XCTAssertEqual(ui.direction, .auto)
        XCTAssertEqual(resolveSandUiDirection(ui), .rtl)
        XCTAssertTrue(ui.reducedMotion)
        XCTAssertTrue(ui.highContrast)
        XCTAssertEqual(ui.textScale, 2)

        let media = restored.getCallMediaPreferences()
        XCTAssertEqual(media.microphoneId, "mic-1")
        XCTAssertNil(media.cameraId)
    }

    func testAllowedExternalUrlStripsForeignWebAuthTokensFromQueryAndFragment() throws {
        let sanitized = try XCTUnwrap(ExternalURLPolicy.parseAllowed(
            "https://example.com/path?keep=1&tgWebAuthToken=secret&%2561utologin_token=hidden#route?ok=2&tgwebauthnonce=leak"
        ))
        let components = try XCTUnwrap(URLComponents(string: sanitized))
        XCTAssertEqual(components.host, "example.com")
        XCTAssertEqual(components.path, "/path")
        XCTAssertEqual(components.percentEncodedQuery, "keep=1")
        XCTAssertEqual(components.percentEncodedFragment, "route?ok=2")
        XCTAssertFalse(sanitized.lowercased().contains("tgwebauth"))
        XCTAssertFalse(sanitized.lowercased().contains("autologin_token"))
    }

    func testAllowedExternalUrlPreservesUnrelatedParametersAndNonWebSchemes() {
        XCTAssertEqual(
            ExternalURLPolicy.parseAllowed("https://example.com/?token=safe#route?mode=1"),
            "https://example.com/?token=safe#route?mode=1"
        )
        XCTAssertEqual(
            ExternalURLPolicy.parseAllowed("mailto:user@example.com?autologin_token=mail-metadata"),
            "mailto:user@example.com?autologin_token=mail-metadata"
        )
    }

    func testServerAcceptedBrowserAuthUrlMustStayOnConfiguredOrigin() {
        XCTAssertEqual(
            ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
                "https://api.ombhrum.com/auth/browser?attempt=1",
                expectedOrigin: "https://api.ombhrum.com"
            ),
            "https://api.ombhrum.com/auth/browser?attempt=1"
        )
        XCTAssertNil(ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
            "https://attacker.invalid/auth/browser",
            expectedOrigin: "https://api.ombhrum.com"
        ))
        XCTAssertNil(ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
            "http://api.ombhrum.com/auth/browser",
            expectedOrigin: "https://api.ombhrum.com"
        ))
        XCTAssertNil(ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
            "https://user@api.ombhrum.com/auth/browser",
            expectedOrigin: "https://api.ombhrum.com"
        ))
    }

    @MainActor
    func testCoordinatorOwnsAndScopesSettingsWithoutRendererHostBypass() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(settingsPath: root.appendingPathComponent("settings.json").path)
        store.setMcpCustomInstructions(["GitHub": "owner-a"])
        store.scopeToAccount("owner-a")

        let host = SettingsParityHost()
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor, settingsStore: store)

        XCTAssertEqual(coordinator.sharedSettingsSnapshot().mcpCustomInstructions["GitHub"], "owner-a")
        coordinator.updateAccountSettingsScope("owner-b")
        XCTAssertTrue(coordinator.sharedSettingsSnapshot().mcpCustomInstructions.isEmpty)
        coordinator.updateAccountSettingsScope(nil)
        XCTAssertNil(coordinator.sharedSettingsSnapshot().mcpCustomInstructionsAccountScope)
    }

    @MainActor
    func testOnboardingSeenRoutesThroughAccountScopedCanonicalSettingsOwner() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        store.scopeToAccount("owner-a")
        let host = SettingsParityHost()
        let coordinator = MahayanaCoordinator(
            hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }),
            settingsStore: store
        )

        let initial = try await coordinator.request(method: "getOnboardingSeen")
        XCTAssertEqual(initial.value as? Bool, false)

        let updated = try await coordinator.request(
            method: "setOnboardingSeen",
            params: ["seen": true]
        )
        XCTAssertEqual(updated.value as? Bool, true)
        XCTAssertEqual(store.getHasSeenOnboarding(), true)

        coordinator.updateAccountSettingsScope("owner-b")
        let foreignAccount = try await coordinator.request(method: "getOnboardingSeen")
        XCTAssertEqual(foreignAccount.value as? Bool, false)
        XCTAssertNil(store.getHasSeenOnboarding())
    }

    func testAutoReviewInstructionEditorPreservesIdentityAndCrossListEdits() {
        let base = SandAutoReviewInstructions(
            isEnabled: true,
            allowInstructions: ["read files", "run tests"],
            blockInstructions: ["delete files"]
        )
        XCTAssertEqual(
            sandAutoReviewInstructionRows(base),
            [
                .init(behavior: .allow, text: "read files", listIndex: 0),
                .init(behavior: .allow, text: "run tests", listIndex: 1),
                .init(behavior: .ask, text: "delete files", listIndex: 0),
            ]
        )
        XCTAssertNil(
            saveSandAutoReviewInstruction(
                base,
                text: "run tests",
                behavior: .allow,
                editing: nil
            )
        )

        let edited = saveSandAutoReviewInstruction(
            base,
            text: "run checks",
            behavior: .allow,
            editing: .init(
                behavior: .allow,
                text: "run tests",
                listIndex: 1
            )
        )
        XCTAssertEqual(
            edited?.allowInstructions,
            ["read files", "run checks"]
        )

        let moved = saveSandAutoReviewInstruction(
            base,
            text: "delete files",
            behavior: .allow,
            editing: .init(
                behavior: .ask,
                text: "delete files",
                listIndex: 0
            )
        )
        XCTAssertEqual(
            moved?.allowInstructions,
            ["read files", "run tests", "delete files"]
        )
        XCTAssertEqual(moved?.blockInstructions, [])

        let removed = removeSandAutoReviewInstruction(
            base,
            row: .init(
                behavior: .allow,
                text: "read files",
                listIndex: 0
            )
        )
        XCTAssertEqual(removed.allowInstructions, ["run tests"])

        XCTAssertEqual(
            reconcileSandAutoReviewInstructionRow(
                .init(
                    isEnabled: true,
                    allowInstructions: ["new", "run tests"],
                    blockInstructions: []
                ),
                row: .init(
                    behavior: .allow,
                    text: "run tests",
                    listIndex: 0
                )
            ),
            .init(
                behavior: .allow,
                text: "run tests",
                listIndex: 1
            )
        )

        let full = SandAutoReviewInstructions(
            isEnabled: true,
            allowInstructions: (0..<SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES)
                .map { "allow-\($0)" },
            blockInstructions: []
        )
        XCTAssertNil(
            saveSandAutoReviewInstruction(
                full,
                text: "overflow",
                behavior: .allow,
                editing: nil
            )
        )

        let oversized = String(repeating: "😀", count: 501)
        let bounded = saveSandAutoReviewInstruction(
            .init(isEnabled: true, allowInstructions: [], blockInstructions: []),
            text: oversized,
            behavior: .allow,
            editing: nil
        )
        XCTAssertEqual(
            bounded?.allowInstructions.first?.utf16.count,
            SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS
        )
        XCTAssertEqual(
            clampSandAutoReviewInstructionDraft(oversized).utf16.count,
            SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS
        )

        let exactLimit = String(repeating: "x", count: SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS)
        XCTAssertNil(
            saveSandAutoReviewInstruction(
                .init(
                    isEnabled: true,
                    allowInstructions: [exactLimit],
                    blockInstructions: []
                ),
                text: exactLimit + "x",
                behavior: .allow,
                editing: nil
            ),
            "Duplicate detection must run after the same 1000-unit clamp used by persistence."
        )
    }

    @MainActor
    func testHostSettingsReconcilerAbsorbsRemoteTrueAndFalseIntoCanonicalLocalMirror() async {
        var local: Bool?
        var remote: Bool? = true
        var generation: UInt64 = 1

        let reconciler = IOSHostSettingsReconciler(
            readLocal: { local },
            writeLocal: { local = $0 },
            clearLocal: { local = nil },
            readRemote: { .init(hasSeenOnboarding: remote) },
            pushRemote: { snapshot in
                remote = snapshot.hasSeenOnboarding
                return .init(hasSeenOnboarding: remote)
            },
            hostGeneration: { generation }
        )
        reconciler.scopeToAccount("owner-a")
        reconciler.setTransportLive(true)

        let reconciledRemoteTrue = await reconciler.reconcileIfReadable()
        XCTAssertTrue(reconciledRemoteTrue)
        XCTAssertEqual(local, true)
        XCTAssertEqual(reconciler.lastSuccessfulAccountScope, "owner-a")

        reconciler.accountDeparted()
        local = nil
        remote = false
        generation &+= 1
        reconciler.scopeToAccount("owner-b")
        reconciler.setTransportLive(true)

        let reconciledRemoteFalse = await reconciler.reconcileIfReadable()
        XCTAssertTrue(reconciledRemoteFalse)
        XCTAssertEqual(local, false)
        XCTAssertEqual(remote, false)
        XCTAssertEqual(reconciler.lastSuccessfulAccountScope, "owner-b")
    }

    @MainActor
    func testHostSettingsTransportDownWritesLocalOnlyThenReconnectBackfillsUnwrittenRemote() async {
        var local: Bool?
        var remote: Bool?
        var remoteReads = 0
        var remoteWrites = 0

        let reconciler = IOSHostSettingsReconciler(
            readLocal: { local },
            writeLocal: { local = $0 },
            clearLocal: { local = nil },
            readRemote: {
                remoteReads += 1
                return .init(hasSeenOnboarding: remote)
            },
            pushRemote: { snapshot in
                remoteWrites += 1
                remote = snapshot.hasSeenOnboarding
                return .init(hasSeenOnboarding: remote)
            },
            hostGeneration: { 1 }
        )
        reconciler.scopeToAccount("owner-a")

        let pushedWhileOffline = await reconciler.pushLocalIfWritable(true)
        XCTAssertFalse(pushedWhileOffline)
        XCTAssertEqual(local, true)
        XCTAssertNil(remote)
        XCTAssertEqual(remoteReads, 0)
        XCTAssertEqual(remoteWrites, 0)

        reconciler.setTransportLive(true)
        let reconciledAfterReconnect = await reconciler.reconcileIfReadable()
        XCTAssertTrue(reconciledAfterReconnect)
        XCTAssertEqual(remote, true)
        XCTAssertGreaterThanOrEqual(remoteReads, 1)
        XCTAssertGreaterThanOrEqual(remoteWrites, 1)

        reconciler.setTransportLive(false)
        let reconciledWhileOffline = await reconciler.reconcileIfReadable()
        XCTAssertFalse(reconciledWhileOffline)
        let pushedFalseWhileOffline = await reconciler.pushLocalIfWritable(false)
        XCTAssertFalse(pushedFalseWhileOffline)
        XCTAssertEqual(local, false)
        XCTAssertEqual(remote, true)
    }

    @MainActor
    func testHostSettingsDropsInFlightResultAfterAccountReplacementAndDeparture() async {
        var local: Bool?
        var continuation: CheckedContinuation<MahayanaHostSettingsSnapshot, Error>?
        let readStarted = expectation(description: "remote host-settings read started")

        let reconciler = IOSHostSettingsReconciler(
            readLocal: { local },
            writeLocal: { local = $0 },
            clearLocal: { local = nil },
            readRemote: {
                try await withCheckedThrowingContinuation { pending in
                    continuation = pending
                    readStarted.fulfill()
                }
            },
            pushRemote: { $0 },
            hostGeneration: { 1 }
        )
        reconciler.scopeToAccount("owner-a")
        reconciler.setTransportLive(true)
        await fulfillment(of: [readStarted], timeout: 2)

        reconciler.scopeToAccount("owner-b")
        continuation?.resume(returning: .init(hasSeenOnboarding: true))
        await Task.yield()
        await Task.yield()

        XCTAssertNil(local)
        XCTAssertNil(reconciler.lastSuccessfulAccountScope)

        reconciler.accountDeparted()
        XCTAssertFalse(reconciler.isReadable)
        let reconciledAfterDeparture = await reconciler.reconcileIfReadable()
        XCTAssertFalse(reconciledAfterDeparture)
        XCTAssertNil(reconciler.lastSuccessfulAccountScope)
    }

    @MainActor
    func testHostSettingsFailureNeverReportsSuccessfulReconciliation() async {
        var local: Bool?
        let reconciler = IOSHostSettingsReconciler(
            readLocal: { local },
            writeLocal: { local = $0 },
            clearLocal: { local = nil },
            readRemote: {
                throw NSError(
                    domain: "SharedSettingsParityTests",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "host settings unavailable"]
                )
            },
            pushRemote: { $0 },
            hostGeneration: { 1 }
        )
        reconciler.scopeToAccount("owner-a")
        reconciler.setTransportLive(true)

        let reconciledFailure = await reconciler.reconcileIfReadable()
        XCTAssertFalse(reconciledFailure)
        XCTAssertNil(local)
        XCTAssertNil(reconciler.lastSuccessfulAccountScope)
    }

    @MainActor
    func testCoordinatorOwnsTimeZoneAndLocalToolPermissionFacades() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        store.setUserTimeZone("America/Los_Angeles")
        store.setLocalToolPermissionCeiling("ask")
        let host = SettingsParityHost()
        let coordinator = MahayanaCoordinator(
            hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }),
            settingsStore: store
        )

        let initialZone = try await coordinator.request(method: "getTimeZone")
        let initialZoneObject = try XCTUnwrap(initialZone.value as? [String: Any])
        XCTAssertEqual(initialZoneObject["detectedTimeZone"] as? String, "America/Los_Angeles")
        XCTAssertTrue(initialZoneObject["overrideTimeZone"] is NSNull)

        let updatedZone = try await coordinator.request(
            method: "setTimeZoneOverride",
            params: ["timeZone": "Asia/Tokyo"]
        )
        XCTAssertEqual(
            (updatedZone.value as? [String: Any])?["overrideTimeZone"] as? String,
            "Asia/Tokyo"
        )
        XCTAssertEqual(store.getUserTimeZone(), "Asia/Tokyo")

        do {
            _ = try await coordinator.request(
                method: "setTimeZoneOverride",
                params: ["timeZone": "Mars/Olympus"]
            )
            XCTFail("invalid IANA zones must fail closed")
        } catch {
            XCTAssertEqual(store.getUserTimeZoneOverride(), "Asia/Tokyo")
        }

        let initialLocalPermission = try await coordinator.request(
            method: "getLocalToolPermission"
        )
        XCTAssertEqual(initialLocalPermission.value as? String, "ask")

        let initialLocalPermissionCeiling = try await coordinator.request(
            method: "getLocalToolPermissionCeiling"
        )
        XCTAssertEqual(initialLocalPermissionCeiling.value as? String, "ask")

        let updatedLocalPermission = try await coordinator.request(
            method: "setLocalToolPermission",
            params: ["permission": "never"]
        )
        XCTAssertEqual(updatedLocalPermission.value as? String, "never")
        XCTAssertEqual(store.getLocalToolPermission(), "never")

        do {
            _ = try await coordinator.request(
                method: "setLocalToolPermission",
                params: ["permission": "always"]
            )
            XCTFail("admin ceiling must reject a less restrictive permission")
        } catch {
            XCTAssertEqual(store.getLocalToolPermission(), "never")
        }
    }

    @MainActor
    func testCoordinatorInferenceProviderFacadeUsesCanonicalSettingsStore() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        let host = SettingsParityHost()
        let supervisor = MahayanaLocalHostSupervisor(
            host: host,
            factory: { host }
        )
        let coordinator = MahayanaCoordinator(
            hostSupervisor: supervisor,
            settingsStore: store
        )

        let initial = try await coordinator.request(
            method: "getInferenceProvider"
        )
        XCTAssertEqual(
            (initial.value as? [String: Any])?["provider"] as? String,
            SandInferenceProvider.fabushi.rawValue
        )

        let updated = try await coordinator.request(
            method: "setInferenceProvider",
            params: [
                "provider": SandInferenceProvider.openrouter.rawValue
            ]
        )
        XCTAssertEqual(
            (updated.value as? [String: Any])?["provider"] as? String,
            SandInferenceProvider.openrouter.rawValue
        )
        XCTAssertEqual(store.getInferenceProvider(), .openrouter)
        XCTAssertEqual(
            Set(SAND_INFERENCE_PROVIDER_DESCRIPTORS.map(\.provider)),
            Set(SandInferenceProvider.allCases)
        )
        XCTAssertEqual(
            sandInferenceProviderDescriptor(.openrouter).usageSource,
            .external
        )

        do {
            _ = try await coordinator.request(
                method: "setInferenceProvider",
                params: ["provider": "missing"]
            )
            XCTFail("unknown providers must fail closed")
        } catch {
            XCTAssertEqual(store.getInferenceProvider(), .openrouter)
        }
    }


    func testLocalToolApprovalsPersistAcrossRestartAndFenceAccountReplacement() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)
        store.scopeToAccount("owner-a")

        XCTAssertTrue(try store.recordLocalToolApproval(
            id: "approval-1",
            action: "run-command",
            target: "swift test",
            expectedAccountScope: "owner-a"
        ))
        XCTAssertEqual(
            store.getLocalToolApprovals(expectedAccountScope: "owner-a"),
            [.init(id: "approval-1", action: "run-command", target: "swift test")]
        )

        let restored = SandSettingsStore(settingsPath: path.path)
        XCTAssertEqual(
            restored.getLocalToolApprovals(expectedAccountScope: "owner-a"),
            [.init(id: "approval-1", action: "run-command", target: "swift test")]
        )

        restored.scopeToAccount("owner-b")
        XCTAssertTrue(restored.getLocalToolApprovals(expectedAccountScope: "owner-a").isEmpty)
        XCTAssertTrue(restored.getLocalToolApprovals(expectedAccountScope: "owner-b").isEmpty)
        XCTAssertFalse(try restored.recordLocalToolApproval(
            id: "stale",
            action: "read-file",
            target: "/tmp/stale",
            expectedAccountScope: "owner-a"
        ))
        XCTAssertTrue(try restored.recordLocalToolApproval(
            id: "approval-2",
            action: "read-file",
            target: "/tmp/current",
            expectedAccountScope: "owner-b"
        ))
        XCTAssertFalse(try restored.clearLocalToolApprovals(expectedAccountScope: "owner-a"))
        XCTAssertEqual(
            restored.getLocalToolApprovals(expectedAccountScope: "owner-b").map(\.id),
            ["approval-2"]
        )
        XCTAssertTrue(try restored.clearLocalToolApprovals(expectedAccountScope: "owner-b"))
        XCTAssertTrue(restored.getLocalToolApprovals(expectedAccountScope: "owner-b").isEmpty)
    }

    func testLocalToolPermissionTranscriptProjectionRetainsCanonicalApprovalIdentity() throws {
        let event: [String: Any] = [
            "entryId": "entry-1",
            "card": [
                "kind": "send-message",
                "message": [
                    "type": "local-tool-permission",
                    "ask": [
                        "requestId": "approval-1",
                        "status": "pending",
                        "action": "run-command",
                        "target": "printf 'hello'",
                    ],
                ],
            ],
        ]
        let row = try XCTUnwrap(projectMobileTranscriptCard(event: event, operationId: "operation-1"))
        XCTAssertEqual(row.localToolPermissionRequestId, "approval-1")
        XCTAssertEqual(row.localToolPermissionAction, "run-command")
        XCTAssertEqual(row.localToolPermissionTarget, "printf 'hello'")

        var malformed = event
        malformed["card"] = [
            "kind": "send-message",
            "message": [
                "type": "local-tool-permission",
                "ask": [
                    "requestId": "approval-2",
                    "status": "pending",
                    "action": "spawn-arbitrary-process",
                    "target": "unsafe",
                ],
            ],
        ]
        XCTAssertNil(projectMobileTranscriptCard(event: malformed, operationId: "operation-1"))
    }

    @MainActor
    func testCoordinatorLocalToolApprovalLifecyclePersistsAllowOnceAndAcceptedSendClears() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        let host = SettingsParityHost()
        let coordinator = MahayanaCoordinator(
            hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }),
            settingsStore: store
        )
        coordinator.updateAccountSettingsScope("owner-a")

        let resolved = try await coordinator.request(
            method: "resolveLocalToolPermissionWithApprovalLifecycle",
            params: [
                "entryId": "entry-1",
                "requestId": "approval-1",
                "agentId": "agent-1",
                "action": "run-command",
                "target": "swift test",
                "resolution": "allow-once",
            ]
        )
        XCTAssertEqual(resolved.value as? String, "allow-once")
        XCTAssertEqual(
            store.getLocalToolApprovals(expectedAccountScope: "owner-a").map(\.id),
            ["approval-1"]
        )

        let sent = try await coordinator.request(
            method: "feature.execute",
            params: [
                "command": [
                    "type": "chat.send",
                    "requestId": "send-1",
                    "agentId": "agent-1",
                    "text": "continue",
                ],
            ]
        )
        XCTAssertNotNil(sent.value)
        XCTAssertTrue(store.getLocalToolApprovals(expectedAccountScope: "owner-a").isEmpty)

        let cleanup = try await coordinator.request(method: "getLocalToolApprovalCleanupState")
        let cleanupObject = try XCTUnwrap(cleanup.value as? [String: Any])
        XCTAssertEqual(cleanupObject["status"] as? String, "idle")
        XCTAssertTrue(cleanupObject["failure"] is NSNull)
    }

    @MainActor
    func testCoordinatorLocalToolApprovalLifecycleFailsClosedAndAccountSwitchFencesStaleApprovals() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        let host = SettingsParityHost()
        let coordinator = MahayanaCoordinator(
            hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }),
            settingsStore: store
        )
        coordinator.updateAccountSettingsScope("owner-a")

        do {
            _ = try await coordinator.request(
                method: "resolveLocalToolPermissionWithApprovalLifecycle",
                params: [
                    "entryId": "entry-invalid",
                    "requestId": "approval-invalid",
                    "agentId": "agent-1",
                    "action": "spawn-arbitrary-process",
                    "target": "unsafe",
                    "resolution": "allow-once",
                ]
            )
            XCTFail("unknown local-tool actions must fail closed")
        } catch {
            XCTAssertTrue(store.getLocalToolApprovals(expectedAccountScope: "owner-a").isEmpty)
        }

        _ = try await coordinator.request(
            method: "resolveLocalToolPermissionWithApprovalLifecycle",
            params: [
                "entryId": "entry-2",
                "requestId": "approval-2",
                "agentId": "agent-1",
                "action": "read-file",
                "target": "/tmp/current",
                "resolution": "allow-once",
            ]
        )
        XCTAssertEqual(store.getLocalToolApprovals(expectedAccountScope: "owner-a").count, 1)

        coordinator.updateAccountSettingsScope("owner-b")
        XCTAssertTrue(store.getLocalToolApprovals(expectedAccountScope: "owner-a").isEmpty)
        XCTAssertTrue(store.getLocalToolApprovals(expectedAccountScope: "owner-b").isEmpty)
        XCTAssertFalse(try store.clearLocalToolApprovals(expectedAccountScope: "owner-a"))
    }

    @MainActor
    func testAcceptedSendSurvivesApprovalCleanupPersistenceFailure() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)
        let host = SettingsParityHost()
        let coordinator = MahayanaCoordinator(
            hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }),
            settingsStore: store
        )
        coordinator.updateAccountSettingsScope("owner-a")
        _ = try await coordinator.request(
            method: "resolveLocalToolPermissionWithApprovalLifecycle",
            params: [
                "entryId": "entry-3",
                "requestId": "approval-3",
                "agentId": "agent-1",
                "action": "run-command",
                "target": "swift test",
                "resolution": "allow-once",
            ]
        )

        try FileManager.default.removeItem(at: path)
        try FileManager.default.createDirectory(at: path, withIntermediateDirectories: false)

        let sent = try await coordinator.request(
            method: "feature.execute",
            params: [
                "command": [
                    "type": "chat.send",
                    "requestId": "send-cleanup-failure",
                    "agentId": "agent-1",
                    "text": "continue",
                ],
            ]
        )
        XCTAssertNotNil(sent.value, "cleanup failure must not flip an accepted send")

        let cleanup = try await coordinator.request(method: "getLocalToolApprovalCleanupState")
        let cleanupObject = try XCTUnwrap(cleanup.value as? [String: Any])
        XCTAssertEqual(cleanupObject["status"] as? String, "failed")
        XCTAssertNotNil(cleanupObject["failure"] as? String)
    }


    func testComposerSubmissionQueuePersistsOrdersDeduplicatesAndFencesAccountSwitch() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)
        store.scopeToAccount("owner-a")
        let one = #"{"agentId":"agent-1","requestId":"nonce-1","text":"first","type":"chat.send"}"#
        let two = #"{"agentId":"agent-1","requestId":"nonce-2","text":"second","type":"chat.send"}"#
        XCTAssertTrue(try store.enqueueComposerSubmission(nonce: "nonce-1", agentId: "agent-1", createdAtMs: 10, commandJSON: one, expectedAccountScope: "owner-a"))
        XCTAssertTrue(try store.enqueueComposerSubmission(nonce: "nonce-2", agentId: "agent-1", createdAtMs: 20, commandJSON: two, expectedAccountScope: "owner-a"))
        XCTAssertTrue(try store.enqueueComposerSubmission(nonce: "nonce-1", agentId: "agent-1", createdAtMs: 10, commandJSON: one, expectedAccountScope: "owner-a"))
        XCTAssertEqual(store.composerSubmissions(expectedAccountScope: "owner-a", agentId: "agent-1").map(\.nonce), ["nonce-1", "nonce-2"])
        let restored = SandSettingsStore(settingsPath: path.path)
        XCTAssertEqual(restored.composerSubmissions(expectedAccountScope: "owner-a").map(\.nonce), ["nonce-1", "nonce-2"])
        XCTAssertTrue(try restored.removeComposerSubmission(nonce: "nonce-1", expectedAccountScope: "owner-a"))
        restored.scopeToAccount("owner-b")
        XCTAssertTrue(restored.composerSubmissions(expectedAccountScope: "owner-a").isEmpty)
        XCTAssertFalse(try restored.enqueueComposerSubmission(nonce: "stale", agentId: "agent-1", createdAtMs: 30, commandJSON: one, expectedAccountScope: "owner-a"))
    }

    @MainActor
    func testCoordinatorComposerQueueRoundTripUsesExactChatSendCommand() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(settingsPath: root.appendingPathComponent("settings.json").path)
        let host = SettingsParityHost()
        let coordinator = MahayanaCoordinator(hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }), settingsStore: store)
        coordinator.updateAccountSettingsScope("owner-a")
        let command: [String: Any] = [
            "type": "chat.send", "requestId": "nonce-queue", "agentId": "agent-1", "text": "hello",
            "attachments": [["id": "attachment-1", "name": "a.txt", "path": "/durable/attachment-1", "sizeBytes": 3]]
        ]
        _ = try await coordinator.request(method: "native.composerQueue.enqueue", params: ["command": command])
        let listed = try await coordinator.request(method: "native.composerQueue.list", params: ["agentId": "agent-1"])
        let rows = try XCTUnwrap(listed.value as? [[String: Any]])
        XCTAssertEqual(rows.count, 1)
        let restored = try XCTUnwrap(rows[0]["command"] as? [String: Any])
        XCTAssertEqual(restored["requestId"] as? String, "nonce-queue")
        _ = try await coordinator.request(method: "native.composerQueue.cancel", params: ["nonce": "nonce-queue"])
        let empty = try await coordinator.request(method: "native.composerQueue.list", params: ["agentId": "agent-1"])
        XCTAssertEqual((empty.value as? [[String: Any]])?.count, 0)
    }

}
