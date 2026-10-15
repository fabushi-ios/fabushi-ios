import XCTest
@testable import Fabushi

@MainActor
private final class MemoryBoxSecretsStore: IOSBoxSecretsStoring {
    var snapshots: [String: [String: String]] = [:]

    func readSnapshot(accountSlot: String) async throws -> [String: String] {
        snapshots[accountSlot] ?? [:]
    }

    func writeSnapshot(accountSlot: String, secrets: [String: String]) async throws {
        snapshots[accountSlot] = secrets
    }

    func deleteSnapshot(accountSlot: String) async throws {
        snapshots.removeValue(forKey: accountSlot)
    }
}

@MainActor
private final class BoxSecretsApplyGate {
    private(set) var started = false
    private var continuation: CheckedContinuation<Void, Never>?

    func wait() async {
        started = true
        await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func open() {
        continuation?.resume()
        continuation = nil
    }
}

@MainActor
private func waitUntilBoxSecretsGateStarts(_ gate: BoxSecretsApplyGate) async {
    for _ in 0..<64 where !gate.started {
        await Task.yield()
    }
}

final class BoxSecretsParityTests: XCTestCase {
    @MainActor
    func testHappyPathValidatesPersistsAndPushesWithoutLoggingValues() async throws {
        let store = MemoryBoxSecretsStore()
        var applied: [[String: String]] = []
        var telemetry: [IOSBoxSecretsTelemetryProjection] = []
        let controller = IOSBoxSecretsController(
            store: store,
            apply: { secrets in
                applied.append(secrets)
                return true
            },
            reporter: { telemetry.append($0) }
        )
        _ = controller.adoptAccount("account-a")

        let synced = try await controller.upsert([
            "API_TOKEN": "top-secret-value",
            "FEATURE_KEY": "enabled",
        ])

        XCTAssertTrue(synced)
        XCTAssertEqual(try await controller.listKeys(), ["API_TOKEN", "FEATURE_KEY"])
        XCTAssertEqual(try await controller.reveal(key: "API_TOKEN"), "top-secret-value")
        XCTAssertEqual(applied.last?["API_TOKEN"], "top-secret-value")
        XCTAssertTrue(
            telemetry.flatMap { $0.metadata.values }.allSatisfy { !$0.contains("top-secret-value") },
            "telemetry must never contain secret values"
        )
        XCTAssertEqual(telemetry.last?.metadata["key_count"], "2")
        XCTAssertNil(telemetry.last?.metadata["account_scope"])
    }

    @MainActor
    func testValidationRejectsReservedNamesBeforePersistenceOrPush() async throws {
        let store = MemoryBoxSecretsStore()
        var applyCount = 0
        let controller = IOSBoxSecretsController(
            store: store,
            apply: { _ in
                applyCount += 1
                return true
            },
            reporter: { _ in }
        )
        _ = controller.adoptAccount("account-a")

        do {
            _ = try await controller.upsert(["PATH": "must-not-land"])
            XCTFail("reserved runtime variables must be rejected")
        } catch let error as IOSBoxSecretsError {
            guard case .validation = error else {
                return XCTFail("expected validation error, got \(error)")
            }
        }

        XCTAssertEqual(store.snapshots["account-a"], nil)
        XCTAssertEqual(applyCount, 0)
    }

    @MainActor
    func testAccountSwitchFencesInflightPushAndDepartureClearsHost() async throws {
        let store = MemoryBoxSecretsStore()
        store.snapshots["account-a"] = ["TOKEN": "a"]
        store.snapshots["account-b"] = ["TOKEN": "b"]
        let gate = BoxSecretsApplyGate()
        var applied: [[String: String]] = []
        let controller = IOSBoxSecretsController(
            store: store,
            apply: { secrets in
                applied.append(secrets)
                if secrets["TOKEN"] == "a" {
                    await gate.wait()
                }
                return true
            },
            reporter: { _ in }
        )
        _ = controller.adoptAccount("account-a")

        let oldPush = Task { @MainActor in
            try await controller.push(trigger: "account")
        }
        await waitUntilBoxSecretsGateStarts(gate)
        XCTAssertTrue(gate.started)

        _ = controller.adoptAccount("account-b")
        gate.open()

        do {
            _ = try await oldPush.value
            XCTFail("old account push must be fenced after account switch")
        } catch let error as IOSBoxSecretsError {
            XCTAssertEqual(error, .superseded)
        }

        XCTAssertTrue(try await controller.push(trigger: "account"))
        XCTAssertEqual(applied.last, ["TOKEN": "b"])

        await controller.departAccount()
        XCTAssertEqual(applied.last, [:], "logout/account departure must clear Host box secrets")
        XCTAssertNil(controller.accountSlot)
    }

    @MainActor
    func testCancellationDoesNotReportSuccessfulCompletion() async throws {
        let store = MemoryBoxSecretsStore()
        store.snapshots["account-a"] = ["TOKEN": "a"]
        let gate = BoxSecretsApplyGate()
        var telemetry: [IOSBoxSecretsTelemetryProjection] = []
        let controller = IOSBoxSecretsController(
            store: store,
            apply: { _ in
                await gate.wait()
                try Task.checkCancellation()
                return true
            },
            reporter: { telemetry.append($0) }
        )
        _ = controller.adoptAccount("account-a")

        let push = Task { @MainActor in
            try await controller.push(trigger: "edit")
        }
        await waitUntilBoxSecretsGateStarts(gate)
        push.cancel()
        gate.open()

        do {
            _ = try await push.value
            XCTFail("cancelled push must not succeed")
        } catch is CancellationError {
            // Expected.
        }
        XCTAssertFalse(telemetry.contains { $0.metadata["outcome"] == "ok" })
    }

    @MainActor
    func testRestartReusesOnlyMatchingAccountSnapshot() async throws {
        let store = MemoryBoxSecretsStore()
        let first = IOSBoxSecretsController(
            store: store,
            apply: { _ in true },
            reporter: { _ in }
        )
        _ = first.adoptAccount("account-a")
        _ = try await first.upsert(["A_TOKEN": "persisted"])

        let restarted = IOSBoxSecretsController(
            store: store,
            apply: { _ in true },
            reporter: { _ in }
        )
        _ = restarted.adoptAccount("account-a")
        XCTAssertEqual(try await restarted.reveal(key: "A_TOKEN"), "persisted")

        _ = restarted.adoptAccount("account-b")
        XCTAssertNil(try await restarted.reveal(key: "A_TOKEN"))
    }

    func testShippingRuntimeOwnsBoxSecretsBeforeCoordinatorFallback() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let main = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        let provider = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/coordinator/production-provider.swift"),
            encoding: .utf8
        )
        let cursorAuth = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/account/cursor-auth.swift"),
            encoding: .utf8
        )
        let boxSecrets = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/account/box-secrets.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(main.contains("boxSecretsMainRPC.route(method: method, args: args)"))
        XCTAssertTrue(main.contains("method: \"setBoxSecrets\""))
        XCTAssertTrue(main.contains("await boxSecretsController.departAccount()"))
        XCTAssertTrue(provider.contains("pushBoxSecrets(trigger: \"startup\")"))
        XCTAssertTrue(provider.contains("pushBoxSecrets(trigger: \"retry\")"))
        XCTAssertTrue(boxSecrets.contains("com.ombhrum.fabushi.box-secrets.v1"))
        XCTAssertTrue(cursorAuth.contains("com.ombhrum.fabushi.cursor"))
        XCTAssertFalse(boxSecrets.contains("com.ombhrum.fabushi.cursor"))
    }
}
