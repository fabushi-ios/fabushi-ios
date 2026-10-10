import XCTest
@testable import Fabushi

final class MobileComposerDraftPersistenceTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let suite = "MobileComposerDraftPersistenceTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return defaults
    }

    private let attachment = MobileComposerAttachment(
        id: "sha",
        name: "notes.txt",
        path: "/private/fabushi/notes.txt",
        mimeType: "text/plain",
        sizeBytes: 12
    )

    func testAccountAndAgentScopesAreIsolated() {
        let defaults = defaults()
        MobileComposerDraftPersistence.save(
            accountScopeKey: "account-a",
            agentID: "agent-a",
            snapshot: .init(text: "hello", attachments: [attachment], recovery: nil),
            defaults: defaults
        )
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account-a",
                agentID: "agent-a",
                defaults: defaults
            ).text,
            "hello"
        )
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account-b",
                agentID: "agent-a",
                defaults: defaults
            ),
            .empty
        )
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account-a",
                agentID: "agent-b",
                defaults: defaults
            ),
            .empty
        )
    }

    func testRecoveryPayloadSurvivesRestartAndEmptySnapshotClears() {
        let defaults = defaults()
        let recovery = MobileComposerRecovery(
            requestId: "request-1",
            text: "",
            attachments: [attachment]
        )
        MobileComposerDraftPersistence.save(
            accountScopeKey: "account",
            agentID: "agent",
            snapshot: .init(text: "", attachments: [], recovery: recovery),
            defaults: defaults
        )
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account",
                agentID: "agent",
                defaults: defaults
            ).recovery,
            recovery
        )
        MobileComposerDraftPersistence.save(
            accountScopeKey: "account",
            agentID: "agent",
            snapshot: .empty,
            defaults: defaults
        )
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account",
                agentID: "agent",
                defaults: defaults
            ),
            .empty
        )
    }

    func testCorruptOrWrongSchemaDataFailsClosedAndIsRemoved() throws {
        let defaults = defaults()
        let key = MobileComposerDraftPersistence.key(accountScopeKey: "account", agentID: "agent")
        defaults.set(Data("not-json".utf8), forKey: key)
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account",
                agentID: "agent",
                defaults: defaults
            ),
            .empty
        )
        XCTAssertNil(defaults.data(forKey: key))

        let wrong = try JSONSerialization.data(
            withJSONObject: [
                "schemaVersion": 99,
                "text": "bad",
                "attachments": [],
            ]
        )
        defaults.set(wrong, forKey: key)
        XCTAssertEqual(
            MobileComposerDraftPersistence.load(
                accountScopeKey: "account",
                agentID: "agent",
                defaults: defaults
            ),
            .empty
        )
        XCTAssertNil(defaults.data(forKey: key))
    }
}
