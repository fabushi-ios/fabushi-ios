import Foundation
import XCTest
@testable import Fabushi

@MainActor
final class IOSDandelionNativeRuntimeTests: XCTestCase {
    private final class FakeSecretStore: IOSDandelionNativeSecretStoring {
        var requested: [(String, String)] = []
        func loadOrCreateSecret(accountScope: String, appId: String) throws -> Data {
            requested.append((accountScope, appId))
            return Data(repeating: 7, count: 32)
        }
    }

    private final class FakeHost: MahayanaHostRequesting, MahayanaDandelionNativeRequesting, @unchecked Sendable {
        let scope: String
        let generation: String
        var nativeMethods: [String] = []
        var nativeParams: [[String: Any]] = []

        init(scope: String, generation: String) {
            self.scope = scope
            self.generation = generation
        }

        @MainActor
        func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult {
            guard method == "feature.plugin.accountIdentity" else {
                throw MahayanaHostRuntime.HostError.requestFailed("unexpected method")
            }
            return MahayanaHostJSONResult(value: [
                "protocol": "fabushi.plugin-account-identity.v1",
                "accountScope": scope,
                "accountGeneration": generation,
            ])
        }

        @MainActor
        func dandelionNativeRequest(
            method: String,
            params: [String: Any]
        ) async throws -> MahayanaHostJSONResult {
            nativeMethods.append(method)
            nativeParams.append(params)
            if method == "probeFile" {
                return MahayanaHostJSONResult(value: [
                    "length": 5,
                    "sha256": String(repeating: "a", count: 64),
                    "revision": 1,
                ])
            }
            return MahayanaHostJSONResult(value: [
                "protocol": "fabushi.ios.dandelion-native-owner.v1",
                "accountBound": method != "clearAccount",
                "grantCount": method == "registerSource" ? 1 : 0,
                "revision": 1,
                "nativeRuntimeEnabled": false,
            ])
        }
    }

    func testRuntimeGrantPickerAndSecureStorageStayAccountFenced() async throws {
        let scope = String(repeating: "a", count: 64)
        let generation = String(repeating: "b", count: 64)
        let host = FakeHost(scope: scope, generation: generation)
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("dandelion-picker-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let secretStore = FakeSecretStore()
        var scopeStarts = 0
        var scopeStops = 0
        let broker = IOSDandelionNativeRuntimeBroker(
            hostSupervisor: supervisor,
            pickDirectory: { root },
            confirmRuntime: { _, _, _ in true },
            beginSecurityScope: { _ in scopeStarts += 1; return true },
            endSecurityScope: { _ in scopeStops += 1 },
            secretStore: secretStore
        )
        broker.updateAccountScope(scope)

        let runtimeGrantResult = try await broker.requestRuntimeGrant(
            appId: "global-dharma",
            version: "1.0.0",
            packageDigest: String(repeating: "c", count: 64)
        )
        let runtimeGrant = try XCTUnwrap(runtimeGrantResult)
        let sourceGrantResult = try await broker.registerSelectedSource(runtimeGrant: runtimeGrant)
        let sourceGrant = try XCTUnwrap(sourceGrantResult)
        XCTAssertEqual(scopeStarts, 1)
        XCTAssertTrue(host.nativeMethods.contains("syncAccount"))
        XCTAssertTrue(host.nativeMethods.contains("registerSource"))

        let probe = try await broker.probeSource(
            runtimeGrant: runtimeGrant,
            grantId: sourceGrant,
            relativePath: "scripture.txt",
            maxBytes: 1024
        )
        XCTAssertEqual((probe["length"] as? NSNumber)?.intValue, 5)

        let secret = try broker.runtimeSecret(for: runtimeGrant)
        XCTAssertEqual(secret, Data(repeating: 7, count: 32))
        XCTAssertEqual(secretStore.requested.first?.0, scope)
        XCTAssertEqual(secretStore.requested.first?.1, "global-dharma")

        await broker.revokeSource(sourceGrant)
        XCTAssertEqual(scopeStops, 1)
        XCTAssertTrue(host.nativeMethods.contains("revokeSource"))
        try? FileManager.default.removeItem(at: root)
    }

    func testAccountTransitionInvalidatesRuntimeGrantAndReleasesSource() async throws {
        let scope = String(repeating: "a", count: 64)
        let host = FakeHost(scope: scope, generation: String(repeating: "b", count: 64))
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("dandelion-transition-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var stopped = 0
        let broker = IOSDandelionNativeRuntimeBroker(
            hostSupervisor: supervisor,
            pickDirectory: { root },
            confirmRuntime: { _, _, _ in true },
            beginSecurityScope: { _ in true },
            endSecurityScope: { _ in stopped += 1 },
            secretStore: FakeSecretStore()
        )
        broker.updateAccountScope(scope)
        let runtimeGrantResult = try await broker.requestRuntimeGrant(
            appId: "global-dharma",
            version: "1.0.0",
            packageDigest: String(repeating: "c", count: 64)
        )
        let runtimeGrant = try XCTUnwrap(runtimeGrantResult)
        _ = try await broker.registerSelectedSource(runtimeGrant: runtimeGrant)
        await broker.prepareAccountTransition()
        XCTAssertEqual(stopped, 1)
        XCTAssertTrue(host.nativeMethods.contains("clearAccount"))

        do {
            _ = try broker.runtimeSecret(for: runtimeGrant)
            XCTFail("stale runtime grant must be rejected")
        } catch {
            XCTAssertTrue(true)
        }
        try? FileManager.default.removeItem(at: root)
    }
}
