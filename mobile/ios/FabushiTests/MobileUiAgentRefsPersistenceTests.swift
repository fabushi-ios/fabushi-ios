import XCTest
@testable import Fabushi

final class MobileUiAgentRefsPersistenceTests: XCTestCase {
    @MainActor
    func testRecentsPersistAcrossRelaunchAtAccountScope() throws {
        let suite = "FabushiTests.MobileUiAgentRefs.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        var recents: [String] = []
        for index in 0..<28 {
            recents = MobileUiAgentRefsPersistence.recordingRecent(
                "assistants:agent-\(index)",
                existing: recents
            )
        }
        for index in 0..<58 {
            recents = MobileUiAgentRefsPersistence.recordingRecent(
                "emoji:emoji-\(index)",
                existing: recents
            )
        }
        MobileUiAgentRefsPersistence.persistRecentKeys(
            recents,
            accountScopeKey: "account-a",
            defaults: defaults
        )

        let restored = MobileUiAgentRefsPersistence.loadRecentKeys(
            accountScopeKey: "account-a",
            defaults: defaults
        )
        XCTAssertEqual(restored.filter { $0.hasPrefix("assistants:") }.count, 20)
        XCTAssertEqual(restored.filter { $0.hasPrefix("emoji:") }.count, 50)
        XCTAssertTrue(
            MobileUiAgentRefsPersistence.loadRecentKeys(
                accountScopeKey: "account-b",
                defaults: defaults
            ).isEmpty
        )
    }

    @MainActor
    func testSameAccountRecentsAreSharedAcrossAgentSwitches() throws {
        let suite = "FabushiTests.MobileUiAgentRefs.Shared.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        let values = ["assistants:alpha", "tools:github", "emoji:👍"]
        MobileUiAgentRefsPersistence.persistRecentKeys(
            values,
            accountScopeKey: "account-a",
            defaults: defaults
        )
        XCTAssertEqual(
            MobileUiAgentRefsPersistence.loadRecentKeys(
                accountScopeKey: "account-a",
                defaults: defaults
            ),
            values
        )
    }

    @MainActor
    func testCorruptAndWrongSchemaEnvelopesAreClearedFailClosed() throws {
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
                defaults: defaults
            ).isEmpty
        )
        XCTAssertNil(defaults.object(forKey: key))

        let wrongSchema = #"{"schemaVersion":2,"recentKeys":["assistants:a"]}"#
        defaults.set(Data(wrongSchema.utf8), forKey: key)
        XCTAssertTrue(
            MobileUiAgentRefsPersistence.loadRecentKeys(
                accountScopeKey: "account-a",
                defaults: defaults
            ).isEmpty
        )
        XCTAssertNil(defaults.object(forKey: key))
    }

    @MainActor
    func testEmojiValuesStoreActualEmojiNotCatalogIds() {
        let keys = [
            "assistants:alpha",
            "emoji:👍",
            "automations:daily",
            "emoji:❤️",
        ]
        XCTAssertEqual(
            MobileUiAgentRefsPersistence.emojiValues(from: keys),
            ["👍", "❤️"]
        )
    }

    @MainActor
    func testUnknownRecentCategoriesAndDuplicatesAreRejected() {
        XCTAssertEqual(
            MobileUiAgentRefsPersistence.normalizedRecentKeys([
                "assistants:a",
                "unknown:x",
                "emoji:👍",
                "assistants:a",
            ]),
            ["assistants:a", "emoji:👍"]
        )
    }
}
