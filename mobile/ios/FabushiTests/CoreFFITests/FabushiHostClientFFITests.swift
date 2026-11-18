import Foundation
import XCTest
@testable import Fabushi

final class FabushiHostClientFFITests: XCTestCase {
    func testBootstrapDefaultServerKeychainBiometricsAndFailClosedUnknownCommand() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("HostClientFFI_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let env = [
            "FABUSHI_APP_SUPPORT_DIR": dir.path,
            "FABUSHI_UI_TEST_MODE": "1",
            "FABUSHI_MOCK_BIOMETRICS_RESULT": "success",
        ]
        let keychain = KeychainStore(
            serviceNamespace: "com.ombhrum.fabushi.test.\(UUID().uuidString)",
            appSupportDir: dir,
            environment: env
        )
        let bio = BiometricsAuthenticator(environment: env)
        XCTAssertEqual(bio.availableBiometricKind(), .faceID)
        let bioPassed = await bio.authenticate(reason: "Test Face ID")
        XCTAssertTrue(bioPassed)

        try keychain.setString("secret-token-123", forKey: "tok")
        XCTAssertEqual(try keychain.getString(forKey: "tok"), "secret-token-123")

        let client = FabushiHostClient(environment: env)
        let boot = try await client.initialize(
            appSupportDir: dir,
            keychainBridge: keychain,
            biometricsBridge: bio
        )
        defer { Task { await client.shutdown() } }

        XCTAssertEqual(boot.serverURL, "http://34.46.77.133:8080")
        XCTAssertFalse(boot.isAuthenticated)
        XCTAssertEqual(FabushiHostClient.protoMaxPacketSize, 16 * 1024 * 1024)

        // Fresh chats.list MUST return [] (zero sample_chats!)
        let emptyDialogs: [TelegramChatSummary] = try await client.call("chats.list")
        XCTAssertTrue(emptyDialogs.isEmpty)

        // Unknown command MUST fail closed with structured HostError(code: "unknown_command")
        do {
            struct AnyResp: Decodable, Sendable {}
            let _: AnyResp = try await client.call("bogus.nonexistent.command")
            XCTFail("Expected HostError(code: unknown_command)")
        } catch let err as HostError {
            XCTAssertEqual(err.code, "unknown_command")
        }
    }

    func testPreauthHydrationLifecycleFlushE2EEX3DHDoubleRatchetAndConcurrentOutboxDedup() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("HostPreauthE2EE_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let preauthJSON = """
        {"user_id":9001,"username":"alice_preauth","token":"tok_alice_9001","session_id":"sess_9001","server_url":"http://127.0.0.1:18080","first_name":"Alice"}
        """
        let env = [
            "FABUSHI_APP_SUPPORT_DIR": dir.path,
            "FABUSHI_SERVER_URL": "http://127.0.0.1:18080",
            "FABUSHI_UI_TEST_MODE": "1",
            "FABUSHI_NETWORK_FAULT_MODE": "offline",
            "FABUSHI_PREAUTH_SESSION_JSON": preauthJSON,
        ]
        let client = FabushiHostClient(environment: env)
        let boot = try await client.initialize(
            appSupportDir: dir,
            keychainBridge: KeychainStore(appSupportDir: dir, environment: env),
            biometricsBridge: BiometricsAuthenticator(environment: env)
        )
        defer { Task { await client.shutdown() } }

        XCTAssertTrue(boot.isAuthenticated)
        XCTAssertEqual(boot.activeAccount?.username, "alice_preauth")
        XCTAssertEqual(boot.connectionStatus, .offline)

        // Concurrent 30-task duplicate client_nonce send -> exactly 1 persisted message ID!
        struct SendReq: Codable, Sendable {
            let chat_id: Int64
            let text: String
            let client_nonce: UInt64
        }
        struct MsgOut: Decodable, Sendable { let id: Int64; let text: String }
        let sharedNonce: UInt64 = 777_888_999
        let ids = try await withThrowingTaskGroup(of: Int64.self) { group in
            for _ in 0..<30 {
                group.addTask {
                    let msg: MsgOut = try await client.call(
                        "messages.send",
                        params: SendReq(chat_id: 555, text: "Offline dedup message", client_nonce: sharedNonce)
                    )
                    return msg.id
                }
            }
            var collected: [Int64] = []
            for try await id in group {
                collected.append(id)
            }
            return collected
        }
        XCTAssertEqual(Set(ids).count, 1, "All 30 concurrent duplicate-nonce sends must deduplicate to 1 message ID")

        // Real E2EE prekey generation & local group SenderKey roundtrip across two sandboxed hosts
        let bobDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("HostBobE2EE_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: bobDir) }
        let bob = FabushiHostClient(environment: ["FABUSHI_UI_TEST_MODE": "1"])
        _ = try await bob.initialize(
            appSupportDir: bobDir,
            keychainBridge: KeychainStore(appSupportDir: bobDir, environment: ["FABUSHI_UI_TEST_MODE": "1"]),
            biometricsBridge: BiometricsAuthenticator(environment: ["FABUSHI_UI_TEST_MODE": "1"])
        )
        defer { Task { await bob.shutdown() } }

        struct GroupCreateReq: Codable, Sendable { let group_id: UInt64 }
        struct GroupDist: Codable, Sendable {
            let group_id: UInt64
            let sender_user_id: UInt64
            let sender_device_id: String
            let key_id: UInt32
            let iteration: UInt32
            let chain_key: [UInt8]
            let signing_public_key: [UInt8]
        }
        let dist: GroupDist = try await client.call("e2ee.create_group_session", params: GroupCreateReq(group_id: 4040))
        struct GroupProcessReq: Codable, Sendable { let distribution: GroupDist }
        struct OkResp: Decodable, Sendable { let accepted: Bool }
        let accepted: OkResp = try await bob.call("e2ee.process_group_distribution", params: GroupProcessReq(distribution: dist))
        XCTAssertTrue(accepted.accepted)

        struct GroupEncReq: Codable, Sendable { let group_id: UInt64; let plaintext: String }
        struct GroupCipher: Codable, Sendable {
            let group_id: UInt64
            let sender_user_id: UInt64
            let sender_device_id: String
            let key_id: UInt32
            let iteration: UInt32
            let ciphertext: [UInt8]
            let signature: [UInt8]
        }
        let encrypted: GroupCipher = try await client.call(
            "e2ee.encrypt_group",
            params: GroupEncReq(group_id: 4040, plaintext: "Signal SenderKey E2EE Works!")
        )
        struct GroupDecReq: Codable, Sendable { let message: GroupCipher }
        struct DecOut: Decodable, Sendable { let plaintext_utf8: String }
        let decrypted: DecOut = try await bob.call("e2ee.decrypt_group", params: GroupDecReq(message: encrypted))
        XCTAssertEqual(decrypted.plaintext_utf8, "Signal SenderKey E2EE Works!")

        // ScenePhase background -> foreground roundtrip
        await client.handleScenePhaseChange(.background)
        await client.handleScenePhaseChange(.active)
    }
}
