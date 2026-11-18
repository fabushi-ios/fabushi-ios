import Foundation
import XCTest
@testable import Fabushi

final class SemanticWorkspaceTreeTests: XCTestCase {
    func testSwiftAndRustSemanticHashMatchByteForByteAndResistBoundaryCollisions() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("SemanticFFI_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let client = FabushiHostClient(environment: ["FABUSHI_UI_TEST_MODE": "1"])
        _ = try await client.initialize(
            appSupportDir: dir,
            keychainBridge: KeychainStore(appSupportDir: dir, environment: ["FABUSHI_UI_TEST_MODE": "1"]),
            biometricsBridge: BiometricsAuthenticator(environment: ["FABUSHI_UI_TEST_MODE": "1"])
        )
        defer { Task { await client.shutdown() } }

        let rustSnap: SemanticWorkspaceTree = try await client.call("semantic.snapshot")
        XCTAssertEqual(rustSnap.stateHash.count, 64)
        XCTAssertEqual(rustSnap.computeFullStateHash(), rustSnap.stateHash)

        // Length-prefix collision resistance ("ab" + "" != "a" + "b")
        let h1 = SemanticWorkspaceTree.computeHashParts(layout: "ab", chatId: nil, text: "", modal: nil, call: false)
        let h2 = SemanticWorkspaceTree.computeHashParts(layout: "a", chatId: nil, text: "b", modal: nil, call: false)
        XCTAssertNotEqual(h1, h2)

        struct PartsReq: Codable, Sendable {
            let layout: String
            let chat_id: UInt64?
            let text: String
            let modal: String?
            let call: Bool
        }
        struct HashResp: Decodable, Sendable { let state_hash: String }
        let rustParts: HashResp = try await client.call(
            "semantic.compute_hash",
            params: PartsReq(layout: "SplitView", chat_id: 99, text: "你好 👋", modal: "settings", call: true)
        )
        let swiftParts = SemanticWorkspaceTree.computeHashParts(
            layout: "SplitView",
            chatId: 99,
            text: "你好 👋",
            modal: "settings",
            call: true
        )
        XCTAssertEqual(swiftParts, rustParts.state_hash)

        let receipt: SemanticActionReceipt = try await client.call(
            "semantic.perform_action",
            params: SemanticAction.composeText(text: "Deterministic UI action")
        )
        XCTAssertTrue(receipt.success)
        XCTAssertEqual(receipt.status, .succeeded)
        XCTAssertNotEqual(receipt.resultingStateHash, rustSnap.stateHash)
    }

    @MainActor
    func testSemanticTreeExporterWritesAtomicallyToConfiguredPath() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("SemanticExport_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let outFile = dir.appendingPathComponent("semantic_tree.json")

        let exporter = SemanticTreeExporter(
            environment: ["FABUSHI_SEMANTIC_EXPORT_PATH": outFile.path]
        )
        var tree = SemanticWorkspaceTree(windowLayout: "SplitView", selectedChatId: 501, composerText: "Hello")
        tree.recomputeStateHash()
        let json = try exporter.updateAndExport(tree)
        XCTAssertTrue(FileManager.default.fileExists(atPath: outFile.path))

        let diskData = try Data(contentsOf: outFile)
        let decoded = try JSONDecoder().decode(SemanticWorkspaceTree.self, from: diskData)
        XCTAssertEqual(decoded.selectedChatId, 501)
        XCTAssertEqual(decoded.stateHash, tree.stateHash)
        XCTAssertTrue(json.contains(tree.stateHash))
    }
}
