import Foundation
import XCTest
@testable import Fabushi

private func profileCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "profile-token" },
        getMachineId: { "profile-machine" }
    )
}

private func jsonResponse(_ request: URLRequest, _ object: [String: Any]) throws -> (Data, URLResponse) {
    (
        try JSONSerialization.data(withJSONObject: object),
        HTTPURLResponse(
            url: request.url!,
            statusCode: 200,
            httpVersion: "HTTP/2",
            headerFields: ["Content-Type": "application/json"]
        )!
    )
}

final class CursorProfileParityTests: XCTestCase {
    func testUpdateNameSplitsFirstTokenFromRemainingName() async throws {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(request.url?.path, "/aiserver.v1.DashboardService/UpdateUserName")
                XCTAssertEqual(request.timeoutInterval, 10)
                let body = try JSONSerialization.jsonObject(with: request.httpBody!) as? [String: String]
                XCTAssertEqual(body?["firstName"], "Ada")
                XCTAssertEqual(body?["lastName"], "Lovelace Byron")
                return try jsonResponse(request, [:])
            }
        )
        try await client.updateCursorAccountName("  Ada   Lovelace Byron ")
    }

    func testWeeklyUsagePreservesIncludedAndOnDemandSemantics() async throws {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                switch request.url?.lastPathComponent {
                case "GetSandUsageStatus":
                    return try jsonResponse(request, [
                        "usagePercent": 42.5,
                        "nextResetTimestampUtc": "2026-10-10T00:00:00Z",
                        "usesPooledEnterpriseAllowance": false,
                        "hasNonZeroIncludedLimit": true,
                    ])
                case "GetCurrentPeriodUsage":
                    return try jsonResponse(request, [
                        "spendLimitUsage": [
                            "individualUsed": 500,
                            "individualLimit": 1000,
                        ],
                    ])
                default:
                    XCTFail("Unexpected weekly usage method")
                    return try jsonResponse(request, [:])
                }
            }
        )
        let usage = await client.getCursorWeeklyUsage()
        XCTAssertEqual(usage?["percentUsed"] as? Double, 42.5)
        XCTAssertEqual(usage?["hasNonZeroIncludedLimit"] as? Bool, true)
        let onDemand = usage?["onDemand"] as? [String: Any]
        XCTAssertEqual(onDemand?["usedCents"] as? Double, 500)
        XCTAssertEqual(onDemand?["limitCents"] as? Double, 1000)
        XCTAssertNotNil(usage?["nextResetMs"] as? Int64)
    }

    func testPrivacyModeDefaultsFailClosedAndDisablesForTrainingModes() async throws {
        let allowed = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                try jsonResponse(request, [
                    "privacyMode": "PRIVACY_MODE_USAGE_DATA_TRAINING_ALLOWED",
                ])
            }
        )
        let allowedPrivacyMode = await allowed.getCursorPrivacyModeEnabled()
        XCTAssertFalse(allowedPrivacyMode)

        let noStorage = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                try jsonResponse(request, [
                    "privacyMode": "PRIVACY_MODE_NO_STORAGE",
                ])
            }
        )
        let noStoragePrivacyMode = await noStorage.getCursorPrivacyModeEnabled()
        XCTAssertTrue(noStoragePrivacyMode)
    }

    func testUsageSummaryAndDashboardActionUseDesktopTimeoutAndProjection() async throws {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(request.timeoutInterval, 15)
                switch request.url?.lastPathComponent {
                case "GetSandUsageStatus":
                    return try jsonResponse(request, [
                        "usagePercent": 25,
                        "hasAvailableUsage": true,
                        "hasNonZeroIncludedLimit": true,
                        "sandTrialCancelable": false,
                        "upgradeRecommendation": [
                            "disabled": false,
                            "cta": [
                                "label": "Ask for more",
                                "dashboardAction": [
                                    "action": "requestLimitIncrease",
                                    "args": ["source": "usage"],
                                    "successMessage": "Requested",
                                ],
                            ],
                        ],
                    ])
                case "GetCurrentPeriodUsage":
                    return try jsonResponse(request, [
                        "billingCycleEnd": "1791590400000",
                        "spendLimitUsage": [
                            "individualUsed": 200,
                            "individualLimit": 800,
                        ],
                    ])
                case "GetTeams":
                    return try jsonResponse(request, [
                        "teams": [["id": 8, "isEnterprise": true]],
                    ])
                case "GetSandTrialClaimStatus":
                    return try jsonResponse(request, [
                        "status": "SAND_TRIAL_CLAIM_STATUS_GRANTED",
                    ])
                case "ClientAction":
                    let body = try JSONSerialization.jsonObject(with: request.httpBody!) as? [String: Any]
                    XCTAssertEqual(body?["action"] as? String, "requestLimitIncrease")
                    return try jsonResponse(request, [
                        "success": true,
                        "infoMessage": "Done",
                    ])
                default:
                    XCTFail("Unexpected usage method")
                    return try jsonResponse(request, [:])
                }
            }
        )

        let summary = try await client.getCursorUsageSummary()
        XCTAssertEqual(summary["isEnterprise"] as? Bool, true)
        XCTAssertEqual(summary["hasEndedSandTrial"] as? Bool, true)
        let cta = summary["upgradeCta"] as? [String: Any]
        XCTAssertEqual(cta?["label"] as? String, "Ask for more")

        let action = try await client.invokeCursorDashboardAction(
            action: "requestLimitIncrease",
            args: ["source": "usage"]
        )
        XCTAssertEqual(action["ok"] as? Bool, true)
        XCTAssertEqual(action["message"] as? String, "Done")
    }

    func testAnysphereMembershipRequiresTeamOneBillingAndPositiveSeats() async throws {
        let member = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(request.url?.path, "/aiserver.v1.DashboardService/GetTeams")
                return try jsonResponse(request, [
                    "teams": [
                        ["id": 8, "hasBilling": true, "seats": 100],
                        ["id": 1, "hasBilling": true, "seats": 3],
                    ],
                ])
            }
        )
        XCTAssertTrue(try await member.isAnysphereUser())

        let notMember = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                try jsonResponse(request, [
                    "teams": [
                        ["id": 1, "hasBilling": false, "seats": 3],
                        ["id": 1, "hasBilling": true, "seats": 0],
                    ],
                ])
            }
        )
        XCTAssertFalse(try await notMember.isAnysphereUser())
    }

    func testCancelTrialReturnsStableResultInsteadOfThrowing() async {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { _ in
                throw IOSCursorDashboardError(message: "cancel failed")
            }
        )
        let result = await client.cancelCursorSandTrial()
        XCTAssertEqual(result["ok"] as? Bool, false)
        XCTAssertEqual(result["message"] as? String, "cancel failed")
    }
}


@MainActor
final class CursorLocalToolPermissionCeilingParityTests: XCTestCase {
    private enum TestError: Error { case fetch, host }

    private func makeStore() throws -> (URL, SandSettingsStore) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-cursor-ceiling-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return (root, SandSettingsStore(settingsPath: root.appendingPathComponent("settings.json").path))
    }

    func testDashboardPermissionCeilingDecodesAuthoritativeDesktopWireFields() async throws {
        func read(_ raw: UInt8) async throws -> SandLocalToolPermission? {
            let client = IOSCursorDashboardClient(
                credentials: profileCredentials(),
                backendURL: URL(string: "https://backend.example.test")!,
                requestExecutor: { request in
                    XCTAssertEqual(request.url?.path, "/aiserver.v1.DashboardService/GetTeamAdminSettingsOrEmptyIfNotInTeam")
                    XCTAssertEqual(request.timeoutInterval, 10)
                    let body = Data([0xe2, 0x03, 0x02, 0x08, raw])
                    return (body, HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: "HTTP/2", headerFields: ["Content-Type": "application/proto"])!)
                }
            )
            return try await client.getLocalToolPermissionCeiling()
        }
        let never = try await read(1)
        let ask = try await read(2)
        let always = try await read(3)
        let unknown = try await read(0)
        XCTAssertEqual(never, "never")
        XCTAssertEqual(ask, "ask")
        XCTAssertEqual(always, "always")
        XCTAssertNil(unknown)
    }

    func testLoggedInCeilingAppliesAndEffectiveChangeSyncsHostOnce() async throws {
        let (root, store) = try makeStore()
        defer { try? FileManager.default.removeItem(at: root) }
        store.setLocalToolPermission("always")
        var hostSyncs: [SandLocalToolPermission] = []
        let sync = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: store,
            fetchCeiling: { "ask" },
            syncHost: { hostSyncs.append($0) },
            reportFailure: { _, _, _ in XCTFail("unexpected failure") }
        )
        sync.consume(.init(loggedIn: true))
        await sync.waitForIdleForTesting()
        XCTAssertEqual(store.getLocalToolPermissionCeiling(), "ask")
        XCTAssertEqual(store.getResolvedLocalToolPermission(), "ask")
        XCTAssertEqual(hostSyncs, ["ask"])
        sync.consume(.init(loggedIn: true))
        await sync.waitForIdleForTesting()
        XCTAssertEqual(hostSyncs, ["ask"])
    }

    func testLoggedOutClearsCeilingAndProjectsChangedEffectivePermissionOnce() async throws {
        let (root, store) = try makeStore()
        defer { try? FileManager.default.removeItem(at: root) }
        store.setLocalToolPermission("always")
        store.setLocalToolPermissionCeiling("ask")
        var hostSyncs: [SandLocalToolPermission] = []
        let sync = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: store,
            fetchCeiling: { "ask" },
            syncHost: { hostSyncs.append($0) },
            reportFailure: { _, _, _ in XCTFail("unexpected failure") }
        )
        sync.consume(.init(loggedIn: false))
        await sync.waitForIdleForTesting()
        XCTAssertNil(store.getLocalToolPermissionCeiling())
        XCTAssertEqual(store.getResolvedLocalToolPermission(), "always")
        XCTAssertEqual(hostSyncs, ["always"])
    }

    func testOutOfOrderOlderFetchCannotOverwriteNewerStatusGeneration() async throws {
        let (root, store) = try makeStore()
        defer { try? FileManager.default.removeItem(at: root) }
        store.setLocalToolPermission("always")
        var fetchCount = 0
        var hostSyncs: [SandLocalToolPermission] = []
        let sync = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: store,
            fetchCeiling: {
                fetchCount += 1
                if fetchCount == 1 {
                    do { try await Task.sleep(nanoseconds: 80_000_000) } catch {}
                    return "always"
                }
                return "never"
            },
            syncHost: { hostSyncs.append($0) },
            reportFailure: { _, _, _ in XCTFail("unexpected failure") }
        )
        sync.consume(.init(loggedIn: true))
        try? await Task.sleep(nanoseconds: 10_000_000)
        sync.consume(.init(loggedIn: true))
        await sync.waitForIdleForTesting()
        try? await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertEqual(store.getLocalToolPermissionCeiling(), "never")
        XCTAssertEqual(store.getResolvedLocalToolPermission(), "never")
        XCTAssertEqual(hostSyncs, ["never"])
    }

    func testSameEffectivePermissionDoesNotRepeatHostSync() async throws {
        let (root, store) = try makeStore()
        defer { try? FileManager.default.removeItem(at: root) }
        store.setLocalToolPermission("ask")
        var hostSyncs: [SandLocalToolPermission] = []
        let sync = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: store,
            fetchCeiling: { "always" },
            syncHost: { hostSyncs.append($0) },
            reportFailure: { _, _, _ in XCTFail("unexpected failure") }
        )
        sync.consume(.init(loggedIn: true))
        await sync.waitForIdleForTesting()
        XCTAssertEqual(store.getResolvedLocalToolPermission(), "ask")
        XCTAssertTrue(hostSyncs.isEmpty)
    }

    func testFetchAndHostFailuresFailClosed() async throws {
        let (fetchRoot, fetchStore) = try makeStore()
        defer { try? FileManager.default.removeItem(at: fetchRoot) }
        fetchStore.setLocalToolPermission("always")
        var fetchFailures: [String] = []
        var fetchHostSyncs: [SandLocalToolPermission] = []
        let fetchFailure = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: fetchStore,
            fetchCeiling: { throw TestError.fetch },
            syncHost: { fetchHostSyncs.append($0) },
            reportFailure: { area, leg, _ in fetchFailures.append("\(area):\(leg)") }
        )
        fetchFailure.consume(.init(loggedIn: true))
        await fetchFailure.waitForIdleForTesting()
        XCTAssertEqual(fetchStore.getResolvedLocalToolPermission(), "never")
        XCTAssertEqual(fetchHostSyncs, ["never"])
        XCTAssertEqual(fetchFailures, ["cursor-profile:local-tool-ceiling-fetch"])

        let (hostRoot, hostStore) = try makeStore()
        defer { try? FileManager.default.removeItem(at: hostRoot) }
        hostStore.setLocalToolPermission("always")
        var attempts: [SandLocalToolPermission] = []
        var failures: [String] = []
        let hostFailure = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: hostStore,
            fetchCeiling: { "ask" },
            syncHost: { permission in
                attempts.append(permission)
                if permission == "ask" { throw TestError.host }
            },
            reportFailure: { area, leg, _ in failures.append("\(area):\(leg)") }
        )
        hostFailure.consume(.init(loggedIn: true))
        await hostFailure.waitForIdleForTesting()
        XCTAssertEqual(hostStore.getResolvedLocalToolPermission(), "ask")
        XCTAssertEqual(attempts, ["ask", "never"])
        XCTAssertEqual(failures, ["host-settings:local-tool-ceiling"])
    }
}

@MainActor
private final class CursorAuthFreshnessCredentialStore: IOSCursorCredentialStoring {
    private var values: [String: String] = [
        IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY: "freshness-access",
        IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY: "freshness-refresh",
        iosMachineIDSecretKey: "freshness-machine",
    ]
    func readSecret(_ key: String) async throws -> String? { values[key] }
    func writeSecret(_ key: String, value: String) async throws { values[key] = value }
    func deleteSecret(_ key: String) async throws { values.removeValue(forKey: key) }
    func waitForEncryptedStorage() async throws {}
}

private func cursorAuthProfileTestToken(
    authId: String = "github|42",
    email: String = "ada@example.test"
) throws -> String {
    let payload = try JSONSerialization.data(withJSONObject: [
        "sub": authId,
        "email": email,
    ])
    let encoded = payload.base64EncodedString()
        .replacingOccurrences(of: "+", with: "-")
        .replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
    return "e30.\(encoded).signature"
}

@MainActor
final class CursorAuthFreshnessParityTests: XCTestCase {
    func testPublishedLogoutAdvancesFreshnessAndStatusRetainsLatestGeneration() async throws {
        let service = IOSCursorAuthService(
            store: CursorAuthFreshnessCredentialStore(),
            backendURL: URL(string: "https://backend.example.test")!,
            websiteURL: URL(string: "https://cursor.example.test")!,
            structuredLogRequestExecutor: { request in
                (Data(), HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: "HTTP/2", headerFields: [:])!)
            }
        )
        var observed: [IOSCursorAuthStatus] = []
        service.setStatusObserver { observed.append($0) }
        await Task.yield()
        let before = await service.status()
        XCTAssertEqual(before.freshness, 0)
        try await service.logout()
        let after = await service.status()
        XCTAssertFalse(after.loggedIn)
        XCTAssertEqual(after.freshness, 1)
        XCTAssertEqual(observed.last?.freshness, 1)
    }

    func testAuthStatusEnrichesAndCachesCanonicalProfile() async throws {
        let store = CursorAuthFreshnessCredentialStore()
        try await store.writeSecret(
            IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY,
            value: try cursorAuthProfileTestToken()
        )
        let service = IOSCursorAuthService(
            store: store,
            backendURL: URL(string: "https://backend.example.test")!,
            websiteURL: URL(string: "https://cursor.example.test")!
        )
        var fetchCount = 0
        service.setProfileFetcher {
            fetchCount += 1
            return .init(
                displayName: "Ada Lovelace",
                email: "profile@example.test",
                profilePictureUrl: "https://images.example.test/ada.png",
                isAnysphereUser: true
            )
        }

        let first = await service.status()
        XCTAssertTrue(first.loggedIn)
        XCTAssertEqual(first.authId, "github|42")
        XCTAssertEqual(first.email, "ada@example.test")
        XCTAssertEqual(first.displayName, "Ada Lovelace")
        XCTAssertEqual(first.profilePictureUrl, "https://images.example.test/ada.png")
        XCTAssertEqual(first.isAnysphereUser, true)
        XCTAssertEqual(fetchCount, 1)

        let second = await service.status()
        XCTAssertEqual(second.displayName, "Ada Lovelace")
        XCTAssertEqual(fetchCount, 1)
    }

    func testLateProfileFetchCannotCrossNewAuthOperation() async throws {
        let store = CursorAuthFreshnessCredentialStore()
        try await store.writeSecret(
            IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY,
            value: try cursorAuthProfileTestToken()
        )
        let service = IOSCursorAuthService(
            store: store,
            backendURL: URL(string: "https://backend.example.test")!,
            websiteURL: URL(string: "https://cursor.example.test")!
        )
        var firstContinuation: CheckedContinuation<IOSCursorAuthProfile?, Never>?
        var fetchCount = 0
        service.setProfileFetcher {
            fetchCount += 1
            if fetchCount == 1 {
                return await withCheckedContinuation { continuation in
                    firstContinuation = continuation
                }
            }
            return nil
        }

        let read = Task { @MainActor in await service.status() }
        while firstContinuation == nil {
            await Task.yield()
        }
        let replacement = try service.beginLogin()
        firstContinuation?.resume(returning: .init(
            displayName: "Stale Name",
            email: "stale@example.test",
            profilePictureUrl: "https://images.example.test/stale.png",
            isAnysphereUser: true
        ))

        let status = await read.value
        XCTAssertNil(status.displayName)
        XCTAssertNil(status.profilePictureUrl)
        XCTAssertNil(status.isAnysphereUser)
        XCTAssertGreaterThanOrEqual(fetchCount, 2)
        service.cancelLogin(attemptId: replacement.attemptId)
    }

    func testDisplayNameMutationSettlesThroughAuthOwnerAndClearsOnLogout() async throws {
        let store = CursorAuthFreshnessCredentialStore()
        try await store.writeSecret(
            IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY,
            value: try cursorAuthProfileTestToken()
        )
        let service = IOSCursorAuthService(
            store: store,
            backendURL: URL(string: "https://backend.example.test")!,
            websiteURL: URL(string: "https://cursor.example.test")!,
            structuredLogRequestExecutor: { request in
                (
                    Data(),
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/2",
                        headerFields: [:]
                    )!
                )
            }
        )
        var writes: [String] = []
        var observed: [IOSCursorAuthStatus] = []
        service.setStatusObserver { observed.append($0) }
        await Task.yield()

        let updated = try await service.updateDisplayName("  Ada   Lovelace  ") { name in
            writes.append(name)
        }
        XCTAssertEqual(writes, ["Ada Lovelace"])
        XCTAssertEqual(updated.displayName, "Ada Lovelace")
        XCTAssertEqual(updated.freshness, 1)
        XCTAssertEqual(observed.last?.displayName, "Ada Lovelace")

        let settled = await service.status()
        XCTAssertEqual(settled.displayName, "Ada Lovelace")
        try await service.logout()
        let loggedOut = await service.status()
        XCTAssertFalse(loggedOut.loggedIn)
        XCTAssertNil(loggedOut.displayName)
        XCTAssertEqual(loggedOut.freshness, 2)
    }

    func testDisplayNameLateResultCannotPublishAcrossNewAuthOperation() async throws {
        let store = CursorAuthFreshnessCredentialStore()
        try await store.writeSecret(
            IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY,
            value: try cursorAuthProfileTestToken()
        )
        let service = IOSCursorAuthService(
            store: store,
            backendURL: URL(string: "https://backend.example.test")!,
            websiteURL: URL(string: "https://cursor.example.test")!
        )
        var releaseWriter: CheckedContinuation<Void, Never>?
        let update = Task { @MainActor in
            try await service.updateDisplayName("Old Account Name") { _ in
                await withCheckedContinuation { continuation in
                    releaseWriter = continuation
                }
            }
        }
        while releaseWriter == nil {
            await Task.yield()
        }
        let replacement = try service.beginLogin()
        releaseWriter?.resume()

        do {
            _ = try await update.value
            XCTFail("stale display-name result must not publish into a newer auth operation")
        } catch let error as IOSCursorAuthError {
            XCTAssertEqual(error, .signInRequired)
        }
        let status = await service.status()
        XCTAssertNil(status.displayName)
        service.cancelLogin(attemptId: replacement.attemptId)
    }
}

@MainActor
final class CursorAuthOperationEpochParityTests: XCTestCase {
    func testNewLoginSupersedesOlderAttemptBeforePolling() async throws {
        let service = IOSCursorAuthService(
            store: CursorAuthFreshnessCredentialStore(),
            backendURL: URL(string: "https://backend.example.test")!,
            websiteURL: URL(string: "https://cursor.example.test")!
        )
        let older = try service.beginLogin()
        let newer = try service.beginLogin()

        do {
            _ = try await service.pollLogin(attemptId: older.attemptId)
            XCTFail("superseded login attempt must not poll or mutate credentials")
        } catch let error as IOSCursorAuthError {
            XCTAssertEqual(error, .loginAttemptNotFound)
        }

        service.cancelLogin(attemptId: newer.attemptId)
        do {
            _ = try await service.pollLogin(attemptId: newer.attemptId)
            XCTFail("cancelled login attempt must remain superseded")
        } catch let error as IOSCursorAuthError {
            XCTAssertEqual(error, .loginAttemptNotFound)
        }
    }
}


final class CursorMdmSignInPolicyParityTests: XCTestCase {
    func testManagedPolicyEncodingNormalizesDesktopFields() throws {
        let header = try XCTUnwrap(iosCursorMdmSignInPolicyHeaderValue([
            "SignInEnforcement": "true",
            "AllowedOrganizationIds": "00042, 42, 0",
            "AllowedTeamIds": "7, 9",
            "AllowedTeamId": 7,
            "AllowedLoginEmails": "Admin@Example.COM, person@example.com",
            "AllowedLoginDomains": "@Example.COM, corp.example",
        ]))
        XCTAssertTrue(header.hasPrefix("v1."))
        let encoded = String(header.dropFirst(3))
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        let padded = encoded + String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        let data = try XCTUnwrap(Data(base64Encoded: padded))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["enforced"] as? Bool, true)
        XCTAssertEqual(object["allowedOrganizationIds"] as? [String], ["42"])
        XCTAssertEqual(object["allowedTeamIds"] as? [String], ["7", "9"])
        XCTAssertEqual(object["allowedLoginEmails"] as? [String], ["admin@example.com", "person@example.com"])
        XCTAssertEqual(object["allowedLoginDomains"] as? [String], ["corp.example", "example.com"])
    }

    func testManagedPolicyHeaderIsAbsentWithoutRestrictions() {
        XCTAssertNil(iosCursorMdmSignInPolicyHeaderValue([:]))
        var request = URLRequest(url: URL(string: "https://backend.example.test/auth/poll")!)
        applyIOSCursorMdmSignInPolicy(to: &request, headerValue: "v1.policy")
        XCTAssertEqual(
            request.value(forHTTPHeaderField: IOS_CURSOR_MDM_SIGN_IN_POLICY_HEADER_NAME),
            "v1.policy"
        )
    }

    func testPolicyViolationPayloadIsNarrowAndUsesCanonicalError() {
        XCTAssertTrue(iosCursorIsSignInPolicyViolationPayload(
            Data(#"{"error":"sign_in_policy_violation"}"#.utf8)
        ))
        XCTAssertFalse(iosCursorIsSignInPolicyViolationPayload(
            Data(#"{"error":"other"}"#.utf8)
        ))
        XCTAssertEqual(
            IOSCursorAuthError.signInPolicyViolation.errorDescription,
            IOS_CURSOR_SIGN_IN_POLICY_VIOLATION_MESSAGE
        )
    }
}
