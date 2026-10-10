import XCTest
@testable import Fabushi

final class MobileUiAgentRefsPersistenceTests: XCTestCase {
    @MainActor
    func testRecentsPersistAcrossRelaunchWithAccountAndAgentIsolation() throws {
        let suite = "FabushiTests.MobileUiAgentRefs.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        var agentA: [String] = []
        for index in 0..<28 {
            agentA = MobileUiAgentRefsPersistence.recordingRecent(
                "assistants:agent-\(index)",
                existing: agentA
            )
        }
        for index in 0..<58 {
            agentA = MobileUiAgentRefsPersistence.recordingRecent(
                "emoji:emoji-\(index)",
                existing: agentA
            )
        }
        MobileUiAgentRefsPersistence.persistRecentKeys(
            agentA,
            accountScopeKey: "account-a",
            agentID: "agent-a",
            defaults: defaults
        )
        MobileUiAgentRefsPersistence.persistRecentKeys(
            ["tools:github"],
            accountScopeKey: "account-a",
            agentID: "agent-b",
            defaults: defaults
        )

        let restoredA = MobileUiAgentRefsPersistence.loadRecentKeys(
            accountScopeKey: "account-a",
            agentID: "agent-a",
            defaults: defaults
        )
        XCTAssertEqual(restoredA.filter { $0.hasPrefix("assistants:") }.count, 20)
        XCTAssertEqual(restoredA.filter { $0.hasPrefix("emoji:") }.count, 50)
        XCTAssertEqual(
            MobileUiAgentRefsPersistence.loadRecentKeys(
                accountScopeKey: "account-a",
                agentID: "agent-b",
                defaults: defaults
            ),
            ["tools:github"]
        )
        XCTAssertTrue(
            MobileUiAgentRefsPersistence.loadRecentKeys(
                accountScopeKey: "account-b",
                agentID: "agent-a",
                defaults: defaults
            ).isEmpty
        )
    }

    @MainActor
    func testCorruptEnvelopeIsClearedFailClosed() throws {
        let suite = "FabushiTests.MobileUiAgentRefs.Corrupt.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let key = try XCTUnwrap(
            MobileUiAgentRefsPersistence.storageKey("account-a")
        )
        defaults.set(Data("not-json".utf8), forKey: key)

        XCTAssertTrue(
            MobileUiAgentRefsPersistence.loadRecentKeys(
                accountScopeKey: "account-a",
                agentID: "agent-a",
                defaults: defaults
            ).isEmpty
        )
        XCTAssertNil(defaults.object(forKey: key))
    }

    @MainActor
    func testEmojiCatalogIdsShareTheCanonicalRecentSlice() {
        let keys = [
            "assistants:alpha",
            "emoji:smile",
            "automations:daily",
            "emoji:wave",
        ]
        XCTAssertEqual(
            MobileUiAgentRefsPersistence.emojiCatalogIDs(from: keys),
            ["smile", "wave"]
        )
    }

    @MainActor
    func testUnknownRecentCategoriesAreRejected() {
        XCTAssertEqual(
            MobileUiAgentRefsPersistence.normalizedRecentKeys([
                "assistants:a",
                "unknown:x",
                "emoji:e",
                "assistants:a",
            ]),
            ["assistants:a", "emoji:e"]
        )
    }
}
