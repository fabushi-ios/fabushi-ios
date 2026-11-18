#if canImport(XCTest)
import XCTest
@testable import Fabushi

final class SemanticWorkspaceTreeTests: XCTestCase {

    func testDeterministicHashPartsAndCollisionResistance() {
        let h1 = SemanticWorkspaceTree.computeHashParts(
            layout: "Normal",
            chatId: 42,
            text: "hi",
            modal: nil,
            call: false
        )
        let h2 = SemanticWorkspaceTree.computeHashParts(
            layout: "Normal",
            chatId: 42,
            text: "hi",
            modal: nil,
            call: false
        )
        let h3 = SemanticWorkspaceTree.computeHashParts(
            layout: "Normal",
            chatId: 43,
            text: "hi",
            modal: nil,
            call: false
        )
        XCTAssertEqual(h1, h2)
        XCTAssertNotEqual(h1, h3)
        XCTAssertEqual(h1.count, 64)

        // Length-prefix & Option tag framing eliminates delimiter/sentinel collisions
        XCTAssertNotEqual(
            SemanticWorkspaceTree.computeHashParts(layout: "Normal", chatId: 1, text: "foo|bar", modal: "baz", call: false),
            SemanticWorkspaceTree.computeHashParts(layout: "Normal", chatId: 1, text: "foo", modal: "bar|baz", call: false)
        )
        XCTAssertNotEqual(
            SemanticWorkspaceTree.computeHashParts(layout: "Normal", chatId: nil, text: "hi", modal: nil, call: false),
            SemanticWorkspaceTree.computeHashParts(layout: "Normal", chatId: 0, text: "hi", modal: nil, call: false)
        )
        XCTAssertNotEqual(
            SemanticWorkspaceTree.computeHashParts(layout: "Normal", chatId: 1, text: "hi", modal: nil, call: false),
            SemanticWorkspaceTree.computeHashParts(layout: "Normal", chatId: 1, text: "hi", modal: "none", call: false)
        )
    }

    func testBitExactSwiftAndRustFFIStateHashAndActionsParity() async throws {
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("SemanticParity_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }

        let client = HostBridgeClient(
            keychain: KeychainSessionStore(serviceNamespace: "test.semantic.\(UUID().uuidString)")
        )
        try await client.initialize(appDataDirectory: sandbox, isTestMode: true)

        // 1. Parts hash bit-for-bit parity between Swift CryptoKit.SHA256 and Rust sha2::Sha256
        struct PartsParams: Codable, Sendable {
            let layout: String
            let chat_id: UInt64?
            let text: String
            let modal: String?
            let call: Bool
        }
        struct HashResp: Decodable, Sendable { let state_hash: String }
        let rustParts: HashResp = try await client.call(
            "semantic.hash",
            params: PartsParams(layout: "CompactTab", chat_id: 9001, text: "全球法布施", modal: "settings", call: true)
        )
        let swiftParts = SemanticWorkspaceTree.computeHashParts(
            layout: "CompactTab",
            chatId: 9001,
            text: "全球法布施",
            modal: "settings",
            call: true
        )
        XCTAssertEqual(swiftParts, rustParts.state_hash, "Swift and Rust SHA-256 compute_hash_parts MUST match bit-for-bit")

        // 2. Full tree snapshot SHA-256 parity
        let snap1 = try await client.semanticSnapshot()
        XCTAssertEqual(
            snap1.computeFullStateHash(),
            snap1.stateHash,
            "Swift computeFullStateHash() MUST match Rust SemanticWorkspaceTree::compute_full_state_hash() bit-for-bit"
        )

        // 3. SemanticAction round-trips mutate state and produce deterministic new hashes
        let r1 = try await client.semanticAct(.selectChat(chatId: 77))
        XCTAssertTrue(r1.success)
        XCTAssertEqual(r1.status, .succeeded)
        XCTAssertNotEqual(r1.resultingStateHash, snap1.stateHash)

        let r2 = try await client.semanticAct(.composeText(text: "Hello Semantic Tree"))
        XCTAssertNotEqual(r2.resultingStateHash, r1.resultingStateHash)

        let snap2 = try await client.semanticSnapshot()
        XCTAssertEqual(snap2.selectedChatId, 77)
        XCTAssertEqual(snap2.composerText, "Hello Semantic Tree")
        XCTAssertEqual(snap2.computeFullStateHash(), snap2.stateHash)
        XCTAssertEqual(snap2.stateHash, r2.resultingStateHash)

        await client.shutdown()
    }
}
