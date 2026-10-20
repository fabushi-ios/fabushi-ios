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
        var deliveredBootstrap = false
        var cancelledRunnerIds: [String] = []
        var runtimeGenerationOverride: String?
        var requestedMethods: [String] = []
        var activePointerOverride: [String: Any]?

        init(scope: String, generation: String) {
            self.scope = scope
            self.generation = generation
        }

        @MainActor
        func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult {
            requestedMethods.append(method)
            switch method {
            case "feature.plugin.accountIdentity":
                return MahayanaHostJSONResult(value: [
                    "protocol": "fabushi.plugin-account-identity.v1",
                    "accountScope": scope,
                    "accountGeneration": generation,
                ])
            case "feature.plugin.active":
                if let activePointerOverride {
                    return MahayanaHostJSONResult(value: activePointerOverride)
                }
                return MahayanaHostJSONResult(value: [
                    "pluginId": params["pluginId"] as? String ?? "global-dharma",
                    "version": "1.0.0",
                    "artifactSha256": String(repeating: "c", count: 64),
                    "runtime": "native",
                    "installedPath": "/tmp/fabushi/plugins/global-dharma/1.0.0/artifact",
                    "requestedPermissions": ["native.runtime"],
                ])
            default:
                throw MahayanaHostRuntime.HostError.requestFailed("unexpected method")
            }
        }

        @MainActor
        func dandelionNativeRequest(
            method: String,
            params: [String: Any]
        ) async throws -> MahayanaHostJSONResult {
            nativeMethods.append(method)
            nativeParams.append(params)
            switch method {
            case "probeFile":
                return MahayanaHostJSONResult(value: [
                    "length": 5,
                    "sha256": String(repeating: "a", count: 64),
                    "revision": 1,
                ])
            case "authorizeRuntime":
                return MahayanaHostJSONResult(value: [
                    "token": params["grantToken"]!,
                    "accountScope": scope,
                    "accountGeneration": runtimeGenerationOverride ?? generation,
                    "appId": params["appId"]!,
                    "version": params["version"]!,
                    "packageDigest": params["packageDigest"]!,
                ])
            case "startStaticRunner":
                return MahayanaHostJSONResult(value: [
                    "runnerId": "runner-1",
                    "accountScope": scope,
                    "accountGeneration": runtimeGenerationOverride ?? generation,
                    "appId": params["appId"]!,
                    "version": params["version"]!,
                    "packageDigest": params["packageDigest"]!,
                    "grantToken": params["grantToken"]!,
                ])
            case "nextStaticPlatformRequest":
                if deliveredBootstrap {
                    return MahayanaHostJSONResult(value: NSNull())
                }
                deliveredBootstrap = true
                guard let grant = nativeParams.first(where: { ($0["grantToken"] as? String) != nil }),
                      let token = grant["grantToken"] as? String,
                      let appId = grant["appId"] as? String,
                      let version = grant["version"] as? String,
                      let digest = grant["packageDigest"] as? String
                else {
                    throw MahayanaHostRuntime.HostError.requestFailed("missing runtime grant")
                }
                return MahayanaHostJSONResult(value: [
                    "requestId": "request-1",
                    "method": "dandelion.static.bootstrap",
                    "params": [
                        "protocol": "fabushi.ios.dandelion-static-bootstrap.v1",
                        "runnerId": "runner-1",
                        "accountScope": scope,
                        "accountGeneration": runtimeGenerationOverride ?? generation,
                        "appId": appId,
                        "version": version,
                        "packageDigest": digest,
                        "grantToken": token,
                    ],
                ])
            case "resolveStaticPlatformRequest", "staticRunnerStatus":
                guard let grant = nativeParams.first(where: {
                    ($0["grantToken"] as? String) != nil && ($0["appId"] as? String) != nil
                }) else {
                    throw MahayanaHostRuntime.HostError.requestFailed("missing runtime grant")
                }
                return MahayanaHostJSONResult(value: [
                    "runnerId": "runner-1",
                    "accountScope": scope,
                    "accountGeneration": runtimeGenerationOverride ?? generation,
                    "appId": grant["appId"]!,
                    "version": grant["version"]!,
                    "packageDigest": grant["packageDigest"]!,
                    "phase": "running",
                    "executionEnabled": false,
                    "privateDirectoryOwned": true,
                    "runDirectoryOwned": true,
                ])
            case "cancelStaticRunner":
                if let runnerId = params["runnerId"] as? String {
                    cancelledRunnerIds.append(runnerId)
                }
                return MahayanaHostJSONResult(value: [
                    "cancelled": true,
                    "runner": [
                        "runnerId": params["runnerId"]!,
                        "phase": "cancelled",
                        "executionEnabled": false,
                    ],
                ])
            default:
                return MahayanaHostJSONResult(value: [
                    "protocol": "fabushi.ios.dandelion-native-owner.v1",
                    "accountBound": method != "clearAccount",
                    "grantCount": method == "registerSource" ? 1 : 0,
                    "runtimeGrantCount": method == "authorizeRuntime" ? 1 : 0,
                    "staticRunnerCount": 0,
                    "revision": 1,
                    "nativeRuntimeEnabled": false,
                ])
            }
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
        XCTAssertTrue(host.nativeMethods.contains("authorizeRuntime"))
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

    func testRuntimeGrantRequiresCanonicalActivePointerBeforeAndAfterConfirmation() async throws {
        let scope = String(repeating: "a", count: 64)
        let generation = String(repeating: "b", count: 64)
        let host = FakeHost(scope: scope, generation: generation)
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        var confirmations = 0
        let broker = IOSDandelionNativeRuntimeBroker(
            hostSupervisor: supervisor,
            confirmRuntime: { _, _, _ in
                confirmations += 1
                return true
            }
        )
        broker.updateAccountScope(scope)

        host.activePointerOverride = [
            "pluginId": "global-dharma",
            "version": "1.0.0",
            "artifactSha256": String(repeating: "d", count: 64),
            "runtime": "native",
            "installedPath": "/tmp/fabushi/plugins/global-dharma/1.0.0/artifact",
            "requestedPermissions": ["native.runtime"],
        ]
        do {
            _ = try await broker.requestRuntimeGrant(
                appId: "global-dharma",
                version: "1.0.0",
                packageDigest: String(repeating: "c", count: 64)
            )
            XCTFail("mismatched canonical active pointer must reject before user confirmation")
        } catch {
            XCTAssertEqual(confirmations, 0)
            XCTAssertFalse(host.nativeMethods.contains("authorizeRuntime"))
        }

        host.activePointerOverride = nil
        let grant = try await broker.requestRuntimeGrant(
            appId: "global-dharma",
            version: "1.0.0",
            packageDigest: String(repeating: "c", count: 64)
        )
        XCTAssertNotNil(grant)
        XCTAssertEqual(confirmations, 1)
        XCTAssertEqual(
            host.requestedMethods.filter { $0 == "feature.plugin.active" }.count,
            3
        )
        XCTAssertTrue(host.nativeMethods.contains("authorizeRuntime"))
    }

    func testStaticRunnerFoundationFencesGrantAndKeepsExecutionPromotionClosed() async throws {
        let scope = String(repeating: "a", count: 64)
        let generation = String(repeating: "b", count: 64)
        let host = FakeHost(scope: scope, generation: generation)
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let broker = IOSDandelionNativeRuntimeBroker(
            hostSupervisor: supervisor,
            confirmRuntime: { _, _, _ in true },
            secretStore: FakeSecretStore()
        )
        broker.updateAccountScope(scope)

        let grantResult = try await broker.requestRuntimeGrant(
            appId: "global-dharma",
            version: "1.0.0",
            packageDigest: String(repeating: "c", count: 64)
        )
        let grant = try XCTUnwrap(grantResult)
        XCTAssertEqual(grant.accountGeneration, generation)
        let status = try await broker.bootstrapStaticRunnerFoundation(runtimeGrant: grant)

        XCTAssertEqual(status["runnerId"] as? String, "runner-1")
        XCTAssertEqual(status["executionEnabled"] as? Bool, false)
        XCTAssertEqual(status["privateDirectoryOwned"] as? Bool, true)
        XCTAssertTrue(host.nativeMethods.contains("startStaticRunner"))
        XCTAssertTrue(host.nativeMethods.contains("nextStaticPlatformRequest"))
        XCTAssertTrue(host.nativeMethods.contains("resolveStaticPlatformRequest"))

        await broker.cancelStaticRunner("runner-1")
        XCTAssertEqual(host.cancelledRunnerIds, ["runner-1"])
    }

    func testStaticRunnerRejectsStaleCanonicalGenerationAndCancelsLease() async throws {
        let scope = String(repeating: "a", count: 64)
        let generation = String(repeating: "b", count: 64)
        let host = FakeHost(scope: scope, generation: generation)
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let broker = IOSDandelionNativeRuntimeBroker(
            hostSupervisor: supervisor,
            confirmRuntime: { _, _, _ in true },
            secretStore: FakeSecretStore()
        )
        broker.updateAccountScope(scope)

        let grantResult = try await broker.requestRuntimeGrant(
            appId: "global-dharma",
            version: "1.0.0",
            packageDigest: String(repeating: "c", count: 64)
        )
        let grant = try XCTUnwrap(grantResult)
        host.runtimeGenerationOverride = String(repeating: "d", count: 64)

        do {
            _ = try await broker.bootstrapStaticRunnerFoundation(runtimeGrant: grant)
            XCTFail("stale canonical generation must reject the runner lease")
        } catch is CancellationError {
            XCTAssertEqual(host.cancelledRunnerIds, ["runner-1"])
            XCTAssertTrue(host.nativeMethods.contains("startStaticRunner"))
            XCTAssertTrue(host.nativeMethods.contains("cancelStaticRunner"))
        }
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
